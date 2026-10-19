//! Native, keyboard-accessible popup menus with explicit light/dark rendering.
//! No undocumented Windows theme APIs are used.
use crate::{model::*, tray::TrayEvent};
use std::{collections::BTreeSet, mem::size_of, ptr, sync::Mutex};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    UI::{Controls::*, HiDpi::*, WindowsAndMessaging::*},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preference {
    Dark,
    AlwaysOnTop,
    StartWithWindows,
    ConfirmDisabling,
    ApplyOnStart,
}

#[derive(Clone, Default)]
pub struct State {
    pub settings: Settings,
    pub active: BTreeSet<String>,
    pub reliable: bool,
    pub busy: bool,
    pub storage_ok: bool,
    pub can_apply: bool,
    pub startup: Option<bool>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub fn update(state: State) {
    *STATE.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(state);
}

struct Item {
    label: String,
    event: Option<TrayEvent>,
    enabled: bool,
    checked: bool,
    children: Vec<Item>,
}

impl Item {
    fn action(label: impl Into<String>, event: TrayEvent, enabled: bool) -> Self {
        Self {
            label: label.into(),
            event: Some(event),
            enabled,
            checked: false,
            children: vec![],
        }
    }
    fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
    fn separator() -> Self {
        Self {
            label: String::new(),
            event: None,
            enabled: false,
            checked: false,
            children: vec![],
        }
    }
}

fn items(state: &State) -> Vec<Item> {
    let editable = state.storage_ok;
    let monitors_editable = editable && !state.busy;
    let profiles = state
        .settings
        .profiles
        .iter()
        .map(|profile| {
            let current = state.reliable && profile.targets == state.active;
            Item::action(
                if current {
                    format!("{} (current)", profile.name)
                } else {
                    profile.name.clone()
                },
                TrayEvent::SelectProfile(profile.name.clone()),
                monitors_editable,
            )
            .checked(profile.name == state.settings.selected)
        })
        .collect::<Vec<_>>();
    let mut result = vec![
        Item::action("Open Monitor Switcher", TrayEvent::Show, true),
        Item {
            label: "Profiles".into(),
            event: None,
            enabled: !profiles.is_empty(),
            checked: false,
            children: profiles,
        },
        Item::action("Apply selected profile", TrayEvent::Apply, state.can_apply),
        Item::action(
            "Save current set…",
            TrayEvent::SaveProfile,
            monitors_editable && state.reliable,
        ),
        Item::action("Refresh monitors", TrayEvent::Refresh, !state.busy),
        Item::separator(),
        Item::action("Settings", TrayEvent::Settings, true),
        Item::action("Diagnostics", TrayEvent::Diagnostics, true),
        Item::separator(),
    ];
    for (label, preference, checked, enabled) in [
        (
            "Start with Windows",
            Preference::StartWithWindows,
            state.startup.unwrap_or(false),
            editable && state.startup.is_some(),
        ),
        (
            "Dark appearance",
            Preference::Dark,
            state.settings.preferences.dark,
            editable,
        ),
        (
            "Always on top",
            Preference::AlwaysOnTop,
            state.settings.preferences.always_on_top,
            editable,
        ),
        (
            "Confirm before disabling",
            Preference::ConfirmDisabling,
            state.settings.preferences.confirm_disabling,
            editable,
        ),
        (
            "Apply profile on app start",
            Preference::ApplyOnStart,
            state.settings.preferences.apply_on_start,
            editable,
        ),
    ] {
        result.push(Item::action(label, TrayEvent::Toggle(preference), enabled).checked(checked));
    }
    result.push(Item::separator());
    result.push(Item::action("Exit", TrayEvent::Exit, true));
    result
}

#[derive(Clone, Copy)]
struct Palette {
    background: COLORREF,
    text: COLORREF,
    muted: COLORREF,
    highlight: COLORREF,
    line: COLORREF,
}
const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}
impl Palette {
    fn new(dark: bool) -> Self {
        let p = crate::theme::Palette::new(dark);
        let colour = |c: eframe::egui::Color32| rgb(c.r(), c.g(), c.b());
        Self {
            background: colour(p.surface),
            text: colour(p.text),
            muted: colour(p.muted),
            highlight: colour(p.selected_card),
            line: colour(p.border),
        }
    }
}

