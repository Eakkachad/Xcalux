use crate::app::PaintApp;
use crate::canvas::{BlendMode, Layer};
use crate::commands::CommandId;
use crate::history::{HistoryCommand, LayerPropertyChange};
use crate::ui::layout::{PanelKind, PanelLocation};
use crate::ui::{panel_section, section_frame};

pub fn draw_right_panel(app: &mut PaintApp, ctx: &egui::Context) {
    if !app.show_minimal_ui && !app.workspace_layout.right_panel_visible {
        // Draw collapsed tab
        egui::SidePanel::right("right_sidebar_tab")
            .resizable(false)
            .default_width(22.0)
            .min_width(22.0)
            .max_width(22.0)
            .show(ctx, |ui| {
                let resp = ui
                    .add_sized(
                        egui::vec2(20.0, ui.available_height()),
                        egui::Button::new(egui::RichText::new("◀").size(10.0)),
                    )
                    .on_hover_text("Show Right Panel");
                if resp.clicked() {
                    app.workspace_layout.right_panel_visible = true;
                    app.workspace_layout.right_panel_collapsed = false;
                }
            });
        return;
    }
    if !app.show_minimal_ui {
        let panel_width = if app.workspace_layout.right_panel_collapsed {
            22.0
        } else {
            app.workspace_layout.right_panel_width
        };
        let panel_min_w = if app.workspace_layout.right_panel_collapsed {
            22.0
        } else {
            180.0
        };
        let panel_max_w = if app.workspace_layout.right_panel_collapsed {
            22.0
        } else {
            500.0
        };

        let side_panel = egui::SidePanel::right("right_sidebar")
            .resizable(!app.workspace_layout.right_panel_collapsed)
            .default_width(panel_width)
            .min_width(panel_min_w)
            .max_width(panel_max_w);

        side_panel.show(ctx, |ui| {
            let panel_style = ui.style_mut();
            panel_style.spacing.item_spacing = egui::vec2(4.0, 2.0);
            panel_style.spacing.button_padding = egui::vec2(3.0, 1.0);

            // Collapse/expand button at top
            ui.horizontal(|ui| {
                let collapse_label = if app.workspace_layout.right_panel_collapsed {
                    "◀"
                } else {
                    "▶"
                };
                let resp = ui
                    .add_sized(
                        egui::vec2(14.0, 14.0),
                        egui::Button::new(egui::RichText::new(collapse_label).size(9.0)),
                    )
                    .on_hover_text(if app.workspace_layout.right_panel_collapsed {
                        "Expand Right Panel"
                    } else {
                        "Collapse Right Panel"
                    });
                if resp.clicked() {
                    if app.workspace_layout.right_panel_collapsed {
                        app.workspace_layout.right_panel_collapsed = false;
                        app.workspace_layout.right_panel_visible = true;
                    } else {
                        app.workspace_layout.right_panel_collapsed = true;
                    }
                }
            });

            if !app.workspace_layout.right_panel_collapsed {
                egui::ScrollArea::vertical()
                    .id_source("right_sidebar_scroll")
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            // PANEL VISIBILITY SHORTCUT ROW
                            panel_section(ui, "TOGGLE PANELS", |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing.x = 2.0;
                                    ui.spacing_mut().item_spacing.y = 2.0;

                                    let nav_vis =
                                        app.workspace_layout.panel_visible(PanelKind::Navigator);
                                    if ui
                                        .add(egui::Button::new("🧭").selected(nav_vis))
                                        .on_hover_text("Toggle Navigator panel")
                                        .clicked()
                                    {
                                        app.workspace_layout
                                            .toggle_panel_visibility(PanelKind::Navigator);
                                    }
                                    let wheel_vis =
                                        app.workspace_layout.panel_visible(PanelKind::ColorWheel);
                                    if ui
                                        .add(egui::Button::new("🎨").selected(wheel_vis))
                                        .on_hover_text("Toggle Color Wheel panel")
                                        .clicked()
                                    {
                                        app.workspace_layout
                                            .toggle_panel_visibility(PanelKind::ColorWheel);
                                    }
                                    let sliders_vis =
                                        app.workspace_layout.panel_visible(PanelKind::ColorSliders);
                                    if ui
                                        .add(egui::Button::new("🎚").selected(sliders_vis))
                                        .on_hover_text("Toggle Color sliders")
                                        .clicked()
                                    {
                                        app.workspace_layout
                                            .toggle_panel_visibility(PanelKind::ColorSliders);
                                    }
                                    let pal_vis =
                                        app.workspace_layout.panel_visible(PanelKind::ColorPalette);
                                    if ui
                                        .add(egui::Button::new("▦").selected(pal_vis))
                                        .on_hover_text("Toggle Color Palette panel")
                                        .clicked()
                                    {
                                        app.workspace_layout
                                            .toggle_panel_visibility(PanelKind::ColorPalette);
                                    }
                                    let hist_vis =
                                        app.workspace_layout.panel_visible(PanelKind::ColorHistory);
                                    if ui
                                        .add(egui::Button::new("⏱").selected(hist_vis))
                                        .on_hover_text("Toggle Color History panel")
                                        .clicked()
                                    {
                                        app.workspace_layout
                                            .toggle_panel_visibility(PanelKind::ColorHistory);
                                    }
                                    let layers_vis = app
                                        .workspace_layout
                                        .panel_visible(PanelKind::LayersManager);
                                    if ui
                                        .add(egui::Button::new("🗂").selected(layers_vis))
                                        .on_hover_text("Toggle Layers panel")
                                        .clicked()
                                    {
                                        app.workspace_layout
                                            .toggle_panel_visibility(PanelKind::LayersManager);
                                    }
                                    /*
                                    let ref_vis =
                                        app.workspace_layout.panel_visible(PanelKind::Reference);
                                    if ui
                                        .add(egui::Button::new("🖼").selected(ref_vis))
                                        .on_hover_text("Toggle Reference panel")
                                        .clicked()
                                    {
                                        app.workspace_layout
                                            .toggle_panel_visibility(PanelKind::Reference);
                                    }
                                    */
                                    let sym_vis =
                                        app.workspace_layout.panel_visible(PanelKind::Symmetry);
                                    if ui
                                        .add(egui::Button::new("🪞").selected(sym_vis))
                                        .on_hover_text("Toggle Symmetry panel")
                                        .clicked()
                                    {
                                        app.workspace_layout
                                            .toggle_panel_visibility(PanelKind::Symmetry);
                                    }
                                    let tool_vis =
                                        app.workspace_layout.panel_visible(PanelKind::ToolOptions);
                                    if ui
                                        .add(egui::Button::new("🛠").selected(tool_vis))
                                        .on_hover_text("Toggle Tool Options panel")
                                        .clicked()
                                    {
                                        app.workspace_layout
                                            .toggle_panel_visibility(PanelKind::ToolOptions);
                                    }
                                });
                            }); // panel_section TOGGLE PANELS

                            // ── NAVIGATOR ──
                            if app
                                .workspace_layout
                                .is_panel_at(PanelKind::Navigator, PanelLocation::Right)
                            {
                                section_frame(ui, |ui| {
                                    let cr = egui::CollapsingHeader::new("NAVIGATOR")
                                        .default_open(true)
                                        .show(ui, |ui| {
                                            draw_navigator_content(app, ui);
                                        });
                                    crate::ui::handle_panel_drag(
                                        &cr.header_response,
                                        PanelKind::Navigator,
                                        app,
                                    );
                                    let _ = cr.header_response.context_menu(|ui| {
                                        panel_menu(ui, PanelKind::Navigator, app)
                                    });
                                });
                                ui.add_space(5.0);
                            }

                            // ── COLOR WHEEL ──
                            if app
                                .workspace_layout
                                .is_panel_at(PanelKind::ColorWheel, PanelLocation::Right)
                            {
                                section_frame(ui, |ui| {
                                    let cr = egui::CollapsingHeader::new("COLOR WHEEL")
                                        .default_open(true)
                                        .show(ui, |ui| {
                                            draw_color_wheel_content(app, ui);
                                        });
                                    crate::ui::handle_panel_drag(
                                        &cr.header_response,
                                        PanelKind::ColorWheel,
                                        app,
                                    );
                                    let _ = cr.header_response.context_menu(|ui| {
                                        panel_menu(ui, PanelKind::ColorWheel, app)
                                    });
                                });
                                ui.add_space(5.0);
                            }

                            // ── COLOR SLIDERS (RGB + HSV) ──
                            if app
                                .workspace_layout
                                .is_panel_at(PanelKind::ColorSliders, PanelLocation::Right)
                            {
                                section_frame(ui, |ui| {
                                    let cr = egui::CollapsingHeader::new("COLOR SLIDERS")
                                        .default_open(true)
                                        .show(ui, |ui| {
                                            draw_color_sliders_content(app, ui);
                                        });
                                    crate::ui::handle_panel_drag(
                                        &cr.header_response,
                                        PanelKind::ColorSliders,
                                        app,
                                    );
                                    let _ = cr.header_response.context_menu(|ui| {
                                        panel_menu(ui, PanelKind::ColorSliders, app)
                                    });
                                });
                                ui.add_space(5.0);
                            }

                            // ── COLOR PALETTE ──
                            if app
                                .workspace_layout
                                .is_panel_at(PanelKind::ColorPalette, PanelLocation::Right)
                            {
                                section_frame(ui, |ui| {
                                    let cr = egui::CollapsingHeader::new("COLOR PALETTE")
                                        .default_open(true)
                                        .show(ui, |ui| {
                                            draw_color_palette_content(app, ui);
                                        });
                                    crate::ui::handle_panel_drag(
                                        &cr.header_response,
                                        PanelKind::ColorPalette,
                                        app,
                                    );
                                    let _ = cr.header_response.context_menu(|ui| {
                                        panel_menu(ui, PanelKind::ColorPalette, app)
                                    });
                                });
                                ui.add_space(5.0);
                            }

                            // ── COLOR HISTORY ──
                            if app
                                .workspace_layout
                                .is_panel_at(PanelKind::ColorHistory, PanelLocation::Right)
                            {
                                section_frame(ui, |ui| {
                                    let cr = egui::CollapsingHeader::new("COLOR HISTORY")
                                        .default_open(true)
                                        .show(ui, |ui| {
                                            draw_color_history_content(app, ui);
                                        });
                                    crate::ui::handle_panel_drag(
                                        &cr.header_response,
                                        PanelKind::ColorHistory,
                                        app,
                                    );
                                    let _ = cr.header_response.context_menu(|ui| {
                                        panel_menu(ui, PanelKind::ColorHistory, app)
                                    });
                                });
                                ui.add_space(5.0);
                            }

                            // ── LAYERS MANAGER ──
                            if app
                                .workspace_layout
                                .is_panel_at(PanelKind::LayersManager, PanelLocation::Right)
                                && !app.layer_panel_on_left
                            {
                                section_frame(ui, |ui| {
                                    let cr = egui::CollapsingHeader::new("LAYERS MANAGER")
                                        .default_open(true)
                                        .show(ui, |ui| {
                                            draw_layers_manager_widget(app, ui, ctx);
                                        });
                                    crate::ui::handle_panel_drag(
                                        &cr.header_response,
                                        PanelKind::LayersManager,
                                        app,
                                    );
                                    let _ = cr.header_response.context_menu(|ui| {
                                        panel_menu(ui, PanelKind::LayersManager, app)
                                    });
                                });
                                ui.add_space(5.0);
                            }

                            /*
                            // ── REFERENCE ──
                            if app
                                .workspace_layout
                                .is_panel_at(PanelKind::Reference, PanelLocation::Right)
                            {
                                section_frame(ui, |ui| {
                                    let cr = egui::CollapsingHeader::new("REFERENCE")
                                        .default_open(true)
                                        .show(ui, |ui| {
                                            draw_reference_widget(app, ui, ctx);
                                        });
                                    crate::ui::handle_panel_drag(
                                        &cr.header_response,
                                        PanelKind::Reference,
                                        app,
                                    );
                                    let _ = cr.header_response.context_menu(|ui| {
                                        panel_menu(ui, PanelKind::Reference, app)
                                    });
                                });
                                ui.add_space(5.0);
                            }
                            */
                        }); // ui.vertical
                    }); // scroll area
            } // if !collapsed
        }); // side_panel.show
    } // if !show_minimal_ui
} // fn draw_right_panel

