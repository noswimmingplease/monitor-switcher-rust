#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod display;
mod model;
mod startup;
mod store;
mod theme;
mod tray;
mod tray_menu;

use eframe::egui::{self, Color32, RichText, Vec2};
use model::*;
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

const MONITOR_SCAN_INTERVAL: Duration = Duration::from_secs(4);
const MIN_BACKGROUND_WAKE: Duration = Duration::from_millis(50);

fn primary_button(ui: &egui::Ui, label: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(label).color(Color32::WHITE))
        .fill(ui.visuals().selection.bg_fill)
}

#[derive(Debug, PartialEq, Eq)]
enum CloseAction {
    Hide,
    Wait,
    Exit,
}

fn close_action(tray_enabled: bool, busy: bool) -> CloseAction {
    if tray_enabled {
        CloseAction::Hide
    } else if busy {
        CloseAction::Wait
    } else {
        CloseAction::Exit
    }
}

fn monitor_information(
    ui: &mut egui::Ui,
    monitor: &Monitor,
) -> egui::collapsing_header::CollapsingResponse<()> {
    egui::CollapsingHeader::new("Monitor information")
        .id_salt(("monitor-information", &monitor.path))
        .show(ui, |ui| {
            ui.label(&monitor.name);
            ui.add(egui::Label::new(RichText::new(&monitor.path).monospace().small()).wrap());
            if let Some(g) = &monitor.geometry {
                ui.label(format!("Position {}, {}", g.x, g.y));
            }
        })
}

fn monitor_alias_editor(
    ui: &mut egui::Ui,
    path: &str,
    alias: &mut String,
    width: f32,
) -> egui::Response {
    ui.add_sized(
        [width, 26.0],
        egui::TextEdit::singleline(alias)
            .id_salt(("monitor-alias", path))
            .desired_width(width),
    )
}

fn move_monitor(monitors: &mut Vec<Monitor>, from: &str, to: &str) -> bool {
    let Some(from_index) = monitors.iter().position(|monitor| monitor.path == from) else {
        return false;
    };
    let Some(to_index) = monitors.iter().position(|monitor| monitor.path == to) else {
        return false;
    };
    if from_index == to_index {
        return false;
    }
    let monitor = monitors.remove(from_index);
    monitors.insert(to_index, monitor);
    true
}

enum WorkerEvent {
    Detected(AppResult<Vec<Monitor>>),
    Applied(display::ApplyOutcome),
}
struct Application {
    settings: Settings,
    settings_draft: Settings,
    dir: PathBuf,
    monitors: Vec<Monitor>,
    reliable: bool,
    busy: bool,
    quarantined: bool,
    rx: mpsc::Receiver<WorkerEvent>,
    tx: mpsc::Sender<WorkerEvent>,
    status: String,
    error: bool,
    status_expires: Option<Instant>,
    tab: usize,
    last_scan: Instant,
    save_dialog: bool,
    save_name: String,
    pending: Option<BTreeSet<String>>,
    pending_name: String,
    delete_profile: Option<String>,
    tray: Option<tray::Tray>,
    exit: bool,
    close_when_done: bool,
    startup_pending: bool,
    storage_ok: bool,
    startup_choice: bool,
    startup_registered: Option<bool>,
    live_preferences: Option<(bool, bool)>,
    display_off_due: Option<Instant>,
}

