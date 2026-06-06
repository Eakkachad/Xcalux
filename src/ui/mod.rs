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
/// Returns the header's Response for attaching context menus.
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
        let (header_rect, header_response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::click());
        ui.painter().rect_filled(header_rect, 0.0, header_bg);
        ui.painter().text(
            egui::pos2(header_rect.min.x + 4.0, header_rect.center().y),
            egui::Align2::LEFT_CENTER,
            title,
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgb(34, 34, 34),
        );
        ui.allocate_ui_at_rect(
            egui::Rect::from_min_size(
                egui::pos2(header_rect.min.x, header_rect.max.y + 1.0),
                egui::vec2(
                    ui.available_width(),
                    ui.available_height() - header_rect.height() - 1.0,
                ),
            ),
            |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
                add_contents(ui);
            },
        );
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

/// Renders all floating panels as egui::Windows.
/// Delegates to panel-specific functions to avoid code extraction.
pub fn render_floating_panels(app: &mut crate::app::PaintApp, ctx: &egui::Context) {
    crate::ui::right_panel::render_floating_right_panels(app, ctx);
    crate::ui::left_panel::render_floating_left_panels(app, ctx);
}