/// Renders floating right-side panels as egui::Windows.
pub fn render_floating_right_panels(app: &mut PaintApp, ctx: &egui::Context) {
    let panels = app.workspace_layout.panels.clone();
    for panel in &panels {
        if panel.location != PanelLocation::Floating || !panel.visible {
            continue;
        }
        let kind = panel.kind;
        let title = panel.title.clone();
        match kind {
            PanelKind::Navigator
            | PanelKind::ColorWheel
            | PanelKind::ColorSliders
            | PanelKind::ColorPalette
            | PanelKind::ColorHistory
            | PanelKind::LayersManager
            | PanelKind::Reference => {}
            _ => continue,
        }
        let default_side = match kind {
            PanelKind::ToolsAndPresets | PanelKind::BrushSettings | PanelKind::ToolOptions => {
                PanelLocation::Left
            }
            _ => PanelLocation::Right,
        };
        let window_resp = egui::Window::new(&title)
            .vscroll(true)
            .resizable(true)
            .min_size([200.0, 200.0])
            .default_size([300.0, 400.0])
            .id(egui::Id::new("floating_r").with(kind))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("← Dock").clicked() {
                        app.workspace_layout.set_panel_location(kind, default_side);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let _ = ui.menu_button("☰", |ui| {
                            crate::ui::panel_location_menu(ui, kind, app);
                        });
                    });
                });
                ui.separator();
                match kind {
                    PanelKind::Navigator => {
                        draw_navigator_content(app, ui);
                    }
                    PanelKind::ColorWheel => {
                        draw_color_wheel_content(app, ui);
                    }
                    PanelKind::ColorSliders => {
                        draw_color_sliders_content(app, ui);
                    }
                    PanelKind::ColorPalette => {
                        draw_color_palette_content(app, ui);
                    }
                    PanelKind::ColorHistory => {
                        draw_color_history_content(app, ui);
                    }
                    PanelKind::LayersManager => {
                        draw_layers_manager_widget(app, ui, ctx);
                    }
                    /*
                    PanelKind::Reference => {
                        draw_reference_widget(app, ui, ctx);
                    }
                    */
                    _ => {}
                }
            });
        // Drag-to-dock detection for floating windows
        if let Some(resp) = window_resp {
            if resp.response.dragged() || resp.response.drag_started() {
                app.floating_drag_panel = Some(crate::ui::layout::FloatingDragState { kind });
            }
            if resp.response.drag_stopped() {
                app.floating_drag_panel = None;
                let screen_rect = ctx.input(|i| i.screen_rect());
                let drop_zone_width = 40.0;
                if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                    if pos.x <= screen_rect.min.x + drop_zone_width {
                        app.workspace_layout
                            .set_panel_location(kind, PanelLocation::Left);
                    } else if pos.x >= screen_rect.max.x - drop_zone_width {
                        app.workspace_layout
                            .set_panel_location(kind, PanelLocation::Right);
                    }
                }
            }
        }
    }
}

