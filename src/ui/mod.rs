pub mod brush_panel;
pub mod command_palette;
pub mod diagnostics;
pub mod dialogs;
pub mod layout;
pub mod left_panel;
pub mod menu;
pub mod quick_bar;
pub mod right_panel;
pub mod status_bar;

pub use layout::{PanelKind, PanelLocation};

/// Shared UI helper: renders a thin bordered frame around existing content (e.g. CollapsingHeader).
pub fn section_frame(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    let frame = egui::Frame::none()
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(184, 184, 184),
        ))
        .inner_margin(egui::Margin::symmetric(1.0, 1.0));
    frame.show(ui, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
        add_contents(ui);
    });
    ui.add_space(2.0);
}

/// Shared UI helper: renders a bordered section with a compact header bar.
/// Returns the header's Response for attaching context menus and drag detection.
pub fn panel_section(
    ui: &mut egui::Ui,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let frame = egui::Frame::none()
        .fill(egui::Color32::from_rgb(238, 238, 238))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(184, 184, 184),
        ))
        .inner_margin(egui::Margin::symmetric(2.0, 2.0));
    let resp = frame.show(ui, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
        // Header
        let header_bg = egui::Color32::from_rgb(230, 230, 230);
        let (header_rect, header_response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 20.0),
            egui::Sense::click_and_drag(),
        );
        ui.painter().rect_filled(header_rect, 0.0, header_bg);
        ui.painter().text(
            egui::pos2(header_rect.min.x + 4.0, header_rect.center().y),
            egui::Align2::LEFT_CENTER,
            title,
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgb(34, 34, 34),
        );
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
            add_contents(ui);
        });
        header_response
    });
    ui.add_space(2.0);
    resp.inner
}

/// Context menu contents for panel location (Dock Left / Dock Right / Float / Hide).
pub fn panel_location_menu(ui: &mut egui::Ui, kind: PanelKind, app: &mut crate::app::PaintApp) {
    if ui.button("Dock Left").clicked() {
        app.workspace_layout
            .set_panel_location(kind, PanelLocation::Left);
        ui.close_menu();
    }
    if ui.button("Dock Right").clicked() {
        app.workspace_layout
            .set_panel_location(kind, PanelLocation::Right);
        ui.close_menu();
    }
    if ui.button("Float").clicked() {
        app.workspace_layout
            .set_panel_location(kind, PanelLocation::Floating);
        ui.close_menu();
    }
    ui.separator();
    if ui.button("Hide").clicked() {
        app.workspace_layout
            .set_panel_location(kind, PanelLocation::Hidden);
        ui.close_menu();
    }
}

const DROP_ZONE_WIDTH: f32 = 40.0;
const DROP_ZONE_COLOR: egui::Color32 = egui::Color32::from_rgba_premultiplied(0, 120, 215, 50);
const DROP_ZONE_HOVER_COLOR: egui::Color32 =
    egui::Color32::from_rgba_premultiplied(0, 120, 215, 90);

/// Renders all floating panels as egui::Windows and handles drop zones.
pub fn render_floating_panels(app: &mut crate::app::PaintApp, ctx: &egui::Context) {
    // Clean up stale floating drag state when pointer is released
    if !ctx.input(|i| i.pointer.primary_down()) {
        app.floating_drag_panel = None;
    }

    crate::ui::right_panel::render_floating_right_panels(app, ctx);
    crate::ui::left_panel::render_floating_left_panels(app, ctx);

    // Draw drop zone indicators if a floating window is being dragged
    if app.floating_drag_panel.is_some() {
        draw_drop_zones(app, ctx);
    }
}