impl Application {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let dir = store::data_dir();
        let loaded = store::load(&dir);
        let storage_ok = loaded.is_ok();
        let (settings, status) = loaded.unwrap_or_else(|e| (Settings::default(), e));
        let startup_pending = settings.preferences.apply_on_start;
        let startup_state = startup::registered();
        let startup_choice = startup_state
            .as_ref()
            .copied()
            .unwrap_or(settings.preferences.start_with_windows);
        let settings_draft = settings.clone();
        let startup_registered = startup_state.as_ref().ok().copied();
        let (tray, tray_error) = match tray::Tray::new(cc) {
            Ok(tray) => (Some(tray), None),
            Err(error) => (None, Some(error)),
        };
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            settings,
            settings_draft,
            dir,
            monitors: vec![],
            reliable: false,
            busy: false,
            quarantined: false,
            rx,
            tx,
            status,
            error: !storage_ok,
            status_expires: Some(
                Instant::now() + Duration::from_secs(if storage_ok { 5 } else { 15 }),
            ),
            tab: 0,
            last_scan: Instant::now(),
            save_dialog: false,
            save_name: String::new(),
            pending: None,
            pending_name: String::new(),
            delete_profile: None,
            tray,
            exit: false,
            close_when_done: false,
            startup_pending,
            storage_ok,
            startup_choice,
            startup_registered,
            live_preferences: None,
            display_off_due: None,
        };
        if let Err(error) = startup_state {
            app.report(error, true);
        }
        if let Some(error) = tray_error {
            app.report(
                format!("{error} The window will remain available; closing it exits the app."),
                true,
            );
        }
        let mut style = (*cc.egui_ctx.global_style()).clone();
        style.spacing.item_spacing = Vec2::new(10.0, 10.0);
        style.spacing.button_padding = Vec2::new(13.0, 8.0);
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        for dark in [true, false] {
            let mut style = style.clone();
            style.visuals = theme::visuals(dark);
            cc.egui_ctx.set_style_of(
                if dark {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                },
                style,
            );
        }
        app.theme(&cc.egui_ctx);
        app.scan(&cc.egui_ctx);
        app
    }
    fn theme(&self, ctx: &egui::Context) {
        Self::set_theme(ctx, self.settings.preferences.dark);
    }
    fn set_theme(ctx: &egui::Context, dark: bool) {
        ctx.set_theme(if dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
        ctx.set_visuals(theme::visuals(dark));
        ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(if dark {
            egui::SystemTheme::Dark
        } else {
            egui::SystemTheme::Light
        }));
        ctx.request_repaint();
    }
    fn sync_live_preferences(&mut self, ctx: &egui::Context) {
        let preferences = if self.tab == 2 {
            &self.settings_draft.preferences
        } else {
            &self.settings.preferences
        };
        let next = (preferences.dark, preferences.always_on_top);
        if self.live_preferences.map(|previous| previous.0) != Some(next.0)
            || ctx.global_style().visuals.dark_mode != next.0
        {
            Self::set_theme(ctx, next.0);
        }
        if self.live_preferences.map(|previous| previous.1) != Some(next.1) {
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(if next.1 {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            }));
            ctx.request_repaint();
        }
        self.live_preferences = Some(next);
    }
    fn tray_available(&self) -> bool {
        self.tray.as_ref().is_some_and(tray::Tray::is_available)
    }
    fn maintain_tray(&mut self) {
        match self.tray.as_mut().and_then(tray::Tray::maintain) {
            Some(tray::TrayStatus::Unavailable(error)) => self.report(error, true),
            Some(tray::TrayStatus::Restored) => {
                self.report("Notification-area icon restored.", false);
            }
            None => {}
        }
    }
    fn background_wake_delay(&self) -> Option<Duration> {
        if self.exit {
            return None;
        }
        let scan = (!self.busy && !self.modal_active() && !self.close_when_done)
            .then(|| MONITOR_SCAN_INTERVAL.saturating_sub(self.last_scan.elapsed()));
        let retry = self.tray.as_ref().and_then(tray::Tray::next_retry_delay);
        scan.into_iter()
            .chain(retry)
            .chain(
                self.status_expires
                    .map(|deadline| deadline.saturating_duration_since(Instant::now())),
            )
            .chain(self.display_off_due.map(|deadline| {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining > Duration::from_secs(1) {
                    remaining - Duration::from_secs(1)
                } else {
                    remaining
                }
            }))
            .min()
            // egui may subtract a predicted frame interval from repaint delays.
            // Never repeatedly request an immediate wake just before a deadline.
            .map(|delay| delay.max(MIN_BACKGROUND_WAKE))
    }
    fn schedule_background_wake(&self, ctx: &egui::Context) {
        if let Some(delay) = self.background_wake_delay() {
            ctx.request_repaint_after(delay);
        }
    }
    fn background_logic(&mut self, ctx: &egui::Context) {
        self.maintain_tray();
        self.events(ctx);
        if self
            .status_expires
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.status.clear();
            self.error = false;
            self.status_expires = None;
            ctx.request_repaint();
        }
        self.sync_live_preferences(ctx);
        if ctx.input(|input| input.viewport().close_requested()) && !self.exit {
            match close_action(
                self.settings.preferences.minimise_to_tray && self.tray_available(),
                self.busy,
            ) {
                CloseAction::Hide => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                CloseAction::Wait => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.close_when_done = true;
                }
                CloseAction::Exit => {}
            }
        }
        if !self.exit
            && !self.close_when_done
            && !self.busy
            && !self.modal_active()
            && self.last_scan.elapsed() >= MONITOR_SCAN_INTERVAL
        {
            self.scan(ctx);
        }
        self.publish_tray_state();
        self.schedule_background_wake(ctx);
    }
    fn report(&mut self, message: impl Into<String>, error: bool) {
        self.status = message.into();
        self.error = error;
        self.status_expires = (!self.status.is_empty())
            .then(|| Instant::now() + Duration::from_secs(if error { 15 } else { 5 }));
        store::log(&self.dir, &self.status);
    }
    fn save_settings(&mut self, next: Settings) -> bool {
        if !self.storage_ok {
            return false;
        }
        if let Err(e) = store::save(&self.dir, &next) {
            self.report(format!("Could not save settings: {e}"), true);
            return false;
        }
        self.settings = next;
        true
    }
    fn save_preferences(&mut self, next: Settings) -> bool {
        if !self.storage_ok {
            return false;
        }
        let enabled = next.preferences.start_with_windows;
        match next
            .validate()
            .and_then(|_| startup::save(enabled, || store::save(&self.dir, &next)))
        {
            Ok(()) => {
                self.settings = next;
                self.startup_registered = Some(enabled);
                true
            }
            Err(error) => {
                self.report(format!("Could not save settings: {error}"), true);
                false
            }
        }
    }
    fn toggle_preference(&mut self, preference: tray_menu::Preference) {
        use tray_menu::Preference;
        let mut next = self.settings.clone();
        match preference {
            Preference::Dark => next.preferences.dark = !next.preferences.dark,
            Preference::AlwaysOnTop => {
                next.preferences.always_on_top = !next.preferences.always_on_top
            }
            Preference::ConfirmDisabling => {
                next.preferences.confirm_disabling = !next.preferences.confirm_disabling
            }
            Preference::ApplyOnStart => {
                next.preferences.apply_on_start = !next.preferences.apply_on_start
            }
            Preference::StartWithWindows => match startup::registered() {
                Ok(enabled) => next.preferences.start_with_windows = !enabled,
                Err(error) => {
                    self.startup_registered = None;
                    self.report(error, true);
                    return;
                }
            },
        }
        let saved = if preference == Preference::StartWithWindows {
            self.save_preferences(next)
        } else {
            self.save_settings(next)
        };
        if saved {
            let saved = &self.settings.preferences;
            let draft = &mut self.settings_draft.preferences;
            match preference {
                Preference::Dark => draft.dark = saved.dark,
                Preference::AlwaysOnTop => draft.always_on_top = saved.always_on_top,
                Preference::ConfirmDisabling => draft.confirm_disabling = saved.confirm_disabling,
                Preference::ApplyOnStart => draft.apply_on_start = saved.apply_on_start,
                Preference::StartWithWindows => {
                    draft.start_with_windows = saved.start_with_windows;
                    self.startup_choice = saved.start_with_windows;
                }
            }
            self.report("Settings saved.", false);
        }
    }
    fn scan(&mut self, ctx: &egui::Context) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.last_scan = Instant::now();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(display::detect)
                .unwrap_or_else(|_| Err("Monitor detection failed unexpectedly.".into()));
            let _ = tx.send(WorkerEvent::Detected(result));
            ctx.request_repaint();
        });
    }
    fn can_apply(&self) -> bool {
        !self.busy
            && !self.modal_active()
            && self.reliable
            && !self.quarantined
            && self.storage_ok
            && self
                .settings
                .profile()
                .is_some_and(|p| p.targets != active_set(&self.monitors))
    }
    fn modal_active(&self) -> bool {
        self.save_dialog || self.pending.is_some() || self.delete_profile.is_some()
    }
    fn delete_saved_profile(&mut self, name: &str) -> bool {
        if !self
            .settings
            .profiles
            .iter()
            .any(|profile| profile.name == name)
        {
            self.report("This profile no longer exists.", true);
            return false;
        }
        let mut next = self.settings.clone();
        next.profiles.retain(|profile| profile.name != name);
        if next.selected == name {
            next.selected = next
                .profiles
                .first()
                .map(|profile| profile.name.clone())
                .unwrap_or_else(|| "Default".into());
        }
        self.save_settings(next)
    }
    fn request(&mut self, targets: BTreeSet<String>, name: String, ctx: &egui::Context) {
        if self.busy || !self.reliable || self.quarantined || !self.storage_ok {
            return;
        }
        if targets == active_set(&self.monitors) {
            self.report("Current profile", false);
            return;
        }
        self.startup_pending = false;
        let disabling = !active_set(&self.monitors).is_subset(&targets);
        if self.settings.preferences.confirm_disabling && disabling {
            self.pending = Some(targets);
            self.pending_name = name;
        } else {
            self.apply(targets, name, ctx);
        }
    }
    fn apply(&mut self, targets: BTreeSet<String>, name: String, ctx: &egui::Context) {
        if self.busy || self.quarantined {
            return;
        }
        self.busy = true;
        self.pending_name = name;
        self.report("Applying monitor profile…", false);
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let fallback = self.settings.fallback.clone();
        std::thread::spawn(move || {
            let result=std::panic::catch_unwind(||display::apply(&targets,fallback.as_deref())).unwrap_or_else(|_|display::ApplyOutcome{
                message:"The display worker failed unexpectedly. Further changes are locked; check Windows Display Settings.".into(),quarantined:true,success:false});
            let _ = tx.send(WorkerEvent::Applied(result));
            ctx.request_repaint();
        });
    }
    fn events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.rx.try_recv() {
            self.busy = false;
            match event {
                WorkerEvent::Detected(Ok(monitors)) => {
                    self.monitors = monitors;
                    self.reliable = true;
                    self.monitors.sort_by_key(|m| {
                        self.settings
                            .order
                            .iter()
                            .position(|p| p == &m.path)
                            .unwrap_or(usize::MAX)
                    });
                    if self.startup_pending {
                        self.startup_pending = false;
                        if self.can_apply() {
                            let p = self.settings.profile().unwrap().clone();
                            self.request(p.targets, p.name, ctx);
                        }
                    }
                }
                WorkerEvent::Detected(Err(e)) => {
                    self.reliable = false;
                    self.report(e, true);
                }
                WorkerEvent::Applied(result) => {
                    self.quarantined |= result.quarantined;
                    self.reliable = false;
                    self.report(
                        if result.success {
                            format!("{} applied.", self.pending_name)
                        } else {
                            result.message
                        },
                        !result.success,
                    );
                    self.scan(ctx);
                }
            }
        }
        let events: Vec<_> = self
            .tray
            .as_ref()
            .map(|t| t.events.try_iter().collect())
            .unwrap_or_default();
        for e in events {
            self.handle_tray_event(e, ctx);
        }
        if self.close_when_done && !self.busy {
            self.exit = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    fn handle_tray_event(&mut self, event: tray::TrayEvent, ctx: &egui::Context) {
        // Native menus can already have queued an action when a modal opens.
        // Guard the handler as well as the menu's displayed enabled state.
        if self.modal_active()
            && matches!(
                &event,
                tray::TrayEvent::Apply
                    | tray::TrayEvent::SelectProfile(_)
                    | tray::TrayEvent::SaveProfile
                    | tray::TrayEvent::Refresh
            )
        {
            return;
        }
        match event {
            tray::TrayEvent::Show => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            tray::TrayEvent::Apply => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                if self.can_apply() {
                    let p = self.settings.profile().unwrap().clone();
                    self.request(p.targets, p.name, ctx);
                }
            }
            tray::TrayEvent::SelectProfile(name) => {
                if !self.busy
                    && self.storage_ok
                    && self
                        .settings
                        .profiles
                        .iter()
                        .any(|profile| profile.name == name)
                {
                    let mut next = self.settings.clone();
                    next.selected = name;
                    self.startup_pending = false;
                    if self.save_settings(next) {
                        self.status.clear();
                    }
                }
            }
            tray::TrayEvent::SaveProfile => {
                if self.reliable && !self.busy && self.storage_ok {
                    self.save_name = self.settings.selected.clone();
                    self.save_dialog = true;
                }
            }
            tray::TrayEvent::Refresh => self.scan(ctx),
            tray::TrayEvent::Settings => self.tab = 2,
            tray::TrayEvent::Diagnostics => self.tab = 3,
            tray::TrayEvent::Toggle(preference) => {
                // Preferences do not mutate the display transaction. Do not
                // drop a menu command when a background scan starts.
                self.toggle_preference(preference);
            }
            tray::TrayEvent::Failure(error) => self.report(error, true),
            tray::TrayEvent::Exit => {
                if self.busy {
                    self.close_when_done = true;
                    self.report(
                        "Finishing the current display operation before exiting…",
                        false,
                    );
                } else {
                    self.exit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        self.sync_live_preferences(ctx);
        self.publish_tray_state();
        ctx.request_repaint();
    }
    fn publish_tray_state(&self) {
        tray_menu::update(tray_menu::State {
            settings: self.settings.clone(),
            active: active_set(&self.monitors),
            reliable: self.reliable,
            busy: self.busy,
            modal_active: self.modal_active(),
            storage_ok: self.storage_ok,
            can_apply: self.can_apply(),
            startup: self.startup_registered,
        });
    }
    fn profile_strip(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("MONITOR PROFILE")
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
            let mut selected = self.settings.selected.clone();
            ui.add_enabled_ui(!self.busy, |ui| {
                egui::ComboBox::from_id_salt("profile")
                    .width(180.0)
                    .selected_text(&selected)
                    .show_ui(ui, |ui| {
                        for p in &self.settings.profiles {
                            ui.selectable_value(&mut selected, p.name.clone(), &p.name);
                        }
                    });
            });
            if self.settings.selected != selected {
                self.startup_pending = false;
                let mut next = self.settings.clone();
                next.selected = selected;
                if self.save_settings(next) {
                    self.status.clear();
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(self.can_apply(), primary_button(ui, "Apply profile"))
                    .clicked()
                {
                    let p = self.settings.profile().unwrap().clone();
                    self.request(p.targets, p.name, ctx);
                }
            });
        });
        let text = if self.busy {
            "Checking displays…"
        } else if !self.reliable {
            "Detection unavailable · Refresh to retry"
        } else if self
            .settings
            .profile()
            .is_some_and(|p| p.targets == active_set(&self.monitors))
        {
            "Current profile"
        } else {
            "Click Apply to use this profile"
        };
        ui.label(
            RichText::new(text)
                .small()
                .color(ui.visuals().weak_text_color()),
        );
    }
    fn topology(&self, ui: &mut egui::Ui) {
        let palette = theme::Palette::new(ui.visuals().dark_mode);
        let active: Vec<_> = self
            .monitors
            .iter()
            .filter_map(|m| m.geometry.as_ref().map(|g| (m, g)))
            .collect();
        if active.is_empty() {
            return;
        }
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 80.0), egui::Sense::hover());
        let minx = active.iter().map(|(_, g)| i64::from(g.x)).min().unwrap();
        let miny = active.iter().map(|(_, g)| i64::from(g.y)).min().unwrap();
        let maxx = active
            .iter()
            .map(|(_, g)| i64::from(g.x) + i64::from(g.effective_size().0))
            .max()
            .unwrap();
        let maxy = active
            .iter()
            .map(|(_, g)| i64::from(g.y) + i64::from(g.effective_size().1))
            .max()
            .unwrap();
        let scale = ((rect.width() - 16.0) / (maxx - minx).max(1) as f32)
            .min((rect.height() - 16.0) / (maxy - miny).max(1) as f32);
        let origin =
            rect.center() - Vec2::new((maxx - minx) as f32, (maxy - miny) as f32) * scale / 2.0;
        for (m, g) in active {
            let pos = origin
                + Vec2::new(
                    (i64::from(g.x) - minx) as f32,
                    (i64::from(g.y) - miny) as f32,
                ) * scale;
            let r = egui::Rect::from_min_size(
                pos,
                Vec2::new(g.effective_size().0 as f32, g.effective_size().1 as f32) * scale,
            )
            .shrink(2.0);
            let primary = g.x == 0 && g.y == 0;
            ui.painter().rect_filled(
                r,
                7.0,
                if primary {
                    palette.selected_card
                } else {
                    ui.visuals().widgets.inactive.bg_fill
                },
            );
            ui.painter().rect_stroke(
                r,
                7.0,
                egui::Stroke::new(
                    1.0_f32,
                    if primary {
                        palette.accent
                    } else {
                        ui.visuals().weak_text_color()
                    },
                ),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                m.number(),
                egui::FontId::proportional(22.0),
                ui.visuals().text_color(),
            );
        }
    }
    fn monitors(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = theme::Palette::new(ui.visuals().dark_mode);
        self.topology(ui);
        ui.horizontal(|ui| {
            ui.heading("Your monitors");
            ui.label(
                RichText::new(format!(
                    "{} of {} active",
                    active_set(&self.monitors).len(),
                    self.monitors.len()
                ))
                .small()
                .weak(),
            );
        });
        ui.label(
            RichText::new(
                "Drag the handle to reorder this list. Arrange displays in Windows Settings.",
            )
            .small()
            .weak(),
        );
        let mut moved = None;
        for m in self.monitors.clone() {
            let response = egui::Frame::new()
                .fill(palette.surface)
                .corner_radius(10.0)
                .inner_margin(11.0)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.horizontal(|ui| {
                        let drag = ui.add_enabled(
                            !self.busy,
                            egui::Label::new(RichText::new("☰").size(21.0).weak())
                                .sense(egui::Sense::drag()),
                        );
                        drag.clone().on_hover_cursor(egui::CursorIcon::Grab);
                        if drag.drag_started() {
                            drag.dnd_set_drag_payload(m.path.clone());
                        }
                        if drag.dragged() {
                            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                        }
                        ui.vertical(|ui| {
                            ui.label(RichText::new(self.settings.title(&m)).strong().size(17.0));
                            ui.label(
                                RichText::new(if m.active {
                                    format!("Display {} · {}", m.number(), m.name)
                                } else {
                                    format!("{} · Connected, disabled", m.name)
                                })
                                .small()
                                .weak(),
                            );
                            if let Some(g) = &m.geometry {
                                ui.label(
                                    RichText::new(format!(
                                        "{} × {} · {}{}",
                                        g.effective_size().0,
                                        g.effective_size().1,
                                        match g.rotation {
                                            2 => "Portrait",
                                            3 => "Landscape flipped",
                                            4 => "Portrait flipped",
                                            _ => "Landscape",
                                        },
                                        if g.x == 0 && g.y == 0 {
                                            " · Primary"
                                        } else {
                                            ""
                                        }
                                    ))
                                    .small()
                                    .weak(),
                                );
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let can_change = self.reliable
                                && !self.busy
                                && !self.quarantined
                                && self.storage_ok
                                && (!m.active || active_set(&self.monitors).len() > 1);
                            if ui
                                .add_enabled(
                                    can_change,
                                    egui::Button::new(if m.active { "Disable" } else { "Enable" }),
                                )
                                .clicked()
                            {
                                match desired_for_toggle(&self.monitors, &m.path) {
                                    Ok(set) => self.request(
                                        set,
                                        format!("Monitor '{}'", self.settings.title(&m)),
                                        ctx,
                                    ),
                                    Err(e) => self.report(e, true),
                                }
                            }
                            ui.label(
                                RichText::new(if m.active { "Active" } else { "Disabled" })
                                    .small()
                                    .color(if m.active {
                                        palette.success
                                    } else {
                                        palette.warning
                                    }),
                            );
                        });
                    });
                    monitor_information(ui, &m);
                })
                .response;
            if let Some(p) = response.dnd_hover_payload::<String>() {
                if *p != m.path {
                    ui.painter().rect_stroke(
                        response.rect,
                        10.0,
                        egui::Stroke::new(2.0_f32, palette.accent),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            if let Some(p) = response.dnd_release_payload::<String>() {
                if *p != m.path {
                    moved = Some((p.as_ref().clone(), m.path));
                }
            }
        }
        if let Some((from, to)) = moved {
            let previous = self.monitors.clone();
            if move_monitor(&mut self.monitors, &from, &to) {
                let mut next = self.settings.clone();
                next.order = self.monitors.iter().map(|m| m.path.clone()).collect();
                if !self.save_settings(next) {
                    self.monitors = previous;
                }
            }
        }
    }
    fn profiles_ui(&mut self, ui: &mut egui::Ui) {
        let palette = theme::Palette::new(ui.visuals().dark_mode);
        ui.horizontal(|ui| {
            ui.heading("Saved profiles");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{} saved", self.settings.profiles.len()))
                        .small()
                        .weak(),
                );
            });
        });
        ui.label(
            RichText::new("Select a profile to review its enabled monitors, then click Apply.")
                .small()
                .weak(),
        );
        ui.add_space(6.0);
        if self.settings.profiles.is_empty() {
            ui.label("No profiles saved yet. Save your current monitor set to create one.");
        }
        let active = active_set(&self.monitors);
        let mut selection = None;
        for profile in &self.settings.profiles {
            let selected = profile.name == self.settings.selected;
            let current = self.reliable && profile.targets == active;
            let border = if selected {
                egui::Stroke::new(1.0_f32, palette.accent)
            } else {
                egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color)
            };
            egui::Frame::new()
                .fill(if selected {
                    palette.selected_card
                } else {
                    palette.surface
                })
                .stroke(border)
                .corner_radius(10.0)
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&profile.name).size(18.0).strong());
                            ui.label(
                                RichText::new(format!(
                                    "{} {} enabled",
                                    profile.targets.len(),
                                    if profile.targets.len() == 1 {
                                        "monitor"
                                    } else {
                                        "monitors"
                                    }
                                ))
                                .small()
                                .weak(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if (!selected || !current)
                                && ui
                                    .add_enabled(
                                        !selected && !self.busy && self.storage_ok,
                                        egui::Button::new(if selected {
                                            "Selected"
                                        } else {
                                            "Select profile"
                                        }),
                                    )
                                    .clicked()
                            {
                                selection = Some(profile.name.clone());
                            }
                            if current {
                                ui.add(
                                    egui::Button::new(
                                        RichText::new("Current").color(palette.success),
                                    )
                                    .fill(palette.success_fill)
                                    .stroke(egui::Stroke::NONE)
                                    .corner_radius(6.0)
                                    .sense(egui::Sense::hover()),
                                );
                            }
                        });
                    });
                    ui.add_space(5.0);
                    let mut targets: Vec<_> = profile.targets.iter().collect();
                    targets.sort_by_key(|p| {
                        self.settings
                            .order
                            .iter()
                            .position(|k| k == *p)
                            .unwrap_or(usize::MAX)
                    });
                    ui.horizontal_wrapped(|ui| {
                        for target in targets {
                            let connected = self.monitors.iter().find(|m| &m.path == target);
                            let name = connected
                                .map(|m| self.settings.title(m))
                                .or_else(|| self.settings.aliases.get(target).cloned())
                                .unwrap_or_else(|| "Unavailable display".into());
                            let unavailable = self.reliable && connected.is_none();
                            egui::Frame::new()
                                .fill(ui.visuals().extreme_bg_color)
                                .corner_radius(6.0)
                                .inner_margin(egui::Margin::symmetric(10, 6))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(if unavailable {
                                            format!("{name} · disconnected")
                                        } else {
                                            name
                                        })
                                        .small(),
                                    );
                                });
                        }
                    });
                });
            ui.add_space(3.0);
        }
        if let Some(name) = selection {
            let mut next = self.settings.clone();
            next.selected = name;
            self.startup_pending = false;
            if self.save_settings(next) {
                self.status.clear();
            }
        }
    }
    fn settings_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Settings");
        ui.add_space(8.0);
        let old = self.settings_draft.preferences.clone();
        ui.checkbox(&mut self.settings_draft.preferences.dark, "Dark appearance");
        ui.checkbox(
            &mut self.settings_draft.preferences.always_on_top,
            "Always on top",
        );
        ui.add_enabled(
            self.tray_available(),
            egui::Checkbox::new(
                &mut self.settings_draft.preferences.minimise_to_tray,
                "Close to notification area",
            ),
        );
        ui.checkbox(&mut self.startup_choice, "Start with Windows");
        ui.checkbox(
            &mut self.settings_draft.preferences.confirm_disabling,
            "Confirm before disabling monitors",
        );
        ui.checkbox(
            &mut self.settings_draft.preferences.apply_on_start,
            "Apply selected profile when this app opens",
        );
        if old.dark != self.settings_draft.preferences.dark {
            Self::set_theme(ctx, self.settings_draft.preferences.dark);
        }
        ui.separator();
        ui.heading("Monitor names");
        let font = egui::TextStyle::Body.resolve(ui.style());
        let label_width = self
            .monitors
            .iter()
            .map(|m| {
                ui.painter()
                    .layout_no_wrap(m.name.clone(), font.clone(), ui.visuals().text_color())
                    .size()
                    .x
            })
            .fold(80.0_f32, f32::max)
            .min(180.0);
        let field_width = (ui.available_width() - label_width - 12.0).max(100.0);
        egui::Grid::new("monitor-name-fields")
            .num_columns(2)
            .spacing(Vec2::new(12.0, 10.0))
            .show(ui, |ui| {
                for m in &self.monitors {
                    ui.add_sized([label_width, 26.0], egui::Label::new(&m.name).truncate());
                    monitor_alias_editor(
                        ui,
                        &m.path,
                        self.settings_draft
                            .aliases
                            .entry(m.path.clone())
                            .or_default(),
                        field_width,
                    );
                    ui.end_row();
                }
            });
        ui.add_space(10.0);
        ui.label("Fallback primary when the current primary is disabled");
        egui::ComboBox::from_id_salt("fallback")
            .selected_text(
                self.settings_draft
                    .fallback
                    .as_ref()
                    .and_then(|p| self.monitors.iter().find(|m| &m.path == p))
                    .map(|m| self.settings.title(m))
                    .unwrap_or_else(|| "Windows chooses".into()),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.settings_draft.fallback, None, "Windows chooses");
                for m in &self.monitors {
                    let title = self.settings.title(m);
                    ui.selectable_value(
                        &mut self.settings_draft.fallback,
                        Some(m.path.clone()),
                        title,
                    );
                }
            });
        ui.add_space(14.0);
        if ui
            .add_enabled(self.storage_ok, egui::Button::new("Save settings"))
            .clicked()
        {
            let mut next = self.settings.clone();
            next.preferences = self.settings_draft.preferences.clone();
            next.preferences.start_with_windows = self.startup_choice;
            next.aliases = self.settings_draft.aliases.clone();
            next.fallback = self.settings_draft.fallback.clone();
            if self.save_preferences(next) {
                self.settings_draft = self.settings.clone();
                self.report("Settings saved.", false);
            }
        }
        ui.separator();
        ui.label(
            RichText::new(concat!(
                "Monitor Switcher ",
                env!("CARGO_PKG_VERSION"),
                " · Windows x64"
            ))
            .weak(),
        );
        ui.label("All application assets, fonts and runtime code are embedded. Windows system libraries and your graphics driver are supplied by Windows.");
        ui.hyperlink_to(
            "Source repository",
            "https://github.com/noswimmingplease/monitor-switcher-rust",
        );
        ui.collapsing("Open-source licences", |ui| {
            ui.label(
                "GPL-3.0-only · Original Monitor Switcher icon © 2026 Ci303 / noswimmingplease",
            );
            ui.label(
                "The GPL and complete dependency licence bundle are embedded in this executable.",
            );
            if ui.button("Copy all licence texts").clicked() {
                ctx.copy_text(format!(
                    "{}\n\n{}",
                    include_str!("../LICENSE"),
                    include_str!("../assets/THIRD-PARTY-LICENCES.txt")
                ));
            }
        });
    }
    fn status_bar(&self, ui: &mut egui::Ui) {
        let fill = ui.visuals().panel_fill;
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(fill).inner_margin(16.0))
            .show(ui, |ui| {
                if self.busy { ui.horizontal(|ui| { ui.spinner(); ui.label("Working…"); }); }
                if let Some(deadline) = self.display_off_due {
                    let seconds = if deadline.saturating_duration_since(Instant::now()) > Duration::from_secs(1) { 2 } else { 1 };
                    ui.label(format!("Turning off displays in {seconds}…"));
                }
                if !self.status.is_empty() {
                    ui.label(RichText::new(&self.status).color(if self.error {
                        ui.visuals().error_fg_color
                    } else { ui.visuals().text_color() }));
                }
                if self.quarantined {
                    ui.label(RichText::new("Display changes locked · Check Windows Display Settings before restarting.").color(ui.visuals().warn_fg_color));
                }
            });
    }
    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.save_dialog {
            egui::Modal::new(egui::Id::new("save_profile")).show(ctx, |ui| {
                ui.heading("Save enabled monitors");
                ui.label("The profile stores which monitors are enabled. Windows controls their arrangement.");
                ui.add(egui::TextEdit::singleline(&mut self.save_name).char_limit(100));
                let exists = self.settings.profiles.iter().any(|p| p.name.eq_ignore_ascii_case(self.save_name.trim()));
                ui.horizontal(|ui| {
                    if ui.add_enabled(profile_name_valid(self.save_name.trim()) && self.reliable && !self.busy && self.storage_ok, primary_button(ui, if exists { "Replace profile" } else { "Save profile" })).clicked() {
                        match self.settings.with_profile(&self.save_name, active_set(&self.monitors)) {
                            Ok(next) => {
                                if self.save_settings(next) {
                                    self.save_dialog = false;
                                    self.report("Profile saved.", false);
                                }
                            }
                            Err(error) => self.report(error, true),
                        }
                    }
                    if ui.button("Cancel").clicked() { self.save_dialog = false; }
                });
            });
        }
        if let Some(targets) = self.pending.clone() {
            egui::Modal::new(egui::Id::new("confirm_apply")).show(ctx, |ui| {
                ui.heading("Apply monitor profile?");
                ui.label("This will change which monitors are enabled.");
                for m in &self.monitors {
                    let enabled = targets.contains(&m.path);
                    if enabled != m.active {
                        ui.label(format!(
                            "{}: {}",
                            if enabled { "Enable" } else { "Disable" },
                            self.settings.title(m)
                        ));
                    }
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !self.busy && self.reliable && !self.quarantined && self.storage_ok,
                            egui::Button::new("Apply"),
                        )
                        .clicked()
                    {
                        self.pending = None;
                        self.apply(targets.clone(), self.pending_name.clone(), ctx);
                    }
                    if ui.button("Cancel").clicked() {
                        self.pending = None;
                    }
                });
            });
        }
        if let Some(name) = self.delete_profile.clone() {
            egui::Modal::new(egui::Id::new("delete_profile")).show(ctx, |ui| {
                ui.heading(format!("Delete '{name}'?"));
                ui.label(
                    "This deletes the saved profile. Your Windows display setup is unchanged.",
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            self.storage_ok && !self.busy,
                            egui::Button::new("Delete profile"),
                        )
                        .clicked()
                    {
                        if self.delete_saved_profile(&name) {
                            self.delete_profile = None;
                            self.report("Profile deleted.", false);
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.delete_profile = None;
                    }
                });
            });
        }
    }
}

