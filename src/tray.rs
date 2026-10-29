use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    ptr,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};
use windows_sys::{
    Win32::{
        Foundation::*,
        System::{LibraryLoader::GetModuleHandleW, Threading::CreateMutexW},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::w,
};

#[derive(Clone, Debug)]
pub enum TrayEvent {
    Show,
    Apply,
    SelectProfile(String),
    SaveProfile,
    Refresh,
    Settings,
    Diagnostics,
    Toggle(crate::tray_menu::Preference),
    Failure(String),
    Exit,
}
#[derive(Clone)]
struct NativeState {
    sender: mpsc::Sender<TrayEvent>,
    context: eframe::egui::Context,
    main_window: isize,
    owner_window: isize,
}
static NATIVE_STATE: Mutex<Option<NativeState>> = Mutex::new(None);
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();
static ICON_AVAILABLE: AtomicBool = AtomicBool::new(false);
static REGISTRATION_RESET: AtomicBool = AtomicBool::new(false);
const CALLBACK: u32 = WM_APP + 17;
const SHOW: u32 = WM_APP + 18;

fn native_state() -> Option<NativeState> {
    NATIVE_STATE
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

fn reveal(state: &NativeState, focus: bool) {
    // A hidden Win32 window does not receive redraws reliably. Make it visible
    // natively before asking egui to process the queued tray event.
    let window = state.main_window as HWND;
    unsafe {
        ShowWindow(
            window,
            if IsIconic(window) != 0 {
                SW_RESTORE
            } else {
                SW_SHOW
            },
        );
        if focus {
            SetForegroundWindow(window);
        }
    }
    state
        .context
        .send_viewport_cmd(eframe::egui::ViewportCommand::Visible(true));
    state
        .context
        .send_viewport_cmd(eframe::egui::ViewportCommand::Minimized(false));
    state.context.request_repaint();
}

fn send(event: TrayEvent) {
    if let Some(state) = native_state() {
        let focus = matches!(
            &event,
            TrayEvent::Show | TrayEvent::Settings | TrayEvent::Diagnostics
        );
        let _ = state.sender.send(event);
        reveal(&state, focus);
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW::default();
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = CALLBACK;
    data.hIcon = unsafe { LoadIconW(GetModuleHandleW(ptr::null()), 1usize as *const u16) };
    let tip: Vec<_> = "Monitor Switcher".encode_utf16().collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    data
}

fn register_icon(hwnd: HWND) -> bool {
    unsafe { Shell_NotifyIconW(NIM_ADD, &icon_data(hwnd)) != 0 }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_MEASUREITEM && lp != 0 && unsafe { crate::tray_menu::measure(hwnd, lp) } {
        return 1;
    }
    if msg == WM_DRAWITEM && lp != 0 && unsafe { crate::tray_menu::draw(lp) } {
        return 1;
    }
    if msg == SHOW {
        send(TrayEvent::Show);
        return 0;
    }
    if TASKBAR_CREATED
        .get()
        .is_some_and(|message| *message != 0 && *message == msg)
    {
        let available = register_icon(hwnd);
        ICON_AVAILABLE.store(available, Ordering::Release);
        REGISTRATION_RESET.store(true, Ordering::Release);
        if !available {
            // Explorer may announce its taskbar before accepting icons. Keep a
            // hidden application reachable while maintenance retries the icon.
            if let Some(state) = native_state() {
                reveal(&state, false);
            }
        }
        return 0;
    }
    if msg == CALLBACK {
        match lp as u32 {
            WM_LBUTTONUP | WM_LBUTTONDBLCLK => send(TrayEvent::Show),
            WM_RBUTTONUP => match crate::tray_menu::show(hwnd) {
                Ok(Some(event)) => send(event),
                Ok(None) => {}
                Err(error) => send(TrayEvent::Failure(error)),
            },
            _ => {}
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TrayStatus {
    Unavailable(String),
    Restored,
}

struct Registration {
    available: bool,
    announced_available: bool,
    retries: usize,
    next_retry: Option<Instant>,
}

impl Registration {
    const RETRY_SECONDS: [u64; 8] = [1, 2, 4, 8, 16, 30, 30, 30];

    fn new(available: bool, now: Instant) -> Self {
        let mut state = Self {
            available,
            announced_available: true,
            retries: 0,
            next_retry: None,
        };
        state.reset(available, now);
        state
    }

    fn reset(&mut self, available: bool, now: Instant) {
        self.available = available;
        self.retries = 0;
        self.next_retry = (!available).then(|| now + Duration::from_secs(Self::RETRY_SECONDS[0]));
    }

    fn next_retry_delay(&self, now: Instant) -> Option<Duration> {
        self.next_retry
            .map(|deadline| deadline.saturating_duration_since(now))
    }

    fn maintain(&mut self, now: Instant, register: impl FnOnce() -> bool) -> Option<TrayStatus> {
        if self.next_retry.is_some_and(|retry| now >= retry) {
            self.available = register();
            self.retries += 1;
            self.next_retry = if self.available {
                None
            } else {
                Self::RETRY_SECONDS
                    .get(self.retries)
                    .map(|delay| now + Duration::from_secs(*delay))
            };
        }
        if self.available == self.announced_available {
            return None;
        }
        self.announced_available = self.available;
        Some(if self.available {
            TrayStatus::Restored
        } else {
            TrayStatus::Unavailable(
                "The notification-area icon is unavailable. The window will remain open while registration is retried for up to three minutes. Restart Monitor Switcher if the icon does not return."
                    .into(),
            )
        })
    }
}

pub struct Tray {
    hwnd: HWND,
    registration: Registration,
    pub events: Receiver<TrayEvent>,
}
impl Tray {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let window = match cc.window_handle().map_err(|e| e.to_string())?.as_raw() {
            RawWindowHandle::Win32(window) => window.hwnd.get(),
            _ => return Err("The notification area requires a native Windows window.".into()),
        };
        unsafe {
            if native_state().is_some() {
                return Err("The notification-area window is already initialised.".into());
            }
            let _ = TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));
            let module = GetModuleHandleW(ptr::null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: module,
                lpszClassName: w!("MonitorSwitcherTray"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                0,
                class.lpszClassName,
                w!("Monitor Switcher"),
                0,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                module,
                ptr::null(),
            );
            if hwnd.is_null() {
                return Err("Could not create the notification-area window.".into());
            }
            let (tx, rx) = mpsc::channel();
            *NATIVE_STATE
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = Some(NativeState {
                sender: tx,
                context: cc.egui_ctx.clone(),
                main_window: window,
                owner_window: hwnd as isize,
            });
            let available = register_icon(hwnd);
            ICON_AVAILABLE.store(available, Ordering::Release);
            REGISTRATION_RESET.store(false, Ordering::Release);
            Ok(Self {
                hwnd,
                registration: Registration::new(available, Instant::now()),
                events: rx,
            })
        }
    }

    pub fn is_available(&self) -> bool {
        ICON_AVAILABLE.load(Ordering::Acquire)
    }

    pub fn next_retry_delay(&self) -> Option<Duration> {
        self.registration.next_retry_delay(Instant::now())
    }

    pub fn maintain(&mut self) -> Option<TrayStatus> {
        let now = Instant::now();
        if REGISTRATION_RESET.swap(false, Ordering::AcqRel) {
            self.registration.reset(self.is_available(), now);
        }
        let status = self.registration.maintain(now, || register_icon(self.hwnd));
        ICON_AVAILABLE.store(self.registration.available, Ordering::Release);
        status
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
        let mut state = NATIVE_STATE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if state
            .as_ref()
            .is_some_and(|state| state.owner_window == self.hwnd as isize)
        {
            *state = None;
            ICON_AVAILABLE.store(false, Ordering::Release);
            REGISTRATION_RESET.store(false, Ordering::Release);
        }
        drop(state);
        unsafe {
            let data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                ..Default::default()
            };
            Shell_NotifyIconW(NIM_DELETE, &data);
            DestroyWindow(self.hwnd);
        }
    }
}

pub struct Instance(HANDLE);
impl Instance {
    pub fn acquire() -> Result<Option<Self>, String> {
        unsafe {
            let handle = CreateMutexW(ptr::null(), 0, w!("Local\\MonitorSwitcherDesktop"));
            if handle.is_null() {
                return Err("Could not acquire the application instance lock.".into());
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(handle);
                let window = FindWindowW(w!("MonitorSwitcherTray"), w!("Monitor Switcher"));
                if !window.is_null() && PostMessageW(window, SHOW, 0, 0) != 0 {
                    return Ok(None);
                }
                return Err(
                    "Monitor Switcher is already running. Open its notification-area icon.".into(),
                );
            }
            Ok(Some(Self(handle)))
        }
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_initial_registration_is_reported_once_before_retrying() {
        let now = Instant::now();
        let mut registration = Registration::new(false, now);
        assert!(matches!(
            registration.maintain(now, || panic!("Registration retried too soon.")),
            Some(TrayStatus::Unavailable(_))
        ));
        assert!(
            registration
                .maintain(now + Duration::from_millis(999), || {
                    panic!("Registration retried too soon.")
                })
                .is_none()
        );
        assert!(
            registration
                .maintain(now + Duration::from_secs(1), || false)
                .is_none()
        );
        assert!(!registration.available);
    }

    #[test]
    fn retry_deadlines_do_not_require_continuous_ui_polling() {
        let now = Instant::now();
        let mut registration = Registration::new(false, now);
        assert_eq!(
            registration.next_retry_delay(now),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            registration.next_retry_delay(now + Duration::from_secs(2)),
            Some(Duration::ZERO)
        );
        registration.maintain(now + Duration::from_secs(2), || true);
        assert_eq!(
            registration.next_retry_delay(now + Duration::from_secs(2)),
            None
        );
    }

    #[test]
    fn retries_back_off_and_stop_after_eight_attempts() {
        let mut now = Instant::now();
        let mut registration = Registration::new(false, now);
        let _ = registration.maintain(now, || false);
        let mut attempts = 0;
        for delay in Registration::RETRY_SECONDS {
            now += Duration::from_secs(delay);
            assert!(
                registration
                    .maintain(now - Duration::from_millis(1), || {
                        panic!("Registration retried before its backoff elapsed.")
                    })
                    .is_none()
            );
            assert!(
                registration
                    .maintain(now, || {
                        attempts += 1;
                        false
                    })
                    .is_none()
            );
        }
        assert_eq!(attempts, 8);
        assert!(registration.next_retry.is_none());
        assert!(
            registration
                .maintain(now + Duration::from_secs(3_600), || {
                    panic!("Registration retried after its bounded attempts finished.")
                })
                .is_none()
        );
    }

    #[test]
    fn successful_recovery_is_reported_once_and_stops_retrying() {
        let now = Instant::now();
        let mut registration = Registration::new(false, now);
        let _ = registration.maintain(now, || false);
        assert_eq!(
            registration.maintain(now + Duration::from_secs(1), || true),
            Some(TrayStatus::Restored)
        );
        assert!(registration.available);
        assert!(
            registration
                .maintain(now + Duration::from_secs(3_600), || {
                    panic!("An available icon must not be registered again.")
                })
                .is_none()
        );
    }

    #[test]
    fn explorer_restart_starts_a_new_retry_episode() {
        let now = Instant::now();
        let mut registration = Registration::new(true, now);
        assert!(
            registration
                .maintain(now, || panic!("An available icon must not be retried."))
                .is_none()
        );
        registration.reset(false, now);
        assert!(matches!(
            registration.maintain(now, || false),
            Some(TrayStatus::Unavailable(_))
        ));
        for _ in Registration::RETRY_SECONDS {
            let due = registration.next_retry.unwrap();
            assert!(registration.maintain(due, || false).is_none());
        }
        assert!(registration.next_retry.is_none());
        let restarted = now + Duration::from_secs(300);
        registration.reset(false, restarted);
        assert!(registration.maintain(restarted, || false).is_none());
        assert_eq!(
            registration.maintain(restarted + Duration::from_secs(1), || true),
            Some(TrayStatus::Restored)
        );
    }
}
