//! Pen, mouse and display settings (shared by every tool's property panel).

use super::curve_editor;
use super::property::percent;
use super::section;
use crate::studio::{DisplaySync, Studio};

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio) {
    egui::CollapsingHeader::new("Input & display").default_open(false).show(ui, |ui| {
        section(ui, "PEN PRESSURE");
        curve_editor::ui(ui, &mut studio.input.pressure_curve);
        ui.add_space(6.0);
        egui::Grid::new("input-settings").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label("Mouse pressure");
            ui.add(percent(&mut studio.input.mouse_pressure));
            ui.end_row();
            ui.label("Native pen");
            ui.checkbox(&mut studio.input.native_pen, "").on_hover_text(
                "Read Windows Ink directly: full-rate pressure, tilt and eraser end. Turn off if your tablet driver misbehaves.",
            );
            ui.end_row();
            ui.label("Eraser end");
            ui.checkbox(&mut studio.input.eraser_end_switch, "").on_hover_text("Flipping the pen switches to the eraser end's tool");
            ui.end_row();
            ui.label("Display sync");
            display_sync_combo(ui, studio);
            ui.end_row();
            ui.label("Latency overlay");
            ui.checkbox(&mut studio.input.show_latency, "")
                .on_hover_text("Show pen rate, input age and frame time in the status bar");
            ui.end_row();
        });
    });
}

fn sync_hover(s: DisplaySync) -> &'static str {
    match s {
        DisplaySync::Smooth => "Queues two frames: steadiest frame rate, more lag",
        DisplaySync::LowLatency => "Queues one frame (default)",
        DisplaySync::FastVsync => "Mailbox: newest frame wins, no tearing; DirectX 12 only",
        DisplaySync::Off => "No vsync: lowest lag, may tear, uses more power while drawing",
    }
}

fn sync_text(s: DisplaySync) -> String {
    if s == DisplaySync::default() { format!("{} (default)", s.label()) } else { s.label().to_owned() }
}

fn display_sync_combo(ui: &mut egui::Ui, studio: &mut Studio) {
    let current = &mut studio.input.display_sync;
    egui::ComboBox::from_id_salt("display-sync").selected_text(sync_text(*current)).show_ui(ui, |ui| {
        for s in DisplaySync::ALL {
            let enabled = s != DisplaySync::FastVsync || studio.fast_vsync_ok;
            let r = ui.add_enabled(enabled, egui::Button::selectable(*current == s, sync_text(s)));
            let r = if enabled { r.on_hover_text(sync_hover(s)) } else { r.on_disabled_hover_text("Needs the DirectX 12 backend") };
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
        DisplaySync::FastVsync => format!("{}. Not applied yet: ARTY starts with Low latency instead.", sync_hover(s)),
        _ => format!("{}. Takes effect the next time ARTY starts.", sync_hover(s)),
    }
}
