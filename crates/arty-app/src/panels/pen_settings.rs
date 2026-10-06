//! Pen, mouse and display settings (shared by every tool's property panel).

use super::curve_editor;
use super::property::percent;
use super::section;
use crate::shell::Shell;
use crate::studio::{DisplaySync, Studio};
use crate::text::{Key, Lang, t};

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell) {
    egui::CollapsingHeader::new(t(Key::PenSettingsInputDisplay)).default_open(false).show(ui, |ui| {
        section(ui, t(Key::SectionPenPressure));
        curve_editor::ui(ui, &mut studio.input.pressure_curve);
        ui.add_space(6.0);
        egui::Grid::new("input-settings").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label(t(Key::LanguageLabel));
            language_combo(ui, shell);
            ui.end_row();
            ui.label(t(Key::PenSettingsMousePressure));
            ui.add(percent(&mut studio.input.mouse_pressure));
            ui.end_row();
            ui.label(t(Key::PenSettingsNativePen));
            ui.checkbox(&mut studio.input.native_pen, "").on_hover_text(t(Key::PenSettingsNativePenTip));
            ui.end_row();
            ui.label(t(Key::PenSettingsEraserEnd));
            ui.checkbox(&mut studio.input.eraser_end_switch, "").on_hover_text(t(Key::PenSettingsEraserEndTip));
            ui.end_row();
            ui.label(t(Key::PenSettingsDisplaySync));
            display_sync_combo(ui, studio);
            ui.end_row();
            ui.label(t(Key::PenSettingsLatencyOverlay));
            ui.checkbox(&mut studio.input.show_latency, "")
                .on_hover_text(t(Key::PenSettingsLatencyOverlayTip));
            ui.end_row();
        });
    });
}

fn language_combo(ui: &mut egui::Ui, shell: &mut Shell) {
    let current = shell.lang;
    egui::ComboBox::from_id_salt("language-switch")
        .selected_text(current.name())
        .show_ui(ui, |ui| {
            for l in Lang::ALL {
                if ui.selectable_label(current == l, l.name()).clicked() {
                    shell.set_lang(l);
                }
            }
        });
}

fn sync_hover(s: DisplaySync) -> &'static str {
    match s {
        DisplaySync::Smooth => t(Key::SyncSmoothTip),
        DisplaySync::LowLatency => t(Key::SyncLowLatencyTip),
        DisplaySync::FastVsync => t(Key::SyncFastVsyncTip),
        DisplaySync::Off => t(Key::SyncOffTip),
    }
}

fn sync_label(s: DisplaySync) -> &'static str {
    match s {
        DisplaySync::Smooth => t(Key::DisplaySyncSmooth),
        DisplaySync::LowLatency => t(Key::DisplaySyncLowLatency),
        DisplaySync::FastVsync => t(Key::DisplaySyncFastVsync),
        DisplaySync::Off => t(Key::DisplaySyncOff),
    }
}

fn sync_text(s: DisplaySync) -> String {
    if s == DisplaySync::default() {
        format!("{} {}", sync_label(s), t(Key::CommonDefaultSuffix))
    } else {
        sync_label(s).to_owned()
    }
}

fn display_sync_combo(ui: &mut egui::Ui, studio: &mut Studio) {
    let current = &mut studio.input.display_sync;
    egui::ComboBox::from_id_salt("display-sync").selected_text(sync_text(*current)).show_ui(ui, |ui| {
        for s in DisplaySync::ALL {
            let enabled = s != DisplaySync::FastVsync || studio.fast_vsync_ok;
            let r = ui.add_enabled(enabled, egui::Button::selectable(*current == s, sync_text(s)));
            let r = if enabled { r.on_hover_text(sync_hover(s)) } else { r.on_disabled_hover_text(t(Key::SyncNeedsDx12)) };
            if r.clicked() {
                *current = s;
            }
        }
    })
    .response
    .on_hover_text(applies(*current));
}

/// When the choice takes effect (main.rs: the window library applies it only at start-up).
fn applies(s: DisplaySync) -> String {
    match s {
        DisplaySync::FastVsync => format!("{}. {}", sync_hover(s), t(Key::SyncNotAppliedPrefix)),
        _ => format!("{}. {}", sync_hover(s), t(Key::SyncAfterRestartPrefix)),
    }
}