struct Row {
    label: Vec<u16>,
    checked: bool,
    separator: bool,
    submenu: bool,
    palette: Palette,
    font: HFONT,
    dpi: u32,
}
impl Row {
    fn px(&self, value: i32) -> i32 {
        (value as i64 * self.dpi as i64 / 96) as i32
    }
}

struct Menu {
    root: HMENU,
    // Boxes keep the item-data pointers stable until DestroyMenu has completed.
    rows: Vec<Box<Row>>,
    events: Vec<Option<TrayEvent>>,
    font: HFONT,
    background: HBRUSH,
    palette: Palette,
    dpi: u32,
}

impl Drop for Menu {
    fn drop(&mut self) {
        unsafe {
            if !self.root.is_null() {
                DestroyMenu(self.root);
            }
            if !self.font.is_null() {
                DeleteObject(self.font);
            }
            if !self.background.is_null() {
                DeleteObject(self.background);
            }
        }
    }
}

impl Menu {
    fn new(hwnd: HWND, state: &State) -> AppResult<Self> {
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let palette = Palette::new(state.settings.preferences.dark);
        let mut metrics = NONCLIENTMETRICSW::default();
        metrics.cbSize = size_of::<NONCLIENTMETRICSW>() as u32;
        let font = unsafe {
            if SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS,
                metrics.cbSize,
                (&mut metrics as *mut NONCLIENTMETRICSW).cast(),
                0,
                dpi,
            ) == 0
            {
                return Err("Windows could not provide the tray menu font.".into());
            }
            CreateFontIndirectW(&metrics.lfMenuFont)
        };
        let mut menu = Self {
            root: ptr::null_mut(),
            rows: vec![],
            events: vec![None],
            font,
            background: unsafe { CreateSolidBrush(palette.background) },
            palette,
            dpi,
        };
        if menu.font.is_null() || menu.background.is_null() {
            return Err("Windows could not create the tray menu resources.".into());
        }
        menu.root = menu.create_popup()?;
        menu.append(menu.root, items(state))?;
        Ok(menu)
    }
    fn create_popup(&self) -> AppResult<HMENU> {
        let popup = unsafe { CreatePopupMenu() };
        if popup.is_null() {
            return Err("Windows could not create the tray menu.".into());
        }
        let info = MENUINFO {
            cbSize: size_of::<MENUINFO>() as u32,
            fMask: MIM_BACKGROUND | MIM_STYLE,
            hbrBack: self.background,
            dwStyle: MNS_NOCHECK,
            ..Default::default()
        };
        if unsafe { SetMenuInfo(popup, &info) } == 0 {
            unsafe {
                DestroyMenu(popup);
            }
            return Err("Windows could not style the tray menu.".into());
        }
        Ok(popup)
    }
    fn append(&mut self, popup: HMENU, items: Vec<Item>) -> AppResult<()> {
        for item in items {
            let submenu = if item.children.is_empty() {
                ptr::null_mut()
            } else {
                self.create_popup()?
            };
            let id = u32::try_from(self.events.len()).map_err(|_| "Too many tray menu items.")?;
            self.events
                .push(if item.enabled { item.event } else { None });
            let separator = item.label.is_empty();
            let mut row = Box::new(Row {
                label: item.label.encode_utf16().chain(Some(0)).collect(),
                checked: item.checked,
                separator,
                submenu: !submenu.is_null(),
                palette: self.palette,
                font: self.font,
                dpi: self.dpi,
            });
            let info = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE | MIIM_STATE | MIIM_ID | MIIM_DATA | MIIM_STRING | MIIM_SUBMENU,
                fType: MFT_OWNERDRAW,
                fState: (if item.enabled {
                    MFS_ENABLED
                } else {
                    MFS_DISABLED
                }) | if item.checked {
                    MFS_CHECKED
                } else {
                    MFS_UNCHECKED
                },
                wID: id,
                hSubMenu: submenu,
                dwItemData: (&*row as *const Row) as usize,
                dwTypeData: row.label.as_mut_ptr(),
                cch: row.label.len().saturating_sub(1) as u32,
                ..Default::default()
            };
            if unsafe { InsertMenuItemW(popup, u32::MAX, 1, &info) } == 0 {
                if !submenu.is_null() {
                    unsafe {
                        DestroyMenu(submenu);
                    }
                }
                return Err("Windows could not add a tray menu item.".into());
            }
            self.rows.push(row);
            if !submenu.is_null() {
                self.append(submenu, item.children)?;
            }
        }
        Ok(())
    }
}

