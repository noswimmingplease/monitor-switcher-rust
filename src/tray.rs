use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    ptr,
    sync::{
        OnceLock,
        mpsc::{self, Receiver},
    },
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
static SENDER: OnceLock<mpsc::Sender<TrayEvent>> = OnceLock::new();
static CONTEXT: OnceLock<eframe::egui::Context> = OnceLock::new();
static MAIN_WINDOW: OnceLock<isize> = OnceLock::new();
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();
const CALLBACK: u32 = WM_APP + 17;
const SHOW: u32 = WM_APP + 18;

fn send(event: TrayEvent) {
    let focus = matches!(
        &event,
        TrayEvent::Show | TrayEvent::Settings | TrayEvent::Diagnostics
    );
    if let Some(tx) = SENDER.get() {
        let _ = tx.send(event);
    }
    // A hidden Win32 window does not receive redraws reliably. Make it visible
    // natively before asking egui to process the queued tray event.
    if let Some(window) = MAIN_WINDOW.get() {
        let window = *window as HWND;
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
    }
    if let Some(ctx) = CONTEXT.get() {
        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Minimized(false));
        ctx.request_repaint();
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
        unsafe {
            Shell_NotifyIconW(NIM_ADD, &icon_data(hwnd));
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

pub struct Tray {
    hwnd: HWND,
    pub events: Receiver<TrayEvent>,
}
impl Tray {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let window = match cc.window_handle().map_err(|e| e.to_string())?.as_raw() {
            RawWindowHandle::Win32(window) => window.hwnd.get(),
            _ => return Err("The notification area requires a native Windows window.".into()),
        };
        unsafe {
            let _ = CONTEXT.set(cc.egui_ctx.clone());
            let _ = MAIN_WINDOW.set(window);
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
            let _ = SENDER.set(tx);
            let data = icon_data(hwnd);
            if Shell_NotifyIconW(NIM_ADD, &data) == 0 {
                DestroyWindow(hwnd);
                return Err("Notification-area icon is unavailable.".into());
            }
            Ok(Self { hwnd, events: rx })
        }
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
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
