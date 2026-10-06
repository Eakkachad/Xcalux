//! Pen, mouse and display settings (shared by every tool's property panel).

use super::curve_editor;
use super::property::percent;
use super::{fill_slider, section};
use crate::shell::{PerfMode, Shell};
use crate::studio::{DisplaySync, Studio};
use crate::text::{Key, t};

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio, shell: &mut Shell) {
    // Bench screenshots (ARTY_BENCH_SETTINGS) open it whatever the saved state says.
    let open = crate::bench::settings_open().then_some(true);
    egui::CollapsingHeader::new(t(Key::PenSettingsInputDisplay)).default_open(false).open(open).show(ui, |ui| {
        section(ui, t(Key::SectionPenPressure));
        curve_editor::ui(ui, &mut studio.input.pressure_curve);
        ui.add_space(6.0);
        // Above the grid: these labels or controls are wider than its two columns allow in a narrow panel.
        ui.horizontal_wrapped(|ui| {
            ui.label(t(Key::LanguageLabel));
            super::lang_switch(ui, shell);
        });
        ui.checkbox(&mut shell.home_at_start, t(Key::HomeShowAtStartup));
        let label = ui.label(t(Key::PerfLabel));
        if open.is_some() {
            label.scroll_to_me(Some(egui::Align::Center));
        }
        perf_switch(ui, shell);
        ui.add_space(6.0);
        egui::Grid::new("input-settings").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label(t(Key::PenSettingsMousePressure));
            fill_slider(ui, percent(&mut studio.input.mouse_pressure));
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

/// [Auto (Light) | Light | Full]: Auto names what it came to on this machine.
fn perf_switch(ui: &mut egui::Ui, shell: &mut Shell) {
    let resolved = if PerfMode::Auto.light(shell.machine.tier()) { Key::PerfLight } else { Key::PerfFull };
    let auto = format!("{} ({})", t(Key::PerfAuto), t(resolved));
    let options = [(PerfMode::Auto, auto.as_str()), (PerfMode::Light, t(Key::PerfLight)), (PerfMode::Full, t(Key::PerfFull))];
    let pal = shell.theme.palette();
    if let Some(mode) = ui.horizontal(|ui| super::segmented(ui, &pal, shell.perf, &options, t(Key::PerfTip))).inner {
        shell.set_perf(mode);
    }
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