// ── Panel location context menu (shared across all sections) ──
fn panel_menu(ui: &mut egui::Ui, kind: PanelKind, app: &mut PaintApp) {
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

// ── Extracted content functions for each panel kind ──

pub(crate) fn draw_navigator_content(app: &mut PaintApp, ui: &mut egui::Ui) {
    let nav_size = ui.available_width().min(300.0);
    if nav_size < 16.0 {
        return;
    }
    ui.vertical_centered(|ui| {
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(nav_size, nav_size),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter().with_clip_rect(rect);

        // Always fill the entire preview with medium gray workspace background
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(60, 60, 60));

        // Guard: skip canvas-aware drawing if canvas size is unavailable
        if app.canvas_width > 0 && app.canvas_height > 0 {
            // Compute aspect‑preserving canvas thumbnail rect centered in the preview
            let canvas_aspect = app.canvas_width as f32 / app.canvas_height as f32;
            let paper_rect = if canvas_aspect >= 1.0 {
                let paper_h = nav_size / canvas_aspect;
                egui::Rect::from_center_size(rect.center(), egui::vec2(nav_size, paper_h))
            } else {
                let paper_w = nav_size * canvas_aspect;
                egui::Rect::from_center_size(rect.center(), egui::vec2(paper_w, nav_size))
            };

            let has_thumbnail = app
                .renderer
                .as_ref()
                .and_then(|r| r.navigator_egui_id)
                .is_some();

            // Draw artwork thumbnail if available, otherwise fallback to checkerboard
            if has_thumbnail {
                if let Some(r) = &app.renderer {
                    if let Some(texture_id) = r.navigator_egui_id {
                        painter.image(
                            texture_id,
                            paper_rect,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                }
            } else {
                // Draw a checkerboard pattern for transparent canvas area
                let cell_size = 4.0;
                let cols = (paper_rect.width() / cell_size).ceil() as i32;
                let rows = (paper_rect.height() / cell_size).ceil() as i32;
                for yi in 0..rows {
                    for xi in 0..cols {
                        if (xi + yi) % 2 == 0 {
                            let cell_rect = egui::Rect::from_min_size(
                                egui::pos2(
                                    paper_rect.min.x + xi as f32 * cell_size,
                                    paper_rect.min.y + yi as f32 * cell_size,
                                ),
                                egui::Vec2::splat(cell_size),
                            )
                            .intersect(paper_rect);
                            if cell_rect.area() > 0.0 {
                                painter.rect_filled(
                                    cell_rect,
                                    0.0,
                                    egui::Color32::from_rgb(220, 220, 220),
                                );
                            }
                        }
                    }
                }
                // Subtle white overlay so the canvas area is visually distinct from the gray workspace
                painter.rect_filled(paper_rect, 0.0, egui::Color32::from_white_alpha(24));
            }

            // Canvas border (always visible)
            painter.rect_stroke(
                paper_rect,
                0.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(184, 184, 184)),
            );

            // Red viewport rectangle — transforms screen corners to world, then to navigator space
            if let Some(view_rect) = app.last_viewport_rect {
                let corners = [
                    view_rect.min,
                    egui::pos2(view_rect.max.x, view_rect.min.y),
                    view_rect.max,
                    egui::pos2(view_rect.min.x, view_rect.max.y),
                ];
                let stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(230, 50, 50));
                let mut nav_corners = Vec::with_capacity(4);
                for pt in corners {
                    let w = app.screen_to_world(pt, view_rect);
                    nav_corners.push(canvas_to_navigator(
                        w,
                        paper_rect,
                        app.canvas_width,
                        app.canvas_height,
                    ));
                }
                for i in 0..nav_corners.len() {
                    let j = (i + 1) % nav_corners.len();
                    painter.line_segment([nav_corners[i], nav_corners[j]], stroke);
                }
            }

            // Click/drag interaction
            if response.clicked() || response.dragged() {
                if let Some(click_pos) = response.interact_pointer_pos() {
                    let canvas_pos = navigator_to_canvas(
                        click_pos,
                        paper_rect,
                        app.canvas_width,
                        app.canvas_height,
                    );
                    let half_w = app.last_viewport_size.x * 0.5;
                    let half_h = app.last_viewport_size.y * 0.5;
                    app.viewport_offset =
                        canvas_pos - egui::vec2(half_w, half_h) / app.viewport_zoom;
                    ui.ctx().request_repaint();
                }
            }
        }

        // Tooltip on the preview area
        let _ = response.on_hover_text("Click or drag to pan the canvas");
    });

    // Buttons and readouts
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .button("Fit")
            .on_hover_text("Fit canvas to view")
            .clicked()
        {
            app.command(CommandId::FitToScreen);
        }
        if ui
            .button("100%")
            .on_hover_text("Set zoom to 100%")
            .clicked()
        {
            app.command(CommandId::ActualSize);
        }
        if ui.button("Reset").on_hover_text("Reset view").clicked() {
            app.command(CommandId::ResetView);
        }
    });
    ui.add_space(4.0);
    ui.label(format!("Zoom: {:.1}%", app.viewport_zoom * 100.0));
    let angle_deg = app.rotation_angle.to_degrees().round();
    let mirror_state = if app.mirror_horizontal {
        "Mirror On"
    } else {
        "Mirror Off"
    };
    ui.label(format!("Rot: {:.0}° | {}", angle_deg, mirror_state));
}

// ── Navigator helper functions ──

/// Convert a canvas‑space position to navigator preview coordinates.
pub(crate) fn canvas_to_navigator(
    canvas_pos: egui::Vec2,
    paper_rect: egui::Rect,
    canvas_width: u32,
    canvas_height: u32,
) -> egui::Pos2 {
    let pct_x = if canvas_width > 0 {
        canvas_pos.x / canvas_width as f32
    } else {
        0.0
    };
    let pct_y = if canvas_height > 0 {
        canvas_pos.y / canvas_height as f32
    } else {
        0.0
    };
    egui::pos2(
        paper_rect.min.x + pct_x * paper_rect.width(),
        paper_rect.min.y + pct_y * paper_rect.height(),
    )
}