pub fn show(hwnd: HWND) -> AppResult<Option<TrayEvent>> {
    let state = STATE
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
        .unwrap_or_default();
    let mut point = POINT::default();
    unsafe {
        GetCursorPos(&mut point);
        // Move only the invisible menu owner so DPI measurement follows the
        // monitor whose notification area was clicked.
        SetWindowPos(
            hwnd,
            ptr::null_mut(),
            point.x,
            point.y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    let menu = Menu::new(hwnd, &state)?;
    let choice = unsafe {
        SetForegroundWindow(hwnd);
        let choice = TrackPopupMenu(
            menu.root,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            hwnd,
            ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        choice
    };
    Ok(menu.events.get(choice as usize).cloned().flatten())
}

// Windows sends these messages only while Menu owns the boxed Row pointers.
pub unsafe fn measure(hwnd: HWND, lp: LPARAM) -> bool {
    let info = unsafe { &mut *(lp as *mut MEASUREITEMSTRUCT) };
    if info.CtlType != ODT_MENU || info.itemData == 0 {
        return false;
    }
    let row = unsafe { &*(info.itemData as *const Row) };
    let dc = unsafe { GetDC(hwnd) };
    if dc.is_null() {
        return false;
    }
    unsafe {
        let previous = SelectObject(dc, row.font);
        let mut size = SIZE::default();
        GetTextExtentPoint32W(
            dc,
            row.label.as_ptr(),
            row.label.len().saturating_sub(1) as i32,
            &mut size,
        );
        SelectObject(dc, previous);
        ReleaseDC(hwnd, dc);
        info.itemWidth = (size.cx + row.px(64)).max(row.px(160)) as u32;
        info.itemHeight = if row.separator {
            row.px(9)
        } else {
            size.cy + row.px(14)
        } as u32;
    }
    true
}

pub unsafe fn draw(lp: LPARAM) -> bool {
    let info = unsafe { &*(lp as *const DRAWITEMSTRUCT) };
    if info.CtlType != ODT_MENU || info.itemData == 0 {
        return false;
    }
    let row = unsafe { &*(info.itemData as *const Row) };
    let disabled = info.itemState & (ODS_DISABLED | ODS_GRAYED) != 0;
    let highlighted = !disabled && info.itemState & ODS_SELECTED != 0;
    unsafe {
        let saved = SaveDC(info.hDC);
        if saved == 0 {
            return false;
        }
        let brush = CreateSolidBrush(if highlighted {
            row.palette.highlight
        } else {
            row.palette.background
        });
        if !brush.is_null() {
            FillRect(info.hDC, &info.rcItem, brush);
            DeleteObject(brush);
        }
        if row.separator {
            let line = CreateSolidBrush(row.palette.line);
            if !line.is_null() {
                let mut rect = info.rcItem;
                rect.left += row.px(12);
                rect.right -= row.px(12);
                rect.top = (rect.top + rect.bottom) / 2;
                rect.bottom = rect.top + 1;
                FillRect(info.hDC, &rect, line);
                DeleteObject(line);
            }
        } else {
            SelectObject(info.hDC, row.font);
            SetBkMode(info.hDC, TRANSPARENT as i32);
            SetTextColor(
                info.hDC,
                if disabled {
                    row.palette.muted
                } else {
                    row.palette.text
                },
            );
            let mut text = info.rcItem;
            text.left += row.px(32);
            text.right -= row.px(24);
            DrawTextW(
                info.hDC,
                row.label.as_ptr(),
                row.label.len().saturating_sub(1) as i32,
                &mut text,
                DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
            );
            if row.checked {
                let check: Vec<_> = "✓".encode_utf16().collect();
                let mut rect = info.rcItem;
                rect.left += row.px(9);
                rect.right = rect.left + row.px(20);
                DrawTextW(
                    info.hDC,
                    check.as_ptr(),
                    check.len() as i32,
                    &mut rect,
                    DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                );
            }
            if row.submenu {
                let arrow: Vec<_> = "›".encode_utf16().collect();
                let mut rect = info.rcItem;
                rect.left = rect.right - row.px(19);
                DrawTextW(
                    info.hDC,
                    arrow.as_ptr(),
                    arrow.len() as i32,
                    &mut rect,
                    DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                );
            }
        }
        if saved != 0 {
            RestoreDC(info.hDC, saved);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_menu_retains_accessible_text_and_draws_both_themes() {
        for dark in [true, false] {
            let mut state = State {
                storage_ok: true,
                ..Default::default()
            };
            state.settings.preferences.dark = dark;
            let menu = Menu::new(ptr::null_mut(), &state).unwrap();
            let mut text = [0_u16; 100];
            let mut info = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STRING | MIIM_DATA,
                dwTypeData: text.as_mut_ptr(),
                cch: text.len() as u32,
                ..Default::default()
            };
            assert_ne!(unsafe { GetMenuItemInfoW(menu.root, 0, 1, &mut info) }, 0);
            assert_eq!(
                String::from_utf16(&text[..info.cch as usize]).unwrap(),
                "Open Monitor Switcher"
            );
            let mut measurement = MEASUREITEMSTRUCT {
                CtlType: ODT_MENU,
                itemData: info.dwItemData,
                ..Default::default()
            };
            assert!(unsafe {
                measure(
                    ptr::null_mut(),
                    (&mut measurement as *mut MEASUREITEMSTRUCT) as LPARAM,
                )
            });
            assert!(measurement.itemWidth >= 160);
            assert!(measurement.itemHeight >= 20);
            unsafe {
                let screen = GetDC(ptr::null_mut());
                assert!(!screen.is_null());
                let dc = CreateCompatibleDC(screen);
                let bitmap = CreateCompatibleBitmap(screen, 400, 80);
                ReleaseDC(ptr::null_mut(), screen);
                assert!(!dc.is_null() && !bitmap.is_null());
                let old = SelectObject(dc, bitmap);
                let drawing = DRAWITEMSTRUCT {
                    CtlType: ODT_MENU,
                    hDC: dc,
                    rcItem: RECT {
                        left: 0,
                        top: 0,
                        right: 400,
                        bottom: 80,
                    },
                    itemData: info.dwItemData,
                    ..Default::default()
                };
                let drawn = draw((&drawing as *const DRAWITEMSTRUCT) as LPARAM);
                let colour = GetPixel(dc, 1, 1);
                SelectObject(dc, old);
                DeleteObject(bitmap);
                DeleteDC(dc);
                assert!(drawn);
                assert_eq!(colour, Palette::new(dark).background);
            }
        }
    }
    #[test]
    fn profiles_are_not_limited_to_three_and_current_selection_is_marked() {
        let path = r"\\?\DISPLAY#MODEL#A#{GUID}".to_owned();
        let mut state = State {
            reliable: true,
            storage_ok: true,
            active: BTreeSet::from([path.clone()]),
            ..Default::default()
        };
        state.settings.selected = "Profile 17".into();
        state.settings.profiles = (0..40)
            .map(|i| Profile {
                name: format!("Profile {i}"),
                targets: BTreeSet::from([path.clone()]),
            })
            .collect();
        let menu = items(&state);
        assert_eq!(menu[1].children.len(), 40);
        assert!(menu[1].children[17].checked);
        assert_eq!(
            menu[1].children.iter().filter(|item| item.checked).count(),
            1
        );
        assert!(menu[1].children[17].label.ends_with("(current)"));
        assert!(!menu[2].enabled);
        state.busy = true;
        assert!(items(&state)[1].children.iter().all(|item| !item.enabled));
    }
    #[test]
    fn theme_and_startup_choices_follow_saved_state() {
        assert_ne!(
            Palette::new(true).background,
            Palette::new(false).background
        );
        let mut state = State {
            storage_ok: true,
            startup: Some(true),
            ..Default::default()
        };
        let startup = |state: &State| {
            items(state)
                .into_iter()
                .find(|item| {
                    matches!(
                        item.event,
                        Some(TrayEvent::Toggle(Preference::StartWithWindows))
                    )
                })
                .unwrap()
        };
        assert!(startup(&state).checked);
        state.startup = None;
        assert!(!startup(&state).enabled);
    }
    #[test]
    fn background_scans_do_not_disable_preference_commands() {
        let state = State {
            busy: true,
            storage_ok: true,
            startup: Some(false),
            ..Default::default()
        };
        let toggles: Vec<_> = items(&state)
            .into_iter()
            .filter(|item| matches!(item.event, Some(TrayEvent::Toggle(_))))
            .collect();
        assert_eq!(toggles.len(), 5);
        assert!(toggles.iter().all(|item| item.enabled));
    }
}