/// Draws translucent blue drop zone indicators at the left/right screen edges.
fn draw_drop_zones(_app: &crate::app::PaintApp, ctx: &egui::Context) {
    let screen_rect = ctx.input(|i| i.screen_rect());
    let pointer_pos = ctx.input(|i| i.pointer.interact_pos());

    let hovered_zone = pointer_pos.and_then(|pos| {
        if pos.x <= screen_rect.min.x + DROP_ZONE_WIDTH {
            Some(PanelLocation::Left)
        } else if pos.x >= screen_rect.max.x - DROP_ZONE_WIDTH {
            Some(PanelLocation::Right)
        } else {
            None
        }
    });

    egui::Area::new(egui::Id::new("drop_zones"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .interactable(false)
        .show(ctx, |ui| {
            let painter = ui.painter();

            // Left drop zone
            let left_rect = egui::Rect::from_min_size(
                egui::pos2(screen_rect.min.x, screen_rect.min.y),
                egui::vec2(DROP_ZONE_WIDTH, screen_rect.height()),
            );
            let left_color = if hovered_zone == Some(PanelLocation::Left) {
                DROP_ZONE_HOVER_COLOR
            } else {
                DROP_ZONE_COLOR
            };
            painter.rect_filled(left_rect, 4.0, left_color);
            painter.text(
                egui::pos2(left_rect.center().x, left_rect.center().y),
                egui::Align2::CENTER_CENTER,
                "Dock Left",
                egui::FontId::proportional(11.0),
                egui::Color32::WHITE,
            );

            // Right drop zone
            let right_rect = egui::Rect::from_min_size(
                egui::pos2(screen_rect.max.x - DROP_ZONE_WIDTH, screen_rect.min.y),
                egui::vec2(DROP_ZONE_WIDTH, screen_rect.height()),
            );
            let right_color = if hovered_zone == Some(PanelLocation::Right) {
                DROP_ZONE_HOVER_COLOR
            } else {
                DROP_ZONE_COLOR
            };
            painter.rect_filled(right_rect, 4.0, right_color);
            painter.text(
                egui::pos2(right_rect.center().x, right_rect.center().y),
                egui::Align2::CENTER_CENTER,
                "Dock Right",
                egui::FontId::proportional(11.0),
                egui::Color32::WHITE,
            );
        });
}

/// Check for drag-to-float on a panel header or drag-handle response.
/// Call this on the header Response of each docked panel.
/// When the user drags beyond THRESHOLD the panel switches to Floating
/// and its `floating.position` is set near the cursor.
const PANEL_DRAG_THRESHOLD: f32 = 6.0;

pub fn handle_panel_drag(
    response: &egui::Response,
    kind: PanelKind,
    app: &mut crate::app::PaintApp,
) {
    // Record drag start
    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            app.panel_drag = Some(crate::ui::layout::PanelDragState {
                kind,
                drag_start_screen: pos,
                detached: false,
            });
        }
    }

    // Check threshold and detach
    let is_active = app
        .panel_drag
        .as_ref()
        .map(|d| d.kind == kind && !d.detached)
        .unwrap_or(false);

    if is_active && response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            let start = app.panel_drag.as_ref().unwrap().drag_start_screen;
            if pos.distance(start) > PANEL_DRAG_THRESHOLD {
                // Switch to Floating and position near cursor
                let side = match kind {
                    PanelKind::ToolsAndPresets
                    | PanelKind::BrushPresets
                    | PanelKind::BrushSettings
                    | PanelKind::ToolOptions
                    | PanelKind::Stabilizer
                    | PanelKind::Symmetry
                    | PanelKind::AdvancedDebug => PanelLocation::Left,
                    _ => PanelLocation::Right,
                };
                // Preserve the current location; use default if not found
                let _curr = app
                    .workspace_layout
                    .find_panel(kind)
                    .map(|p| p.location)
                    .unwrap_or(side);

                app.workspace_layout
                    .set_panel_location(kind, PanelLocation::Floating);
                if let Some(p) = app.workspace_layout.find_panel_mut(kind) {
                    p.floating.position = [pos.x - 60.0, pos.y - 20.0];
                }
                if let Some(d) = app.panel_drag.as_mut() {
                    d.detached = true;
                }
            }
        }
    }

    // Clean up on release
    if response.drag_stopped() {
        if app
            .panel_drag
            .as_ref()
            .map(|d| d.kind == kind)
            .unwrap_or(false)
        {
            app.panel_drag = None;
        }
    }
}