impl eframe::App for Application {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.background_logic(ctx);
        if self
            .display_off_due
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.display_off_due = None;
            if !self.exit && !self.close_when_done {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    PostMessageW, SC_MONITORPOWER, WM_SYSCOMMAND,
                };
                let posted = match frame.window_handle().map(|handle| handle.as_raw()) {
                    Ok(RawWindowHandle::Win32(window)) => unsafe {
                        PostMessageW(
                            window.hwnd.get() as _,
                            WM_SYSCOMMAND,
                            SC_MONITORPOWER as usize,
                            2,
                        ) != 0
                    },
                    _ => false,
                };
                if !posted {
                    self.report("Unable to turn off the displays.", true);
                    ctx.request_repaint();
                }
            }
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ui.set_style(ctx.global_style());
        egui::Panel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(ctx.global_style().visuals.panel_fill)
                    .inner_margin(12.0),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, label) in ["Monitors", "Profiles", "Settings", "Diagnostics"]
                        .iter()
                        .enumerate()
                    {
                        ui.selectable_value(&mut self.tab, i, *label);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                                .small()
                                .color(if ui.visuals().dark_mode {
                                    ui.visuals().hyperlink_color
                                } else {
                                    ui.visuals().selection.bg_fill
                                }),
                        );
                    });
                });
            });
        self.sync_live_preferences(&ctx);
        self.status_bar(ui);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ctx.global_style().visuals.panel_fill)
                    .inner_margin(16.0),
            )
            .show(ui, |ui| {
                if self.tab <= 1 {
                    self.profile_strip(ui, &ctx);
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!self.busy, egui::Button::new("Refresh"))
                            .clicked()
                        {
                            self.scan(&ctx);
                        }
                        if ui
                            .add_enabled(
                                self.reliable && !self.busy && self.storage_ok,
                                egui::Button::new("Save current set…"),
                            )
                            .clicked()
                        {
                            self.save_name = self.settings.selected.clone();
                            self.save_dialog = true;
                        }
                        if self.tab == 1
                            && ui
                                .add_enabled(
                                    !self.busy
                                        && self.storage_ok
                                        && self.settings.profile().is_some(),
                                    egui::Button::new("Delete profile…"),
                                )
                                .clicked()
                        {
                            self.delete_profile = Some(self.settings.selected.clone());
                        }
                        if self.tab == 0
                            && ui
                                .add_enabled(
                                    !self.busy && self.display_off_due.is_none(),
                                    egui::Button::new("Turn off displays"),
                                )
                                .on_hover_text(
                                    "Move the mouse or press a key to wake the displays.",
                                )
                                .clicked()
                        {
                            self.display_off_due = Some(Instant::now() + Duration::from_secs(2));
                            self.schedule_background_wake(&ctx);
                        }
                    });
                    ui.add_space(6.0);
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.tab {
                        0 => self.monitors(ui, &ctx),
                        1 => self.profiles_ui(ui),
                        2 => self.settings_ui(ui, &ctx),
                        _ => {
                            ui.heading("Diagnostics");
                            ui.horizontal(|ui| {
                                if ui.button("Copy diagnostics").clicked() {
                                    ctx.copy_text(
                                        std::fs::read_to_string(self.dir.join("diagnostics.log"))
                                            .unwrap_or_default(),
                                    );
                                }
                                if ui.button("Clear diagnostics").clicked() {
                                    match std::fs::remove_file(self.dir.join("diagnostics.log")) {
                                        Ok(()) => self.report("Diagnostics cleared.", false),
                                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                                        Err(e) => self.report(e.to_string(), true),
                                    }
                                }
                            });
                            let mut log = std::fs::read_to_string(self.dir.join("diagnostics.log"))
                                .unwrap_or_default();
                            ui.add(
                                egui::TextEdit::multiline(&mut log)
                                    .code_editor()
                                    .desired_width(f32::INFINITY)
                                    .interactive(false),
                            );
                        }
                    });
            });
        self.dialogs(&ctx);
        self.sync_live_preferences(&ctx);
        self.publish_tray_state();
        // A modal may have closed during this UI pass; resume its scan deadline.
        self.schedule_background_wake(&ctx);
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--inspect") {
        // Read-only hardware check. It never imports settings or changes topology.
        let result = display::detect()
            .and_then(|m| serde_json::to_string_pretty(&m).map_err(|e| e.to_string()));
        let output = args
            .iter()
            .position(|a| a == "--output")
            .and_then(|i| args.get(i + 1));
        match result {
            Ok(text) => {
                if let Some(path) = output {
                    if std::fs::write(path, text).is_err() {
                        std::process::exit(2);
                    }
                } else {
                    println!("{text}");
                }
            }
            Err(e) => {
                if let Some(path) = output {
                    let _ = std::fs::write(path, e);
                }
                std::process::exit(1);
            }
        }
        return;
    }
    let _instance = match tray::Instance::acquire() {
        Ok(Some(i)) => i,
        Ok(None) => return,
        Err(e) => {
            show_error(&e);
            return;
        }
    };
    let image = image::load_from_memory_with_format(
        include_bytes!("../assets/app.ico"),
        image::ImageFormat::Ico,
    )
    .expect("Embedded icon")
    .into_rgba8();
    let icon = egui::IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Monitor Switcher")
            .with_inner_size([640.0, 760.0])
            .with_min_inner_size([480.0, 420.0])
            .with_icon(icon),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "Monitor Switcher",
        options,
        Box::new(|cc| Ok(Box::new(Application::new(cc)))),
    ) {
        show_error(&e.to_string());
    }
}