/// Convert navigator preview coordinates to canvas‑space position (clamped to canvas bounds).
pub(crate) fn navigator_to_canvas(
    nav_pos: egui::Pos2,
    paper_rect: egui::Rect,
    canvas_width: u32,
    canvas_height: u32,
) -> egui::Vec2 {
    let pct_x = if paper_rect.width() > 0.0 {
        ((nav_pos.x - paper_rect.min.x) / paper_rect.width()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let pct_y = if paper_rect.height() > 0.0 {
        ((nav_pos.y - paper_rect.min.y) / paper_rect.height()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    egui::Vec2::new(pct_x * canvas_width as f32, pct_y * canvas_height as f32)
}

pub(crate) fn draw_color_wheel_content(app: &mut PaintApp, ui: &mut egui::Ui) {
    ui.vertical_centered(|ui| {
        let mut active_col = app.active_color();
        let res =
            crate::app::draw_hsv_color_wheel(ui, &mut active_col, &mut app.color_wheel_drag_zone);
        if res.changed() {
            app.set_active_color(active_col);
        }
        if res.drag_stopped() || res.clicked() {
            app.record_color(active_col);
        }
    });

    ui.add_space(5.0);

    ui.horizontal(|ui| {
        let (swatches_rect, response) =
            ui.allocate_exact_size(egui::vec2(50.0, 50.0), egui::Sense::click());
        let (trans_rect, trans_resp) =
            ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());

        if response.clicked() {
            if let Some(click_pos) = response.interact_pointer_pos() {
                let local_pos = click_pos - swatches_rect.min;
                if local_pos.x >= 0.0
                    && local_pos.x <= 34.0
                    && local_pos.y >= 0.0
                    && local_pos.y <= 34.0
                {
                    app.active_color_is_bg = false;
                    app.active_color_is_transparent = false;
                    app.brush_settings_dirty = true;
                } else if local_pos.x >= 16.0
                    && local_pos.x <= 50.0
                    && local_pos.y >= 16.0
                    && local_pos.y <= 50.0
                {
                    app.active_color_is_bg = true;
                    app.active_color_is_transparent = false;
                    app.brush_settings_dirty = true;
                }
            }
        }

        if trans_resp.clicked() {
            app.active_color_is_transparent = true;
            app.brush_settings_dirty = true;
        }
        if trans_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let painter = ui.painter();
        let bg_rect = egui::Rect::from_min_size(
            swatches_rect.min + egui::vec2(16.0, 16.0),
            egui::vec2(34.0, 34.0),
        );
        let bg_color = egui::Color32::from_rgb(
            (app.background_color[0] * 255.0) as u8,
            (app.background_color[1] * 255.0) as u8,
            (app.background_color[2] * 255.0) as u8,
        );
        painter.rect_filled(bg_rect, 0.0, bg_color);
        if app.active_color_is_bg && !app.active_color_is_transparent {
            painter.rect_stroke(
                bg_rect,
                0.0,
                egui::Stroke::new(2.5, egui::Color32::from_rgb(0, 120, 215)),
            );
        } else {
            painter.rect_stroke(bg_rect, 0.0, egui::Stroke::new(1.0, egui::Color32::GRAY));
        }

        let fg_rect = egui::Rect::from_min_size(swatches_rect.min, egui::vec2(34.0, 34.0));
        let fg_color = egui::Color32::from_rgb(
            (app.foreground_color[0] * 255.0) as u8,
            (app.foreground_color[1] * 255.0) as u8,
            (app.foreground_color[2] * 255.0) as u8,
        );
        painter.rect_filled(fg_rect, 0.0, fg_color);
        if !app.active_color_is_bg && !app.active_color_is_transparent {
            painter.rect_stroke(
                fg_rect,
                0.0,
                egui::Stroke::new(2.5, egui::Color32::from_rgb(0, 120, 215)),
            );
        } else {
            painter.rect_stroke(fg_rect, 0.0, egui::Stroke::new(1.0, egui::Color32::GRAY));
        }

        let size_w = 6.0;
        for row in 0..4 {
            for col in 0..4 {
                let sq_rect = egui::Rect::from_min_max(
                    trans_rect.min + egui::vec2(col as f32 * size_w, row as f32 * size_w),
                    trans_rect.min
                        + egui::vec2((col + 1) as f32 * size_w, (row + 1) as f32 * size_w),
                );
                painter.rect_filled(
                    sq_rect,
                    0.0,
                    if (row + col) % 2 == 0 {
                        egui::Color32::from_gray(240)
                    } else {
                        egui::Color32::from_gray(180)
                    },
                );
            }
        }
        if app.active_color_is_transparent {
            painter.rect_stroke(
                trans_rect,
                0.0,
                egui::Stroke::new(2.5, egui::Color32::from_rgb(0, 120, 215)),
            );
        } else {
            painter.rect_stroke(trans_rect, 0.0, egui::Stroke::new(1.0, egui::Color32::GRAY));
        }

        if ui.button("⇄").on_hover_text("Swap colors (X)").clicked() {
            std::mem::swap(&mut app.foreground_color, &mut app.background_color);
            app.active_color_is_transparent = false;
            app.brush_settings_dirty = true;
        }
    });

    ui.add_space(3.0);
    let active_col = app.active_color();
    let active_hex = format!(
        "#{:02X}{:02X}{:02X}",
        (active_col[0] * 255.0).round() as u8,
        (active_col[1] * 255.0).round() as u8,
        (active_col[2] * 255.0).round() as u8
    );
    ui.horizontal(|ui| {
        ui.label("Hex:");
        let hex_edit = ui.text_edit_singleline(&mut app.hex_color_input);
        if hex_edit.changed() {
            if let Some(parsed) = PaintApp::parse_hex_color(&app.hex_color_input) {
                app.set_active_color(parsed);
                app.record_color(parsed);
            }
        }
        if !hex_edit.has_focus() {
            app.hex_color_input = active_hex;
        }
    });
}

pub(crate) fn draw_color_sliders_content(app: &mut PaintApp, ui: &mut egui::Ui) {
    let mut active_col = app.active_color();
    let mut r_val = (active_col[0] * 255.0).round() as u8;
    let mut g_val = (active_col[1] * 255.0).round() as u8;
    let mut b_val = (active_col[2] * 255.0).round() as u8;
    let mut rgb_changed = false;
    let mut rgb_drag_released = false;
    ui.horizontal(|ui| {
        ui.label("R:");
        let res = ui.add(egui::Slider::new(&mut r_val, 0..=255));
        if res.changed() {
            rgb_changed = true;
        }
        if res.drag_stopped() {
            rgb_drag_released = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("G:");
        let res = ui.add(egui::Slider::new(&mut g_val, 0..=255));
        if res.changed() {
            rgb_changed = true;
        }
        if res.drag_stopped() {
            rgb_drag_released = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("B:");
        let res = ui.add(egui::Slider::new(&mut b_val, 0..=255));
        if res.changed() {
            rgb_changed = true;
        }
        if res.drag_stopped() {
            rgb_drag_released = true;
        }
    });
    if rgb_changed {
        active_col[0] = r_val as f32 / 255.0;
        active_col[1] = g_val as f32 / 255.0;
        active_col[2] = b_val as f32 / 255.0;
        app.set_active_color(active_col);
    }
    if rgb_drag_released {
        app.record_color(active_col);
    }

    ui.add_space(4.0);

    let (h, s, v) = crate::app::rgb_to_hsv(active_col[0], active_col[1], active_col[2]);
    let mut h_deg = (h * 360.0).round() as u32;
    let mut s_pct = (s * 100.0).round() as u32;
    let mut v_pct = (v * 100.0).round() as u32;
    let mut hsv_changed = false;
    let mut hsv_drag_released = false;
    ui.horizontal(|ui| {
        ui.label("H:");
        let res = ui.add(egui::Slider::new(&mut h_deg, 0..=360).suffix("°"));
        if res.changed() {
            hsv_changed = true;
        }
        if res.drag_stopped() {
            hsv_drag_released = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("S:");
        let res = ui.add(egui::Slider::new(&mut s_pct, 0..=100).suffix("%"));
        if res.changed() {
            hsv_changed = true;
        }
        if res.drag_stopped() {
            hsv_drag_released = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("V:");
        let res = ui.add(egui::Slider::new(&mut v_pct, 0..=100).suffix("%"));
        if res.changed() {
            hsv_changed = true;
        }
        if res.drag_stopped() {
            hsv_drag_released = true;
        }
    });
    if hsv_changed {
        let (r, g, b) = crate::app::hsv_to_rgb(
            h_deg as f32 / 360.0,
            s_pct as f32 / 100.0,
            v_pct as f32 / 100.0,
        );
        active_col[0] = r;
        active_col[1] = g;
        active_col[2] = b;
        app.set_active_color(active_col);
    }
    if hsv_drag_released {
        app.record_color(active_col);
    }
}

pub(crate) fn draw_color_palette_content(app: &mut PaintApp, ui: &mut egui::Ui) {
    let mut clicked_palette_color = None;
    let mut clicked_palette_idx = None;
    egui::Grid::new("color_palette")
        .num_columns(6)
        .spacing([4.0, 4.0])
        .show(ui, |ui| {
            for (i, color) in app.palette.iter().enumerate() {
                let fill = egui::Color32::from_rgb(
                    (color[0] * 255.0) as u8,
                    (color[1] * 255.0) as u8,
                    (color[2] * 255.0) as u8,
                );
                let is_selected_swatch = app.selected_palette_index == Some(i);
                let btn_response = ui
                    .add(
                        egui::Button::new("")
                            .min_size(egui::Vec2::splat(22.0))
                            .fill(fill),
                    )
                    .on_hover_text("Pick palette color");
                if is_selected_swatch {
                    ui.painter().rect_stroke(
                        btn_response.rect.expand(1.5),
                        1.0,
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 120, 215)),
                    );
                }
                if btn_response.clicked() {
                    clicked_palette_color = Some(*color);
                    clicked_palette_idx = Some(i);
                }
                if i % 6 == 5 {
                    ui.end_row();
                }
            }
        });
    if let Some(picked) = clicked_palette_color {
        app.set_active_color(picked);
        app.selected_palette_index = clicked_palette_idx;
        app.record_color(picked);
        app.brush_settings_dirty = true;
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .button("Save")
            .on_hover_text("Save current color to selected swatch")
            .clicked()
        {
            if let Some(i) = app.selected_palette_index {
                if i < app.palette.len() {
                    app.palette[i] = app.active_color();
                }
            }
        }
        if ui
            .button("+")
            .on_hover_text("Add current color to palette")
            .clicked()
            && app.palette.len() < 36
        {
            let active_col = app.active_color();
            app.palette.push(active_col);
            app.selected_palette_index = Some(app.palette.len() - 1);
        }
        if ui
            .add_enabled(
                app.selected_palette_index.is_some() && app.palette.len() > 1,
                egui::Button::new("-"),
            )
            .on_hover_text("Remove selected swatch from palette")
            .clicked()
        {
            if let Some(i) = app.selected_palette_index.take() {
                if i < app.palette.len() {
                    app.palette.remove(i);
                }
            }
        }
    });
}

pub(crate) fn draw_color_history_content(app: &mut PaintApp, ui: &mut egui::Ui) {
    let mut clicked_history_color = None;
    if !app.color_history.is_empty() {
        ui.horizontal_wrapped(|ui| {
            let hist_len = app.color_history.len();
            for (i, color) in app.color_history.iter().rev().enumerate() {
                if i >= 12 {
                    break;
                }
                let fill = egui::Color32::from_rgb(
                    (color[0] * 255.0) as u8,
                    (color[1] * 255.0) as u8,
                    (color[2] * 255.0) as u8,
                );
                let btn = ui.add(
                    egui::Button::new("")
                        .min_size(egui::Vec2::splat(16.0))
                        .fill(fill),
                );
                if btn.clicked() {
                    clicked_history_color = Some(*color);
                }
                if i < hist_len.min(12) - 1 {
                    ui.add_space(2.0);
                }
            }
        });
    }
    if let Some(color) = clicked_history_color {
        app.set_active_color(color);
        app.brush_settings_dirty = true;
    }
}

/*
pub(crate) fn draw_reference_widget(app: &mut PaintApp, ui: &mut egui::Ui, _ctx: &egui::Context) {
    egui::CollapsingHeader::new("REFERENCE")
        .default_open(true)
        .show(ui, |ui| {
            ui.group(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(3.0, 2.0);
                ui.horizontal(|ui| {
                    ui.label("Path:");
                    let text_edit = egui::TextEdit::singleline(&mut app.ref_image_path_input)
                        .hint_text("Path to PNG...");
                    ui.add(text_edit);
                });

                ui.horizontal(|ui| {
                    if ui.button("+ Add Image").clicked() {
                        let path = app.ref_image_path_input.clone();
                        if let Err(e) = app.load_reference_image(&path, ui.ctx()) {
                            log::error!("Failed to load reference image: {}", e);
                        } else {
                            app.ref_image_path_input.clear();
                        }
                    }
                    if ui.button("Hide All").clicked() {
                        for img in &mut app.reference_images {
                            img.visible = false;
                        }
                    }
                    if ui.button("Show All").clicked() {
                        for img in &mut app.reference_images {
                            img.visible = true;
                        }
                    }
                });

                if !app.reference_images.is_empty() {
                    ui.add_space(4.0);
                    ui.label("List:");
                    let mut to_remove_idx = None;
                    for (idx, img) in app.reference_images.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            let eye_text = if img.visible { "👁" } else { "⦂" };
                            let btn_eye = egui::Button::new(eye_text)
                                .frame(false)
                                .selected(img.visible);
                            if ui
                                .add(btn_eye)
                                .on_hover_text("Toggle reference visibility")
                                .clicked()
                            {
                                img.visible = !img.visible;
                            }

                            let is_selected = app.selected_reference_idx == Some(idx);
                            let opacity_pct = (img.opacity * 100.0).round() as i32;
                            let label_text = format!("{} ({}%)", img.name, opacity_pct);
                            if ui.selectable_label(is_selected, &label_text).clicked() {
                                app.selected_reference_idx = Some(idx);
                            }
                        });
                    }

                    if let Some(idx) = app.selected_reference_idx {
                        if idx < app.reference_images.len() {
                            ui.add_space(4.0);
                            ui.separator();
                            ui.label("Selected Reference:");

                            let canvas_w = app.canvas_width as f32;
                            let canvas_h = app.canvas_height as f32;
                            let img = &mut app.reference_images[idx];

                            ui.horizontal(|ui| {
                                ui.label("Opacity:");
                                ui.add(
                                    egui::Slider::new(&mut img.opacity, 0.0..=1.0).show_value(true),
                                );
                            });

                            ui.horizontal(|ui| {
                                ui.label("Scale:");
                                ui.add(
                                    egui::Slider::new(&mut img.scale, 0.1..=5.0).show_value(true),
                                );
                            });

                            ui.horizontal(|ui| {
                                ui.label("Rotation:");
                                let mut degrees = img.rotation.to_degrees();
                                if ui
                                    .add(
                                        egui::Slider::new(&mut degrees, -180.0..=180.0)
                                            .show_value(true)
                                            .suffix("°"),
                                    )
                                    .changed()
                                {
                                    img.rotation = degrees.to_radians();
                                }
                            });

                            ui.horizontal(|ui| {
                                if ui
                                    .selectable_label(img.pinned_to_view, "Pin to View")
                                    .clicked()
                                    && !img.pinned_to_view
                                {
                                    img.pinned_to_view = true;
                                    img.world_pos = egui::vec2(200.0, 200.0);
                                }
                                if ui
                                    .selectable_label(!img.pinned_to_view, "Pin to Canvas")
                                    .clicked()
                                    && img.pinned_to_view
                                {
                                    img.pinned_to_view = false;
                                    img.world_pos = egui::vec2(canvas_w * 0.5, canvas_h * 0.5);
                                }
                            });

                            if ui.button("Remove").clicked() {
                                to_remove_idx = Some(idx);
                            }
                        }
                    }

                    if let Some(remove_idx) = to_remove_idx {
                        app.reference_images.remove(remove_idx);
                        app.selected_reference_idx = None;
                    }
                }
            });
        });
}
*/

pub(crate) fn draw_layers_manager_widget(
    app: &mut PaintApp,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
) {
    ui.spacing_mut().item_spacing = egui::vec2(3.0, 1.0);

            // --- LAYER EFFECT SECTION ---
            egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                ui.next_auto_id(),
                false,
            )
            .show_header(ui, |ui| {
                ui.label("Layer Effect");
            })
            .body(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(3.0, 1.0);

                // Paper subsection
                ui.group(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(3.0, 1.0);
                    ui.label("Paper");
                    ui.horizontal(|ui| {
                        ui.label("Paper:");
                        ui.label("[ No Texture ]");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Intensity");
                        let mut dummy_i = 0.0;
                        ui.add_enabled(
                            false,
                            egui::Slider::new(&mut dummy_i, 0.0..=100.0).show_value(false),
                        );
                        ui.add_enabled(false, egui::DragValue::new(&mut dummy_i).speed(1.0));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Scale");
                        let mut dummy_s = 10.0;
                        ui.add_enabled(
                            false,
                            egui::Slider::new(&mut dummy_s, 1.0..=500.0).show_value(false),
                        );
                        ui.add_enabled(
                            false,
                            egui::DragValue::new(&mut dummy_s).speed(1.0).suffix("%"),
                        );
                    });
                    ui.add_enabled(false, egui::Checkbox::new(&mut false, "Apply to Linework"));
                });

                ui.add_space(2.0);

                // Effect subsection
                ui.group(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(3.0, 1.0);
                    ui.label("Effect");
                    ui.horizontal(|ui| {
                        ui.label("Effect:");
                        ui.label("[ No effect ]");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Width");
                        let mut dummy_w = 1.0;
                        ui.add_enabled(
                            false,
                            egui::Slider::new(&mut dummy_w, 1.0..=20.0).show_value(false),
                        );
                        ui.add_enabled(false, egui::DragValue::new(&mut dummy_w).speed(1.0));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Intensity");
                        let mut dummy_i = 0.0;
                        ui.add_enabled(
                            false,
                            egui::Slider::new(&mut dummy_i, 0.0..=100.0).show_value(false),
                        );
                        ui.add_enabled(false, egui::DragValue::new(&mut dummy_i).speed(1.0));
                    });
                });
            });

            ui.add_space(2.0);

            // --- ACTIVE LAYER CONTROLS (Mode / Opacity / Lock / Clipping) ---
            let layer_id = app.active_layer_id;
            let (
                old_opacity,
                old_blend,
                old_lock_alpha,
                old_clipping,
                _old_visible,
                old_name,
                old_locked,
            ) = if let Some(l) = app.layers.get(&layer_id) {
                (
                    l.opacity,
                    l.blend_mode,
                    l.lock_alpha,
                    l.is_clipping,
                    l.visible,
                    l.name.clone(),
                    l.locked,
                )
            } else {
                (
                    1.0,
                    BlendMode::Normal,
                    false,
                    false,
                    true,
                    String::new(),
                    false,
                )
            };

            if let Some(active_layer) = app.layers.get_mut(&app.active_layer_id) {
                // Mode row
                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    egui::ComboBox::from_id_source("blend_mode_dropdown")
                        .selected_text(format!("{:?}", active_layer.blend_mode))
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut active_layer.blend_mode,
                                BlendMode::Normal,
                                "Normal",
                            );
                            ui.selectable_value(
                                &mut active_layer.blend_mode,
                                BlendMode::Multiply,
                                "Multiply",
                            );
                            ui.selectable_value(
                                &mut active_layer.blend_mode,
                                BlendMode::Screen,
                                "Screen",
                            );
                            ui.selectable_value(
                                &mut active_layer.blend_mode,
                                BlendMode::Overlay,
                                "Overlay",
                            );
                            ui.selectable_value(
                                &mut active_layer.blend_mode,
                                BlendMode::Luminosity,
                                "Luminosity (Shine)",
                            );
                            ui.selectable_value(
                                &mut active_layer.blend_mode,
                                BlendMode::Shade,
                                "Shade",
                            );
                        });
                });

                // Opacity row
                ui.horizontal(|ui| {
                    ui.label("Opacity:");
                    ui.add(
                        egui::Slider::new(&mut active_layer.opacity, 0.0..=1.0).show_value(false),
                    );
                    let mut opacity_pct = (active_layer.opacity * 100.0).round() as i32;
                    if ui
                        .add(
                            egui::DragValue::new(&mut opacity_pct)
                                .speed(1)
                                .clamp_range(0..=100),
                        )
                        .changed()
                    {
                        active_layer.opacity = (opacity_pct as f32 / 100.0).clamp(0.0, 1.0);
                    }
                });

                // Lock row
                ui.horizontal(|ui| {
                    ui.label("Lock:");
                    let alpha_locked = active_layer.lock_alpha;
                    let maker_locked = active_layer.locked;
                    let move_locked = false;
                    let full_locked = false;

                    let alpha_res = ui
                        .add_sized(
                            [20.0, 20.0],
                            egui::Button::new(
                                egui::RichText::new(if alpha_locked { "A" } else { "A" })
                                    .size(11.0),
                            )
                            .selected(alpha_locked),
                        )
                        .on_hover_text("Lock Alpha (Preserve Opacity)");
                    if alpha_res.clicked() {
                        active_layer.lock_alpha = !active_layer.lock_alpha;
                    }

                    let draw_res = ui
                        .add_sized(
                            [20.0, 20.0],
                            egui::Button::new(
                                egui::RichText::new(if maker_locked { "D" } else { "D" })
                                    .size(11.0),
                            )
                            .selected(maker_locked),
                        )
                        .on_hover_text("Lock Drawing");
                    if draw_res.clicked() {
                        active_layer.locked = !active_layer.locked;
                    }

                    let _move_btn = ui
                        .add_sized(
                            [20.0, 20.0],
                            egui::Button::new(egui::RichText::new("M").size(11.0))
                                .selected(move_locked),
                        )
                        .on_hover_text("Lock Movement (future)");

                    let _full_btn = ui
                        .add_sized(
                            [20.0, 20.0],
                            egui::Button::new(egui::RichText::new("F").size(11.0))
                                .selected(full_locked),
                        )
                        .on_hover_text("Full Lock (future)");
                });

                // Clipping + Selection Source row
                ui.horizontal(|ui| {
                    if ui
                        .checkbox(&mut active_layer.is_clipping, "Clipping Group")
                        .changed()
                    {
                        // handled below via old_clipping comparison
                    }
                    let sel_src = active_layer.selection_source;
                    let sel_res = ui
                        .add_sized(
                            [20.0, 20.0],
                            egui::Button::new(
                                egui::RichText::new(if sel_src { "◎" } else { "⚬" }).size(12.0),
                            )
                            .selected(sel_src),
                        )
                        .on_hover_text("Selection Source (reference for Bucket/Wand)");
                    if sel_res.clicked() {
                        active_layer.selection_source = !active_layer.selection_source;
                    }
                });
            }

            // Push history commands for property changes
            if let Some(active_layer) = app.layers.get(&layer_id) {
                let aid = app.active_layer_id;
                let mut commands: Vec<HistoryCommand> = Vec::new();
                if (active_layer.opacity - old_opacity).abs() > f32::EPSILON {
                    commands.push(HistoryCommand::LayerProperty {
                        layer_id: aid,
                        property: LayerPropertyChange::Opacity {
                            old: old_opacity,
                            new: active_layer.opacity,
                        },
                    });
                }
                if active_layer.blend_mode != old_blend {
                    commands.push(HistoryCommand::LayerProperty {
                        layer_id: aid,
                        property: LayerPropertyChange::BlendMode {
                            old: old_blend,
                            new: active_layer.blend_mode,
                        },
                    });
                }
                if active_layer.lock_alpha != old_lock_alpha {
                    commands.push(HistoryCommand::LayerProperty {
                        layer_id: aid,
                        property: LayerPropertyChange::LockAlpha {
                            old: old_lock_alpha,
                            new: active_layer.lock_alpha,
                        },
                    });
                }
                if active_layer.is_clipping != old_clipping {
                    commands.push(HistoryCommand::LayerProperty {
                        layer_id: aid,
                        property: LayerPropertyChange::Clipping {
                            old: old_clipping,
                            new: active_layer.is_clipping,
                        },
                    });
                }
                if active_layer.name != old_name {
                    commands.push(HistoryCommand::LayerProperty {
                        layer_id: aid,
                        property: LayerPropertyChange::Rename {
                            old: old_name,
                            new: active_layer.name.clone(),
                        },
                    });
                }
                if active_layer.locked != old_locked {
                    commands.push(HistoryCommand::LayerProperty {
                        layer_id: aid,
                        property: LayerPropertyChange::Locked {
                            old: old_locked,
                            new: active_layer.locked,
                        },
                    });
                }
                for cmd in commands {
                    app.history.push_command(cmd);
                }
            }

            // Vector layer display mode toggle
            let (is_vector, is_spline_mode) = app
                .layers
                .get(&app.active_layer_id)
                .map(|l| {
                    let is_v = matches!(l.kind, crate::canvas::LayerType::Vector);
                    let is_spline = l
                        .vector_data
                        .as_ref()
                        .map(|vd| vd.display_mode == crate::canvas::VectorDisplayMode::SplineMesh)
                        .unwrap_or(false);
                    (is_v, is_spline)
                })
                .unwrap_or((false, false));

            if is_vector {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label("Vector:");
                    if ui.selectable_label(!is_spline_mode, "Raster").clicked() {
                        if let Some(layer) = app.layers.get_mut(&app.active_layer_id) {
                            if let Some(vd) = &mut layer.vector_data {
                                vd.display_mode = crate::canvas::VectorDisplayMode::Rasterized;
                            }
                        }
                    }
                    if ui.selectable_label(is_spline_mode, "Spline").clicked() {
                        if let Some(layer) = app.layers.get_mut(&app.active_layer_id) {
                            if let Some(vd) = &mut layer.vector_data {
                                vd.display_mode = crate::canvas::VectorDisplayMode::SplineMesh;
                            }
                        }
                    }
                });

                let selected_width = app.edit_cp_selection.and_then(|(si, _)| {
                    app.layers.get(&app.active_layer_id).and_then(|l| {
                        l.vector_data
                            .as_ref()
                            .and_then(|vd| vd.strokes.get(si).map(|s| s.width))
                    })
                });
                if let Some(cur_width) = selected_width {
                    let mut new_width = cur_width;
                    let si = app.edit_cp_selection.map(|(s, _)| s).unwrap_or(0);
                    ui.horizontal(|ui| {
                        ui.label("Width:");
                        if ui
                            .add(egui::Slider::new(&mut new_width, 0.1..=10.0).step_by(0.1))
                            .changed()
                        {
                            if let Some(layer) = app.layers.get_mut(&app.active_layer_id) {
                                if let Some(vd) = &mut layer.vector_data {
                                    if si < vd.strokes.len() {
                                        vd.strokes[si].width = new_width;
                                        app.redraw_vector_layer(app.active_layer_id);
                                    }
                                }
                            }
                        }
                    });
                }

                ui.horizontal(|ui| {
                    if ui.button("To Raster").clicked() {
                        app.convert_active_vector_to_raster();
                        ctx.request_repaint();
                    }
                    if ui.button("Export SVG").clicked() {
                        if let Some(layer) = app.layers.get(&app.active_layer_id) {
                            if let Some(vd) = &layer.vector_data {
                                let svg_content = crate::vector::export_strokes_svg(
                                    &vd.strokes,
                                    app.canvas_width,
                                    app.canvas_height,
                                );
                                let svg_path =
                                    std::path::Path::new(&app.document_path).with_extension("svg");
                                if let Err(e) = std::fs::write(&svg_path, &svg_content) {
                                    log::error!("Failed to export SVG: {}", e);
                                } else {
                                    log::info!("Exported SVG to {:?}", svg_path);
                                }
                            }
                        }
                    }
                });
            }

            ui.add_space(2.0);

            // --- LAYER OPERATION ICON TOOLBAR ---
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
                if ui
                    .add_sized([22.0, 22.0], egui::Button::new("🎨⁺"))
                    .on_hover_text("New Raster Layer")
                    .clicked()
                {
                    app.layer_id_counter += 1;
                    let new_id = app.layer_id_counter;
                    let mut new_layer = Layer::new(new_id, format!("Layer {}", new_id));
                    new_layer.kind = crate::canvas::LayerType::Raster;
                    let layer_clone = new_layer.clone();
                    app.layers.insert(new_id, new_layer);
                    app.layer_order.insert(0, new_id);
                    app.active_layer_id = new_id;
                    app.history.push_command(HistoryCommand::LayerCreate {
                        layer: Box::new(layer_clone),
                        index: 0,
                    });
                }
                if ui
                    .add_sized([22.0, 22.0], egui::Button::new("📁⁺"))
                    .on_hover_text("New Folder")
                    .clicked()
                {
                    app.layer_id_counter += 1;
                    let new_id = app.layer_id_counter;
                    let mut new_layer = Layer::new(new_id, format!("Folder {}", new_id));
                    new_layer.kind = crate::canvas::LayerType::Folder {
                        child_ids: Vec::new(),
                    };
                    let layer_clone = new_layer.clone();
                    app.layers.insert(new_id, new_layer);
                    app.layer_order.insert(0, new_id);
                    app.active_layer_id = new_id;
                    app.history.push_command(HistoryCommand::LayerCreate {
                        layer: Box::new(layer_clone),
                        index: 0,
                    });
                }
                if ui
                    .add_sized([22.0, 22.0], egui::Button::new("⬡⁺"))
                    .on_hover_text("New Vector Layer")
                    .clicked()
                {
                    app.layer_id_counter += 1;
                    let new_id = app.layer_id_counter;
                    let mut new_layer = Layer::new(new_id, format!("Vector {}", new_id));
                    new_layer.kind = crate::canvas::LayerType::Vector;
                    new_layer.vector_data = Some(crate::canvas::VectorLayer {
                        strokes: Vec::new(),
                        display_mode: Default::default(),
                    });
                    let layer_clone = new_layer.clone();
                    app.layers.insert(new_id, new_layer);
                    app.layer_order.insert(0, new_id);
                    app.active_layer_id = new_id;
                    app.history.push_command(HistoryCommand::LayerCreate {
                        layer: Box::new(layer_clone),
                        index: 0,
                    });
                }
                ui.separator();
                if ui
                    .add_enabled(
                        app.layer_order.len() > 1,
                        egui::Button::new("🗑").min_size(egui::Vec2::new(22.0, 22.0)),
                    )
                    .on_hover_text("Delete Layer")
                    .clicked()
                {
                    if app.layer_order.len() > 1 {
                        let active_id = app.active_layer_id;
                        if let Some(pos) = app.layer_order.iter().position(|&x| x == active_id) {
                            let removed = app.layers.remove(&active_id).unwrap();
                            app.layer_order.remove(pos);
                            app.active_layer_id = app.layer_order[0];
                            app.history.push_command(HistoryCommand::LayerDelete {
                                layer: Box::new(removed),
                                index: pos,
                            });
                        }
                    }
                }
                if ui
                    .add_sized([22.0, 22.0], egui::Button::new("🧹"))
                    .on_hover_text("Clear Layer")
                    .clicked()
                {
                    app.command(CommandId::ClearLayer);
                }
                if ui
                    .add_sized([22.0, 22.0], egui::Button::new("🪣"))
                    .on_hover_text("Fill Layer")
                    .clicked()
                {
                    app.command(CommandId::FillLayer);
                }
                if ui
                    .add_sized([22.0, 22.0], egui::Button::new("🖼⁺"))
                    .on_hover_text("Import Image as Layer")
                    .clicked()
                {
                    app.command(CommandId::ImportImageAsLayer);
                }
            });

            // --- MASK TOOLBAR ---
            let mask_state = app
                .layers
                .get(&app.active_layer_id)
                .map(|l| (l.mask.is_some(), l.mask.as_ref().is_some_and(|m| m.enabled)));
            let has_mask = mask_state.is_some_and(|(h, _)| h);
            let mask_enabled = mask_state.is_some_and(|(_, e)| e);
            if mask_state.is_some() {
                ui.add_space(1.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
                    if ui
                        .add_enabled(
                            !has_mask,
                            egui::Button::new("🎭⁺").min_size(egui::Vec2::new(22.0, 22.0)),
                        )
                        .on_hover_text("Add Mask")
                        .clicked()
                    {
                        app.command(CommandId::AddLayerMask);
                    }
                    if ui
                        .add_enabled(
                            has_mask,
                            egui::Button::new("🗑🎭").min_size(egui::Vec2::new(22.0, 22.0)),
                        )
                        .on_hover_text("Delete Mask")
                        .clicked()
                    {
                        app.command(CommandId::DeleteLayerMask);
                    }
                    if ui
                        .add_enabled(
                            has_mask,
                            egui::Button::new("🎭").selected(mask_enabled)
                                .min_size(egui::Vec2::new(22.0, 22.0)),
                        )
                        .on_hover_text("Toggle Mask")
                        .clicked()
                    {
                        app.command(CommandId::ToggleLayerMask);
                    }
                    if ui
                        .add_enabled(
                            has_mask,
                            egui::Button::new("🎭✓").min_size(egui::Vec2::new(22.0, 22.0)),
                        )
                        .on_hover_text("Apply Mask")
                        .clicked()
                    {
                        app.command(CommandId::ApplyLayerMask);
                    }
                    if ui
                        .add_enabled(
                            has_mask,
                            egui::Button::new("Inv").min_size(egui::Vec2::new(22.0, 22.0)),
                        )
                        .on_hover_text("Invert Mask")
                        .clicked()
                    {
                        app.command(CommandId::InvertLayerMask);
                    }
                    if has_mask {
                        let label = if app.active_mask_editing { "Mc" } else { "Ma" };
                        if ui
                            .add_sized(
                                [22.0, 22.0],
                                egui::Button::new(label).selected(app.active_mask_editing),
                            )
                            .on_hover_text("Edit Mask / Edit Color")
                            .clicked()
                        {
                            app.active_mask_editing = !app.active_mask_editing;
                        }
                    }
                });
            }

            ui.add_space(2.0);

            // --- LAYER LIST ---
            let mut thumb_textures: ahash::AHashMap<u32, egui::TextureHandle> =
                ahash::AHashMap::default();
            for id in &app.layer_order.clone() {
                if let Some(tex) = app.get_layer_thumbnail_texture(ctx, *id) {
                    thumb_textures.insert(*id, tex);
                }
            }

            egui::ScrollArea::vertical()
                .id_source("layers_list_scroll")
                .max_height(250.0)
                .show(ui, |ui| {
                    let order = app.layer_order.clone();
                    'layer_loop: for id in order {
                        let pointer_released = ui.ctx().input(|i| i.pointer.any_released());
                        let is_active = app.active_layer_id == id;
                        let mut drag_started = false;

                        let row_height = 48.0;
                        let avail_w = ui.available_width();
                        let (row_rect, row_response) =
                            ui.allocate_exact_size(egui::vec2(avail_w, row_height), egui::Sense::click());
                        let row_hovered = row_response.hovered();

                        // Right click activates the layer first
                        if row_response.secondary_clicked() {
                            app.active_layer_id = id;
                        }

                        // Context menu triggers
                        let mut rename_clicked = false;
                        let mut duplicate_clicked = false;
                        let mut delete_clicked = false;
                        let mut merge_clicked = false;

                        row_response.context_menu(|ui| {
                            if ui.button("Rename").clicked() {
                                rename_clicked = true;
                                ui.close_menu();
                            }
                            if ui.button("Duplicate").clicked() {
                                duplicate_clicked = true;
                                ui.close_menu();
                            }
                            let order_len = app.layer_order.len();
                            if ui.add_enabled(order_len > 1, egui::Button::new("Delete")).clicked() {
                                delete_clicked = true;
                                ui.close_menu();
                            }
                            let pos = app.layer_order.iter().position(|&x| x == id).unwrap_or(0);
                            if ui.add_enabled(pos + 1 < order_len, egui::Button::new("Merge Down")).clicked() {
                                merge_clicked = true;
                                ui.close_menu();
                            }
                            ui.separator();
                            ui.menu_button("Properties", |ui| {
                                if let Some(layer) = app.layers.get_mut(&id) {
                                    ui.horizontal(|ui| {
                                        ui.label("Opacity:");
                                        ui.add(egui::Slider::new(&mut layer.opacity, 0.0..=1.0));
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("Blend:");
                                        egui::ComboBox::from_id_source(format!("blend_ctx_{}", id))
                                            .selected_text(format!("{:?}", layer.blend_mode))
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(&mut layer.blend_mode, BlendMode::Normal, "Normal");
                                                ui.selectable_value(&mut layer.blend_mode, BlendMode::Multiply, "Multiply");
                                                ui.selectable_value(&mut layer.blend_mode, BlendMode::Screen, "Screen");
                                                ui.selectable_value(&mut layer.blend_mode, BlendMode::Overlay, "Overlay");
                                                ui.selectable_value(&mut layer.blend_mode, BlendMode::Luminosity, "Luminosity");
                                                ui.selectable_value(&mut layer.blend_mode, BlendMode::Shade, "Shade");
                                            });
                                    });
                                    ui.checkbox(&mut layer.lock_alpha, "Lock Alpha");
                                    ui.checkbox(&mut layer.locked, "Lock Drawing");
                                    ui.checkbox(&mut layer.is_clipping, "Clipping Group");
                                }
                            });
                        });

                        // Deferred context menu execution
                        if rename_clicked {
                            app.renaming_layer_id = Some(id);
                            if let Some(l) = app.layers.get(&id) {
                                app.rename_layer_input = l.name.clone();
                            }
                        }
                        if duplicate_clicked {
                            app.active_layer_id = id;
                            app.duplicate_active_layer();
                        }
                        if delete_clicked {
                            app.active_layer_id = id;
                            app.delete_active_layer();
                        }
                        if merge_clicked {
                            app.active_layer_id = id;
                            app.merge_down();
                        }

                        // Draw row with painter (fresh borrow per iteration)
                        {
                            let p = ui.painter();
                            if is_active {
                                p.rect_filled(row_rect, 0.0, egui::Color32::from_rgb(224, 220, 255));
                                p.rect_stroke(
                                    row_rect,
                                    0.0,
                                    egui::Stroke::new(1.5, egui::Color32::from_rgb(125, 120, 255)),
                                );
                            } else if row_hovered {
                                p.rect_filled(row_rect, 0.0, egui::Color32::from_gray(245));
                            } else {
                                p.rect_filled(row_rect, 0.0, egui::Color32::WHITE);
                            }
                        }

                        if let Some(layer) = app.layers.get_mut(&id) {
                            // Drag handle
                            let grip_rect = egui::Rect::from_min_size(
                                egui::pos2(row_rect.min.x + 2.0, row_rect.min.y + 2.0),
                                egui::vec2(14.0, row_height - 4.0),
                            );
                            let grip_resp = ui.allocate_rect(grip_rect, egui::Sense::click_and_drag());
                            ui.painter().text(
                                grip_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "⠿",
                                egui::FontId::proportional(10.0),
                                egui::Color32::from_gray(160),
                            );

                            if grip_resp.drag_started() {
                                app.dragging_layer_id = Some(id);
                                app.active_layer_id = id;
                                drag_started = true;
                            }

                            // Visibility eye
                            let vis_x = row_rect.min.x + 18.0;
                            let vis_rect = egui::Rect::from_min_size(
                                egui::pos2(vis_x, row_rect.min.y + 2.0),
                                egui::vec2(18.0, row_height - 4.0),
                            );
                            let vis_resp = ui.allocate_rect(vis_rect, egui::Sense::click());
                            {
                                let p = ui.painter();
                                let eye_text = if layer.visible { "👁" } else { "⦂" };
                                let eye_color = if layer.visible {
                                    egui::Color32::BLACK
                                } else {
                                    egui::Color32::from_gray(160)
                                };
                                p.text(
                                    vis_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    eye_text,
                                    egui::FontId::proportional(11.0),
                                    eye_color,
                                );
                            }
                            if vis_resp.clicked() {
                                let old_vis = layer.visible;
                                layer.visible = !layer.visible;
                                app.history.push_command(HistoryCommand::LayerProperty {
                                    layer_id: id,
                                    property: LayerPropertyChange::Visible {
                                        old: old_vis,
                                        new: layer.visible,
                                    },
                                });
                            }

                            // Reference dot
                            let ref_rect = egui::Rect::from_min_size(
                                egui::pos2(vis_x, row_rect.min.y + 24.0),
                                egui::vec2(18.0, 18.0),
                            );
                            let ref_resp = ui.allocate_rect(ref_rect, egui::Sense::click());
                            {
                                let p = ui.painter();
                                let ref_text = if layer.selection_source { "◎" } else { "⚬" };
                                let ref_color = if layer.selection_source {
                                    egui::Color32::from_rgb(0, 120, 215)
                                } else {
                                    egui::Color32::from_gray(160)
                                };
                                p.text(
                                    ref_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    ref_text,
                                    egui::FontId::proportional(10.0),
                                    ref_color,
                                );
                            }
                            if ref_resp.clicked() {
                                layer.selection_source = !layer.selection_source;
                            }

                            // Layer type label
                            let type_str = match &layer.kind {
                                crate::canvas::LayerType::Folder { .. } => "F",
                                crate::canvas::LayerType::Vector => "V",
                                crate::canvas::LayerType::Raster => "R",
                            };

                            // Thumbnail
                            let thumb_left = row_rect.min.x + 42.0;
                            let thumb_size = 38.0;
                            let thumb_rect = egui::Rect::from_min_size(
                                egui::pos2(thumb_left, row_rect.min.y + (row_height - thumb_size) / 2.0),
                                egui::Vec2::splat(thumb_size),
                            );
                            let thumb_resp = ui.allocate_rect(thumb_rect, egui::Sense::click());
                            {
                                let p = ui.painter();
                                p.rect_filled(thumb_rect, 1.0, egui::Color32::WHITE);
                                p.rect_stroke(
                                    thumb_rect,
                                    1.0,
                                    egui::Stroke::new(1.0, egui::Color32::from_gray(180)),
                                );
                                if let Some(tex) = thumb_textures.get(&id) {
                                    p.image(
                                        tex.id(),
                                        thumb_rect,
                                        egui::Rect::from_min_max(
                                            egui::Pos2::ZERO,
                                            egui::Pos2::new(1.0, 1.0),
                                        ),
                                        egui::Color32::WHITE,
                                    );
                                }
                            }
                            if is_active && thumb_resp.clicked() {
                                app.active_mask_editing = false;
                            }

                            // Mask overlay on thumbnail
                            if layer.mask.is_some() {
                                let mask_overlay_rect = egui::Rect::from_min_size(
                                    egui::pos2(thumb_rect.max.x - 16.0, thumb_rect.max.y - 16.0),
                                    egui::Vec2::splat(16.0),
                                );
                                {
                                    let p = ui.painter();
                                    p.rect_filled(mask_overlay_rect, 0.0, egui::Color32::WHITE);
                                    p.rect_stroke(
                                        mask_overlay_rect,
                                        0.0,
                                        egui::Stroke::new(1.0, egui::Color32::from_gray(180)),
                                    );
                                    if is_active && app.active_mask_editing {
                                        p.rect_stroke(
                                            mask_overlay_rect.expand(1.0),
                                            0.0,
                                            egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 200, 80)),
                                        );
                                    }
                                }
                                let mask_thumb_resp =
                                    ui.allocate_rect(mask_overlay_rect, egui::Sense::click());
                                if mask_thumb_resp.clicked() {
                                    app.active_mask_editing = true;
                                }
                                if mask_thumb_resp.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                            }

                            // Text: name, blend mode, opacity
                            let text_x = thumb_left + thumb_size + 6.0;
                            let text_width = row_rect.max.x - text_x - 4.0;
                            let name_rect = egui::Rect::from_min_max(
                                egui::pos2(text_x, row_rect.min.y + 2.0),
                                egui::pos2(text_x + text_width.max(50.0), row_rect.min.y + 16.0),
                            );

                            if app.renaming_layer_id == Some(id) {
                                let text_edit = egui::TextEdit::singleline(&mut app.rename_layer_input)
                                    .font(egui::FontId::proportional(11.0));
                                let res = ui.put(name_rect, text_edit);
                                res.request_focus();
                                if res.lost_focus() || (ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                                    if !app.rename_layer_input.is_empty() {
                                        let old_name = layer.name.clone();
                                        layer.name = app.rename_layer_input.clone();
                                        app.history.push_command(HistoryCommand::LayerProperty {
                                            layer_id: id,
                                            property: LayerPropertyChange::Rename {
                                                old: old_name,
                                                new: layer.name.clone(),
                                            },
                                        });
                                    }
                                    app.renaming_layer_id = None;
                                }
                            } else {
                                let p = ui.painter();
                                p.text(
                                    egui::pos2(text_x, row_rect.min.y + 4.0),
                                    egui::Align2::LEFT_TOP,
                                    format!("{}  {}", type_str, layer.name),
                                    egui::FontId::proportional(11.0),
                                    egui::Color32::BLACK,
                                );
                            }

                            {
                                let p = ui.painter();
                                p.text(
                                    egui::pos2(text_x, row_rect.min.y + 19.0),
                                    egui::Align2::LEFT_TOP,
                                    format!("{:?}", layer.blend_mode),
                                    egui::FontId::proportional(10.0),
                                    egui::Color32::from_gray(100),
                                );
                                let opacity_pct = (layer.opacity * 100.0).round() as i32;
                                p.text(
                                    egui::pos2(text_x, row_rect.min.y + 32.0),
                                    egui::Align2::LEFT_TOP,
                                    format!("{}%", opacity_pct),
                                    egui::FontId::proportional(10.0),
                                    egui::Color32::from_gray(100),
                                );
                            }

                            if row_response.clicked() {
                                app.active_layer_id = id;
                            }
                        }

                        // Drag reorder
                        if let Some(dragging_id) = app.dragging_layer_id {
                            if dragging_id == id && drag_started {
                                app.drag_start_order = Some(app.layer_order.clone());
                            }
                            if dragging_id != id && row_hovered {
                                if let (Some(from), Some(to)) = (
                                    app.layer_order.iter().position(|&lid| lid == dragging_id),
                                    app.layer_order.iter().position(|&lid| lid == id),
                                ) {
                                    app.layer_order.swap(from, to);
                                }
                            }
                            if pointer_released {
                                if let Some(old_order) = app.drag_start_order.take() {
                                    let new_order = app.layer_order.clone();
                                    if old_order != new_order {
                                        app.history.push_command(HistoryCommand::LayerReorder {
                                            old_order,
                                            new_order,
                                        });
                                    }
                                }
                                app.dragging_layer_id = None;
                                continue 'layer_loop;
                            }
                        }
                    }
                });
}
