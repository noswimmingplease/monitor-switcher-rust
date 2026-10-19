//! Shared, opaque colours for the application and native tray menu.
use eframe::egui::{self, Color32, Stroke};

#[derive(Clone, Copy)]
pub struct Palette {
    pub background: Color32,
    pub surface: Color32,
    pub control: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub input: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub selected_card: Color32,
    pub success: Color32,
    pub success_fill: Color32,
    pub warning: Color32,
    pub error: Color32,
}

impl Palette {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                background: Color32::from_rgb(19, 22, 31),
                surface: Color32::from_rgb(30, 35, 48),
                control: Color32::from_rgb(43, 50, 66),
                hover: Color32::from_rgb(55, 65, 87),
                pressed: Color32::from_rgb(65, 77, 102),
                input: Color32::from_rgb(17, 23, 34),
                border: Color32::from_rgb(76, 86, 107),
                text: Color32::from_rgb(236, 239, 248),
                muted: Color32::from_rgb(171, 181, 203),
                accent: Color32::from_rgb(86, 102, 214),
                selected_card: Color32::from_rgb(39, 48, 78),
                success: Color32::from_rgb(131, 218, 177),
                success_fill: Color32::from_rgb(23, 60, 47),
                warning: Color32::from_rgb(245, 197, 99),
                error: Color32::from_rgb(255, 180, 171),
            }
        } else {
            Self {
                background: Color32::from_rgb(245, 247, 251),
                surface: Color32::WHITE,
                control: Color32::from_rgb(233, 238, 245),
                hover: Color32::from_rgb(221, 230, 246),
                pressed: Color32::from_rgb(207, 220, 241),
                input: Color32::WHITE,
                border: Color32::from_rgb(181, 192, 209),
                text: Color32::from_rgb(32, 38, 52),
                muted: Color32::from_rgb(87, 101, 122),
                accent: Color32::from_rgb(36, 87, 198),
                selected_card: Color32::from_rgb(231, 239, 255),
                success: Color32::from_rgb(23, 99, 68),
                success_fill: Color32::from_rgb(226, 243, 233),
                warning: Color32::from_rgb(136, 81, 0),
                error: Color32::from_rgb(164, 38, 44),
            }
        }
    }
}

pub fn visuals(dark: bool) -> egui::Visuals {
    let p = Palette::new(dark);
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    // Widget-specific foregrounds must remain available for selected controls.
    visuals.override_text_color = None;
    visuals.weak_text_color = Some(p.muted);
    visuals.panel_fill = p.background;
    visuals.window_fill = p.surface;
    visuals.window_stroke = Stroke::new(1.0_f32, p.border);
    visuals.extreme_bg_color = p.input;
    visuals.text_edit_bg_color = Some(p.input);
    visuals.faint_bg_color = p.control;
    visuals.code_bg_color = p.control;
    visuals.selection.bg_fill = p.accent;
    visuals.selection.stroke = Stroke::new(1.5_f32, Color32::WHITE);
    visuals.hyperlink_color = if dark {
        Color32::from_rgb(151, 183, 255)
    } else {
        p.accent
    };
    visuals.warn_fg_color = p.warning;
    visuals.error_fg_color = p.error;
    for (widget, fill) in [
        (&mut visuals.widgets.noninteractive, p.surface),
        (&mut visuals.widgets.inactive, p.control),
        (&mut visuals.widgets.hovered, p.hover),
        (&mut visuals.widgets.active, p.pressed),
        (&mut visuals.widgets.open, p.selected_card),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = Stroke::new(1.0_f32, p.border);
        widget.fg_stroke = Stroke::new(1.5_f32, p.text);
    }
    visuals
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contrast(a: Color32, b: Color32) -> f64 {
        let luminance = |c: Color32| {
            let linear = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    #[test]
    fn selected_text_and_status_colours_have_readable_contrast_in_both_themes() {
        for dark in [true, false] {
            let p = Palette::new(dark);
            for (foreground, background) in [
                (Color32::WHITE, p.accent),
                (p.text, p.surface),
                (p.text, p.selected_card),
                (p.muted, p.background),
                (p.success, p.success_fill),
                (p.error, p.background),
                (p.warning, p.background),
            ] {
                assert!(
                    contrast(foreground, background) >= 4.5,
                    "Insufficient contrast: {foreground:?} on {background:?}"
                );
            }
            let v = visuals(dark);
            assert_eq!(v.selection.stroke.color, Color32::WHITE);
            assert_eq!(v.panel_fill.a(), 255);
            assert!(v.override_text_color.is_none());
        }
    }
}
