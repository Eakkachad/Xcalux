use crate::app::PaintApp;
use crate::diagnostics::DeviceType;

const TILE_SIZE_BYTES: usize = 64 * 64 * 4;

/// Render the Performance HUD overlay in the top-right corner of the canvas.
pub fn draw_performance_hud(app: &mut PaintApp, ctx: &egui::Context) {
    if !app.performance_hud.enabled {
        return;
    }

    let avg_ms = app.performance_hud.avg_frame_time() * 1000.0;
    let fps = app.performance_hud.current_fps();

    let cache_used = app.renderer.as_ref().map(|r| r.cache_used()).unwrap_or(0);
    let cache_max = app.renderer.as_ref().map(|r| r.cache_max()).unwrap_or(1);

    // Estimate memory: tile cache + undo stack + active canvas
    let tile_cache_mb = (cache_used * TILE_SIZE_BYTES) as f32 / (1024.0 * 1024.0);
    let undo_mb = (app.history.undo_stack.len() * TILE_SIZE_BYTES) as f32 / (1024.0 * 1024.0);
    let mut active_tile_count = 0usize;
    for layer in app.layers.values() {
        active_tile_count += layer.tiles.len();
    }
    let active_canvas_mb = (active_tile_count * TILE_SIZE_BYTES) as f32 / (1024.0 * 1024.0);

    egui::Window::new("Performance HUD")
        .anchor(egui::Align2::RIGHT_TOP, [-10.0, 10.0])
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(egui::Color32::from_black_alpha(180))
                .inner_margin(egui::Margin::same(8.0)),
        )
        .title_bar(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Performance HUD")
                        .strong()
                        .color(egui::Color32::WHITE),
                );
                ui.separator();
                ui.colored_label(
                    egui::Color32::WHITE,
                    format!("FPS: {:.1} ({:.2} ms)", fps, avg_ms),
                );
                ui.colored_label(
                    egui::Color32::WHITE,
                    format!("VRAM Slots: {} / {}", cache_used, cache_max),
                );
                ui.colored_label(
                    egui::Color32::WHITE,
                    format!(
                        "VRAM Uploads: {}",
                        app.performance_hud.dirty_uploads_this_frame
                    ),
                );
                ui.colored_label(
                    egui::Color32::WHITE,
                    format!("Brush Dabs: {:.0} dabs/sec", app.performance_hud.dab_rate),
                );
                ui.colored_label(
                    egui::Color32::WHITE,
                    format!("Tile Cache: {:.2} MB", tile_cache_mb),
                );
                ui.colored_label(
                    egui::Color32::WHITE,
                    format!(
                        "Undo Stack: {:.2} MB ({} entries)",
                        undo_mb,
                        app.history.undo_stack.len()
                    ),
                );
                ui.colored_label(
                    egui::Color32::WHITE,
                    format!(
                        "Active Canvas: {:.2} MB ({} tiles)",
                        active_canvas_mb, active_tile_count
                    ),
                );
            });
        });
}

/// Render the Tablet Diagnostics window.
pub fn draw_tablet_diagnostics(app: &mut PaintApp, ctx: &egui::Context) {
    if !app.tablet_diagnostics.enabled {
        return;
    }

    let mut open = app.tablet_diagnostics.enabled;
    egui::Window::new("Tablet Diagnostics")
        .open(&mut open)
        .default_size([400.0, 360.0])
        .show(ctx, |ui| {
            ui.heading("Raw Input Stream");
            let device_str = match app.tablet_diagnostics.device_type {
                DeviceType::None => "(none)",
                DeviceType::Mouse => "Mouse",
                DeviceType::Pen => "Pen",
                DeviceType::Touch => "Touch",
            };
            ui.label(format!("Device Type: {}", device_str));
            ui.label(format!(
                "Coordinates: X={:.2}, Y={:.2}",
                app.tablet_diagnostics.raw_x, app.tablet_diagnostics.raw_y
            ));

            ui.horizontal(|ui| {
                ui.label("Pressure:");
                ui.add(
                    egui::ProgressBar::new(app.tablet_diagnostics.pressure)
                        .text(format!("{:.1}%", app.tablet_diagnostics.pressure * 100.0))
                        .desired_width(200.0),
                );
            });

            ui.label(format!(
                "Tilt: X={:.1}°, Y={:.1}°",
                app.tablet_diagnostics.tilt_x_deg, app.tablet_diagnostics.tilt_y_deg
            ));
            ui.label(format!(
                "Tip: {}, Proximity: {}",
                app.tablet_diagnostics.tip_down, app.tablet_diagnostics.in_proximity
            ));
            ui.label(format!(
                "Packet Rate: {} Hz",
                app.tablet_diagnostics.packet_rate
            ));

            ui.add_space(4.0);
            ui.label("Pressure History:");
            let history = app.tablet_diagnostics.pressure_history.clone();
            if history.is_empty() {
                ui.label("(no data yet)");
            } else {
                draw_pressure_graph(ui, &history);
            }
        });
    app.tablet_diagnostics.enabled = open;
}

/// Draw a simple pressure graph as a connected polyline.
fn draw_pressure_graph(ui: &mut egui::Ui, history: &[f32]) {
    use egui::Pos2;
    let desired = egui::vec2(ui.available_width(), 120.0);
    let (rect, _response) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(60));
    painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, egui::Color32::DARK_GRAY));

    if history.len() < 2 {
        return;
    }
    let n = history.len();
    let x_step = rect.width() / (n as f32 - 1.0).max(1.0);
    let points: Vec<Pos2> = history
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            let x = rect.left() + i as f32 * x_step;
            let y = rect.bottom() - p.clamp(0.0, 1.0) * rect.height();
            Pos2::new(x, y)
        })
        .collect();
    let stroke = egui::Stroke::new(1.5, egui::Color32::LIGHT_BLUE);
    for w in points.windows(2) {
        painter.line_segment([w[0], w[1]], stroke);
    }
}