fn show_error(message: &str) {
    let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            windows_sys::core::w!("Monitor Switcher"),
            windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONWARNING,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn application() -> Application {
        let path = r"\\?\DISPLAY#MODEL#A#{GUID}".to_owned();
        let other = r"\\?\DISPLAY#MODEL#B#{GUID}".to_owned();
        let (tx, rx) = mpsc::channel();
        let mut settings = Settings::default();
        settings.selected = "Work".into();
        settings.profiles = vec![
            Profile {
                name: "Work".into(),
                targets: BTreeSet::from([path.clone()]),
            },
            Profile {
                name: "Default".into(),
                targets: BTreeSet::from([path.clone(), other]),
            },
        ];
        Application {
            settings_draft: settings.clone(),
            settings,
            dir: PathBuf::new(),
            monitors: vec![Monitor {
                path,
                name: String::new(),
                device: String::new(),
                active: true,
                geometry: None,
            }],
            reliable: true,
            busy: false,
            quarantined: false,
            rx,
            tx,
            status: String::new(),
            error: false,
            status_expires: None,
            tab: 0,
            last_scan: Instant::now(),
            save_dialog: false,
            save_name: String::new(),
            pending: None,
            pending_name: String::new(),
            delete_profile: None,
            tray: None,
            exit: false,
            close_when_done: false,
            startup_pending: false,
            storage_ok: true,
            startup_choice: false,
            startup_registered: Some(false),
            live_preferences: None,
            display_off_due: None,
        }
    }
    #[test]
    fn apply_availability_tracks_membership_busy_state_and_detection_failures() {
        let mut app = application();
        assert!(!app.can_apply());
        app.settings.selected = "Default".into();
        assert!(app.can_apply());
        app.busy = true;
        assert!(!app.can_apply());
        app.busy = false;
        assert!(app.can_apply());
        app.reliable = false;
        assert!(!app.can_apply());
        app.reliable = true;
        assert!(app.can_apply());
        app.quarantined = true;
        assert!(!app.can_apply());
    }
    #[test]
    fn close_to_tray_does_not_exit_during_a_background_scan() {
        assert_eq!(close_action(true, true), CloseAction::Hide);
        assert_eq!(close_action(true, false), CloseAction::Hide);
        assert_eq!(close_action(false, true), CloseAction::Wait);
        assert_eq!(close_action(false, false), CloseAction::Exit);
    }

    fn hidden_input() -> egui::RawInput {
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .occluded = Some(true);
        input
    }

    #[test]
    fn hidden_logic_drains_worker_results_without_building_a_ui_pass() {
        let mut app = application();
        app.busy = true;
        app.reliable = false;
        app.tx
            .send(WorkerEvent::Detected(Ok(app.monitors.clone())))
            .unwrap();
        let ctx = egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();
        let before = ctx.cumulative_pass_nr();
        let _ = ctx.run_logic(&hidden_input(), |ctx| {
            eframe::App::logic(&mut app, ctx, &mut frame);
        });
        assert!(!app.busy);
        assert!(app.reliable);
        assert_eq!(ctx.cumulative_pass_nr(), before);
        assert_eq!(ctx.input(|input| input.viewport().visible()), Some(false));
    }

    #[test]
    fn hidden_exit_waits_for_the_worker_and_does_not_start_another_scan() {
        let mut app = application();
        app.busy = true;
        app.close_when_done = true;
        app.last_scan = Instant::now() - Duration::from_secs(30);
        let ctx = egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();
        let _ = ctx.run_logic(&hidden_input(), |ctx| {
            eframe::App::logic(&mut app, ctx, &mut frame);
        });
        assert!(!app.exit);
        app.tx
            .send(WorkerEvent::Detected(Ok(app.monitors.clone())))
            .unwrap();
        let output = ctx.run_logic(&hidden_input(), |ctx| {
            eframe::App::logic(&mut app, ctx, &mut frame);
        });
        assert!(app.exit);
        assert!(!app.busy);
        assert_eq!(app.background_wake_delay(), None);
        assert!(
            output.viewport_commands[&egui::ViewportId::ROOT]
                .iter()
                .any(|command| { matches!(command, egui::ViewportCommand::Close) })
        );
    }

    #[test]
    fn status_expiry_does_not_clear_newer_messages() {
        let mut app = application();
        let ctx = egui::Context::default();
        app.status = "Old message".into();
        app.status_expires = Some(Instant::now() - Duration::from_millis(1));
        app.background_logic(&ctx);
        assert!(app.status.is_empty());
        assert!(app.status_expires.is_none());
        app.status = "New error".into();
        app.error = true;
        app.status_expires = Some(Instant::now() + Duration::from_secs(15));
        app.background_logic(&ctx);
        assert_eq!(app.status, "New error");
        assert!(app.error);
    }

    #[test]
    fn display_countdown_schedules_an_intermediate_wake() {
        let mut app = application();
        app.busy = true;
        app.display_off_due = Some(Instant::now() + Duration::from_secs(2));
        let delay = app.background_wake_delay().unwrap();
        assert!(delay <= Duration::from_secs(1));
        assert!(delay > Duration::from_millis(500));
        app.display_off_due = Some(Instant::now() + Duration::from_millis(300));
        let delay = app.background_wake_delay().unwrap();
        assert!(delay <= Duration::from_millis(300));
        assert!(delay >= MIN_BACKGROUND_WAKE);
    }

    #[test]
    fn idle_background_wakes_follow_deadlines_not_continuous_repainting() {
        let mut app = application();
        let delay = app.background_wake_delay().unwrap();
        assert!(delay > Duration::from_secs(2));
        assert!(delay <= MONITOR_SCAN_INTERVAL);
        app.busy = true;
        assert_eq!(app.background_wake_delay(), None);
        app.busy = false;
        app.save_dialog = true;
        assert_eq!(app.background_wake_delay(), None);
        app.save_dialog = false;
        app.last_scan = Instant::now() - MONITOR_SCAN_INTERVAL;
        assert_eq!(app.background_wake_delay(), Some(MIN_BACKGROUND_WAKE));
    }
    #[test]
    fn missing_tray_keeps_the_window_from_being_hidden() {
        let app = application();
        assert!(app.settings.preferences.minimise_to_tray);
        assert!(!app.tray_available());
        assert_eq!(
            close_action(
                app.settings.preferences.minimise_to_tray && app.tray_available(),
                false,
            ),
            CloseAction::Exit,
        );
    }
    #[test]
    fn monitor_reordering_supports_both_directions_and_the_last_slot() {
        let original: Vec<_> = ["A", "B", "C"]
            .into_iter()
            .map(|path| Monitor {
                path: path.into(),
                name: path.into(),
                device: String::new(),
                active: true,
                geometry: None,
            })
            .collect();
        for (from, to, expected) in [
            ("A", "B", ["B", "A", "C"]),
            ("B", "C", ["A", "C", "B"]),
            ("A", "C", ["B", "C", "A"]),
            ("C", "A", ["C", "A", "B"]),
        ] {
            let mut monitors = original.clone();
            assert!(move_monitor(&mut monitors, from, to));
            assert_eq!(
                monitors
                    .iter()
                    .map(|monitor| monitor.path.as_str())
                    .collect::<Vec<_>>(),
                expected,
            );
        }
        let mut monitors = original;
        assert!(!move_monitor(&mut monitors, "A", "missing"));
        assert!(!move_monitor(&mut monitors, "A", "A"));
        assert_eq!(monitors.len(), 3);
    }
    #[test]
    fn tray_actions_cannot_replace_a_pending_modal_operation() {
        let mut app = application();
        let ctx = egui::Context::default();
        for modal in 0..3 {
            app.save_dialog = modal == 0;
            app.pending = (modal == 1).then(BTreeSet::new);
            app.delete_profile = (modal == 2).then(|| "Work".into());
            for event in [
                tray::TrayEvent::SelectProfile("Default".into()),
                tray::TrayEvent::Apply,
                tray::TrayEvent::SaveProfile,
                tray::TrayEvent::Refresh,
            ] {
                app.handle_tray_event(event, &ctx);
            }
            assert_eq!(app.settings.selected, "Work");
            assert!(!app.busy);
            assert_eq!(app.save_dialog, modal == 0);
            assert_eq!(app.pending.is_some(), modal == 1);
            assert_eq!(
                app.delete_profile.as_deref(),
                (modal == 2).then_some("Work")
            );
        }
    }
    #[test]
    fn profile_deletion_uses_the_captured_name_not_the_current_selection() {
        let mut app = application();
        app.dir = std::env::temp_dir().join(format!(
            "monitor-delete-identity-test-{}",
            std::process::id()
        ));
        app.delete_profile = Some("Work".into());
        app.settings.selected = "Default".into();
        let captured_name = app.delete_profile.clone().unwrap();
        assert!(app.delete_saved_profile(&captured_name));
        assert_eq!(app.settings.selected, "Default");
        assert_eq!(app.settings.profiles.len(), 1);
        assert_eq!(app.settings.profiles[0].name, "Default");
        let (saved, _) = store::load(&app.dir).unwrap();
        assert_eq!(saved.profiles[0].name, "Default");
        std::fs::remove_dir_all(&app.dir).unwrap();
    }
    #[test]
    fn monitor_alias_focus_stays_with_the_physical_identity_after_hotplug() {
        let ctx = egui::Context::default();
        let mut aliases = std::collections::BTreeMap::from([
            ("A", "Left".to_owned()),
            ("B", "Middle".to_owned()),
            ("C", "Right".to_owned()),
        ]);
        let render = |rows: &[&'static str],
                      aliases: &mut std::collections::BTreeMap<&'static str, String>,
                      input| {
            let mut ids = std::collections::BTreeMap::new();
            let output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::Grid::new("monitor-name-fields").show(ui, |ui| {
                        for path in rows {
                            ui.label(*path);
                            let response = monitor_alias_editor(
                                ui,
                                path,
                                aliases.get_mut(path).unwrap(),
                                300.0,
                            );
                            ids.insert(*path, response.id);
                            ui.end_row();
                        }
                    });
                });
            });
            output.drop_without_applying_deltas();
            ids
        };
        let initial = render(&["A", "B", "C"], &mut aliases, Default::default());
        ctx.memory_mut(|memory| memory.request_focus(initial["B"]));
        let removed = render(&["B", "C"], &mut aliases, Default::default());
        assert_eq!(initial["B"], removed["B"]);
        assert_eq!(initial["C"], removed["C"]);
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Text("!".into()));
        render(&["B", "C"], &mut aliases, input);
        assert!(aliases["B"].contains('!'));
        assert_eq!(aliases["C"], "Right");
        let reordered = render(&["C", "B"], &mut aliases, Default::default());
        assert_eq!(initial["B"], reordered["B"]);
        assert_eq!(initial["C"], reordered["C"]);
    }
    #[test]
    fn failed_saves_do_not_change_the_selected_or_stored_profiles() {
        let mut app = application();
        // A regular file cannot be used as the settings directory.
        app.dir = std::env::current_exe().unwrap();
        let previous = serde_json::to_string(&app.settings).unwrap();
        let mut next = app.settings.clone();
        next.profiles.clear();
        next.selected = "Default".into();
        assert!(!app.save_settings(next));
        assert_eq!(serde_json::to_string(&app.settings).unwrap(), previous);
        assert!(app.error);
    }
    #[test]
    fn saving_a_profile_selection_does_not_save_unconfirmed_settings() {
        let mut app = application();
        app.settings_draft.preferences.confirm_disabling = false;
        app.settings_draft.preferences.apply_on_start = true;
        app.dir =
            std::env::temp_dir().join(format!("monitor-selection-test-{}", std::process::id()));
        let mut next = app.settings.clone();
        next.selected = "Default".into();
        assert!(app.save_settings(next));
        let (saved, _) = store::load(&app.dir).unwrap();
        assert!(saved.preferences.confirm_disabling);
        assert!(!saved.preferences.apply_on_start);
        std::fs::remove_dir_all(&app.dir).unwrap();
    }
    #[test]
    fn tray_preferences_persist_only_the_requested_setting() {
        let mut app = application();
        app.dir = std::env::temp_dir().join(format!(
            "monitor-tray-preference-test-{}",
            std::process::id()
        ));
        app.settings_draft.preferences.apply_on_start = true;
        app.settings_draft
            .aliases
            .insert("unsaved".into(), "Unsaved alias".into());
        app.toggle_preference(tray_menu::Preference::Dark);
        let (saved, _) = store::load(&app.dir).unwrap();
        assert!(!saved.preferences.dark);
        assert!(!saved.preferences.apply_on_start);
        assert!(saved.aliases.is_empty());
        assert!(!app.settings_draft.preferences.dark);
        assert!(app.settings_draft.preferences.apply_on_start);
        assert!(app.settings_draft.aliases.contains_key("unsaved"));
        std::fs::remove_dir_all(&app.dir).unwrap();
    }
    #[test]
    fn tray_theme_and_window_settings_apply_immediately_during_a_scan() {
        let mut app = application();
        app.dir =
            std::env::temp_dir().join(format!("monitor-live-theme-test-{}", std::process::id()));
        app.tab = 2;
        app.busy = true;
        app.settings_draft
            .aliases
            .insert("pending".into(), "Unsaved".into());
        let ctx = egui::Context::default();
        let output = ctx.run_ui(Default::default(), |_ui| {
            for preference in [
                tray_menu::Preference::Dark,
                tray_menu::Preference::AlwaysOnTop,
                tray_menu::Preference::ConfirmDisabling,
            ] {
                app.handle_tray_event(tray::TrayEvent::Toggle(preference), &ctx);
            }
        });
        assert!(!app.settings.preferences.dark);
        assert!(!app.settings_draft.preferences.dark);
        assert!(!ctx.global_style().visuals.dark_mode);
        assert!(app.settings.preferences.always_on_top);
        assert!(app.settings_draft.preferences.always_on_top);
        assert!(!app.settings.preferences.confirm_disabling);
        assert!(!app.settings_draft.preferences.confirm_disabling);
        assert_eq!(app.live_preferences, Some((false, true)));
        let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
        assert!(commands.iter().any(|command| matches!(
            command,
            egui::ViewportCommand::SetTheme(egui::SystemTheme::Light)
        )));
        assert!(commands.iter().any(|command| matches!(
            command,
            egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop)
        )));
        output.drop_without_applying_deltas();
        let (saved, _) = store::load(&app.dir).unwrap();
        assert!(!saved.preferences.dark);
        assert!(saved.preferences.always_on_top);
        assert!(saved.aliases.is_empty());
        assert!(app.settings_draft.aliases.contains_key("pending"));
        assert!(app.busy);
        std::fs::remove_dir_all(&app.dir).unwrap();
    }
    #[test]
    fn status_bar_and_renderer_background_follow_light_mode() {
        let mut app = application();
        app.settings.preferences.dark = false;
        app.status = "Settings saved.".into();
        let ctx = egui::Context::default();
        Application::set_theme(&ctx, false);
        let output = ctx.run_ui(Default::default(), |ui| app.status_bar(ui));
        let expected = theme::Palette::new(false).background;
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::epaint::Shape::Rect(rect) if rect.fill == expected && rect.fill.a() == 255)));
        assert_eq!(
            eframe::App::clear_color(&app, &ctx.global_style().visuals),
            expected.to_normalized_gamma_f32()
        );
        output.drop_without_applying_deltas();
    }
    #[test]
    fn monitor_information_has_independent_stable_ids_after_reordering() {
        let ctx = egui::Context::default();
        ctx.global_style_mut(|style| style.animation_time = 0.0);
        let mut monitors: Vec<_> = (0..32)
            .map(|i| Monitor {
                path: format!(r"\\?\DISPLAY#MODEL#{i}#{{GUID}}"),
                name: "Same model name".into(),
                device: String::new(),
                active: true,
                geometry: None,
            })
            .collect();
        let render = |monitors: &[Monitor]| {
            let mut headers = Vec::new();
            let output = ctx.run_ui(Default::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    for monitor in monitors {
                        let response = monitor_information(ui, monitor);
                        headers.push((
                            response.header_response.id,
                            response.body_response.is_some(),
                        ));
                    }
                });
            });
            output.drop_without_applying_deltas();
            headers
        };
        let initial = render(&monitors);
        assert_eq!(
            initial
                .iter()
                .map(|(id, _)| *id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            32
        );
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            &ctx,
            initial[0].0,
            false,
        );
        state.set_open(true);
        state.store(&ctx);
        let expanded = render(&monitors);
        assert!(expanded[0].1);
        assert!(expanded[1..].iter().all(|(_, open)| !open));
        monitors.reverse();
        let reordered = render(&monitors);
        assert_eq!(reordered.last().unwrap().0, initial[0].0);
        assert!(reordered.last().unwrap().1);
        assert!(reordered[..31].iter().all(|(_, open)| !open));
    }
}
