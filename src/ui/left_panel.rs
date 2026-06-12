use crate::app::{PaintApp, PresetIcon, ToolId};
use crate::input::{StabilizerLevel, StabilizerMode};
use crate::ui::layout::{PanelKind, PanelLocation};
use crate::ui::panel_section;
use hokusai::{Brush, BrushSetting, BrushState};

fn draw_dashed_line(painter: &egui::Painter, p1: egui::Pos2, p2: egui::Pos2, stroke: egui::Stroke) {
    let dist = p1.distance(p2);
    if dist < 0.1 {
        return;
    }
    let dash_len = 2.0;
    let gap_len = 2.0;
    let step = dash_len + gap_len;
    let dir = (p2 - p1) / dist;
    let mut t = 0.0;
    while t < dist {
        let start = p1 + dir * t;
        let end = p1 + dir * (t + dash_len).min(dist);
        painter.line_segment([start, end], stroke);
        t += step;
    }
}

fn draw_dashed_circle(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    stroke: egui::Stroke,
) {
    let circumference = 2.0 * std::f32::consts::PI * radius;
    if circumference < 0.1 {
        return;
    }
    let dash_len = 2.0;
    let gap_len = 2.0;
    let step = dash_len + gap_len;
    let num_steps = (circumference / step).round() as i32;
    for i in 0..num_steps {
        let angle1 = (i as f32 / num_steps as f32) * 2.0 * std::f32::consts::PI;
        let angle2 = ((i as f32 + 0.5) / num_steps as f32) * 2.0 * std::f32::consts::PI;
        let start = center + egui::vec2(angle1.cos(), angle1.sin()) * radius;
        let end = center + egui::vec2(angle2.cos(), angle2.sin()) * radius;
        painter.line_segment([start, end], stroke);
    }
}

pub fn draw_left_panel(app: &mut PaintApp, ctx: &egui::Context) {
    if !app.show_minimal_ui && !app.workspace_layout.left_panel_visible {
        // Draw collapsed tab
        egui::SidePanel::left("left_sidebar_tab")
            .resizable(false)
            .default_width(22.0)
            .min_width(22.0)
            .max_width(22.0)
            .show(ctx, |ui| {
                let resp = ui
                    .add_sized(
                        egui::vec2(20.0, ui.available_height()),
                        egui::Button::new(egui::RichText::new("▶").size(10.0)),
                    )
                    .on_hover_text("Show Left Panel");
                if resp.clicked() {
                    app.workspace_layout.left_panel_visible = true;
                    app.workspace_layout.left_panel_collapsed = false;
                }
            });
        return;
    }
    if !app.show_minimal_ui {
        let panel_width = if app.workspace_layout.left_panel_collapsed {
            22.0
        } else {
            app.workspace_layout.left_panel_width
        };
        let panel_min_w = if app.workspace_layout.left_panel_collapsed {
            22.0
        } else {
            180.0
        };
        let panel_max_w = if app.workspace_layout.left_panel_collapsed {
            22.0
        } else {
            500.0
        };

        let side_panel = egui::SidePanel::left("left_sidebar")
            .resizable(!app.workspace_layout.left_panel_collapsed)
            .default_width(panel_width)
            .min_width(panel_min_w)
            .max_width(panel_max_w);

        side_panel.show(ctx, |ui| {
            let panel_style = ui.style_mut();
            panel_style.spacing.item_spacing = egui::vec2(4.0, 2.0);
            panel_style.spacing.button_padding = egui::vec2(3.0, 1.0);
            panel_style.spacing.indent = 8.0;

            // Collapse/expand button at top
            ui.horizontal(|ui| {
                let collapse_label = if app.workspace_layout.left_panel_collapsed {
                    "▶"
                } else {
                    "◀"
                };
                let resp = ui
                    .add_sized(
                        egui::vec2(14.0, 14.0),
                        egui::Button::new(egui::RichText::new(collapse_label).size(9.0)),
                    )
                    .on_hover_text(if app.workspace_layout.left_panel_collapsed {
                        "Expand Left Panel"
                    } else {
                        "Collapse Left Panel"
                    });
                if resp.clicked() {
                    if app.workspace_layout.left_panel_collapsed {
                        app.workspace_layout.left_panel_collapsed = false;
                        app.workspace_layout.left_panel_visible = true;
                    } else {
                        app.workspace_layout.left_panel_collapsed = true;
                    }
                }
            });

            if !app.workspace_layout.left_panel_collapsed {
                egui::ScrollArea::vertical()
                    .id_source("left_sidebar_scroll")
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            // ── TOOLS / BRUSH PRESETS ──
                            if app
                                .workspace_layout
                                .is_panel_at(PanelKind::ToolsAndPresets, PanelLocation::Left)
                            {
                                let tp_resp = panel_section(ui, "TOOLS / BRUSH PRESETS", |ui| {
                                    draw_tools_and_presets_content(app, ui, ctx);
                                });
                                crate::ui::handle_panel_drag(
                                    &tp_resp,
                                    PanelKind::ToolsAndPresets,
                                    app,
                                );
                                let _ = tp_resp.context_menu(|ui| {
                                    crate::ui::panel_location_menu(
                                        ui,
                                        PanelKind::ToolsAndPresets,
                                        app,
                                    )
                                });
                            } // is_panel_at ToolsAndPresets

                            // ── BRUSH SETTINGS ──
                            if app
                                .workspace_layout
                                .is_panel_at(PanelKind::BrushSettings, PanelLocation::Left)
                            {
                                let bs_resp = panel_section(ui, "BRUSH SETTINGS", |ui| {
                                    draw_brush_settings_content(app, ui, ctx);
                                });
                                crate::ui::handle_panel_drag(
                                    &bs_resp,
                                    PanelKind::BrushSettings,
                                    app,
                                );
                                let _ = bs_resp.context_menu(|ui| {
                                    crate::ui::panel_location_menu(
                                        ui,
                                        PanelKind::BrushSettings,
                                        app,
                                    )
                                });
                            } // is_panel_at BrushSettings

                            // ── TOOL OPTIONS ──
                            if app
                                .workspace_layout
                                .is_panel_at(PanelKind::ToolOptions, PanelLocation::Left)
                            {
                                let to_resp = panel_section(ui, "TOOL OPTIONS", |ui| {
                                    draw_tool_options_content(app, ui, ctx);
                                });
                                crate::ui::handle_panel_drag(&to_resp, PanelKind::ToolOptions, app);
                                let _ = to_resp.context_menu(|ui| {
                                    crate::ui::panel_location_menu(ui, PanelKind::ToolOptions, app)
                                });
                            } // is_panel_at ToolOptions

                            // ── STABILIZER ──
                            if app
                                .workspace_layout
                                .is_panel_at(PanelKind::Stabilizer, PanelLocation::Left)
                            {
                                let st_resp = panel_section(ui, "STABILIZER", |ui| {
                                    draw_stabilizer_content(app, ui, ctx);
                                });
                                crate::ui::handle_panel_drag(&st_resp, PanelKind::Stabilizer, app);
                                let _ = st_resp.context_menu(|ui| {
                                    crate::ui::panel_location_menu(ui, PanelKind::Stabilizer, app)
                                });
                            } // is_panel_at Stabilizer

                            // ── SYMMETRY ──
                            if app
                                .workspace_layout
                                .is_panel_at(PanelKind::Symmetry, PanelLocation::Left)
                            {
                                let sy_resp = panel_section(ui, "SYMMETRY / DRAWING GUIDE", |ui| {
                                    draw_symmetry_content(app, ui, ctx);
                                });
                                crate::ui::handle_panel_drag(&sy_resp, PanelKind::Symmetry, app);
                                let _ = sy_resp.context_menu(|ui| {
                                    crate::ui::panel_location_menu(ui, PanelKind::Symmetry, app)
                                });
                            } // is_panel_at Symmetry

                            // ── ADVANCED / DEBUG ──
                            if app
                                .workspace_layout
                                .is_panel_at(PanelKind::AdvancedDebug, PanelLocation::Left)
                            {
                                let ad_resp = panel_section(ui, "ADVANCED / DEBUG", |ui| {
                                    draw_advanced_debug_content(app, ui, ctx);
                                });
                                crate::ui::handle_panel_drag(
                                    &ad_resp,
                                    PanelKind::AdvancedDebug,
                                    app,
                                );
                                let _ = ad_resp.context_menu(|ui| {
                                    crate::ui::panel_location_menu(
                                        ui,
                                        PanelKind::AdvancedDebug,
                                        app,
                                    )
                                });
                            } // is_panel_at AdvancedDebug
                        }); // ui.vertical
                    }); // scroll area
            } // if !collapsed
        }); // side_panel.show
    } // if !show_minimal_ui
} // fn draw_left_panel

pub(crate) fn draw_tools_and_presets_content(
    app: &mut PaintApp,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
) {
    egui::Grid::new("tools_grid")
        .num_columns(5)
        .spacing([4.0, 4.0])
        .show(ui, |ui| {
            // ROW 1
            let active_shape_tool = if app.active_tool() == ToolId::EllipseSelect {
                ToolId::EllipseSelect
            } else {
                ToolId::RectSelect
            };
            let is_active =
                app.active_tool() == ToolId::RectSelect || app.active_tool() == ToolId::EllipseSelect;
            let btn = egui::Button::new("").selected(is_active);
            let btn_resp = ui.add_sized([26.0, 26.0], btn);
            let icon_color = if is_active {
                egui::Color32::from_rgb(0, 120, 215)
            } else {
                ui.style().visuals.widgets.inactive.text_color()
            };
            let stroke = egui::Stroke::new(1.0, icon_color);
            if active_shape_tool == ToolId::RectSelect {
                let r = btn_resp.rect.shrink(6.0);
                let w = r.width();
                let h = r.height();
                draw_dashed_line(ui.painter(), r.min, r.min + egui::vec2(w, 0.0), stroke);
                draw_dashed_line(ui.painter(), r.min + egui::vec2(0.0, h), r.max, stroke);
                draw_dashed_line(ui.painter(), r.min, r.min + egui::vec2(0.0, h), stroke);
                draw_dashed_line(ui.painter(), r.min + egui::vec2(w, 0.0), r.max, stroke);
            } else {
                let center = btn_resp.rect.center();
                draw_dashed_circle(ui.painter(), center, 7.0, stroke);
            }
            if btn_resp.clicked() {
                app.set_active_tool(active_shape_tool);
                ctx.request_repaint();
            }
            btn_resp.context_menu(|ui| {
                if ui
                    .selectable_label(app.active_tool() == ToolId::RectSelect, "Rectangle Selection")
                    .clicked()
                {
                    app.set_active_tool(ToolId::RectSelect);
                    ui.close_menu();
                }
                if ui
                    .selectable_label(
                        app.active_tool() == ToolId::EllipseSelect,
                        "Ellipse Selection",
                    )
                    .clicked()
                {
                    app.set_active_tool(ToolId::EllipseSelect);
                    ui.close_menu();
                }
            });
            btn_resp.on_hover_text("Selection Tool [Ctrl+A/D/I] (Right-click to change shape)");

            let active_lasso_tool = if app.active_tool() == ToolId::PolygonLasso {
                ToolId::PolygonLasso
            } else {
                ToolId::Lasso
            };
            let is_active =
                app.active_tool() == ToolId::Lasso || app.active_tool() == ToolId::PolygonLasso;
            let btn = egui::Button::new("").selected(is_active);
            let btn_resp = ui.add_sized([26.0, 26.0], btn);
            let icon_color = if is_active {
                egui::Color32::from_rgb(0, 120, 215)
            } else {
                ui.style().visuals.widgets.inactive.text_color()
            };
            let stroke = egui::Stroke::new(1.0, icon_color);
            let center = btn_resp.rect.center();
            let points = [
                center + egui::vec2(-6.0, -2.0),
                center + egui::vec2(-4.0, -6.0),
                center + egui::vec2(2.0, -6.0),
                center + egui::vec2(6.0, -1.0),
                center + egui::vec2(4.0, 5.0),
                center + egui::vec2(-1.0, 6.0),
                center + egui::vec2(-5.0, 3.0),
            ];
            if active_lasso_tool == ToolId::Lasso {
                for idx in 0..points.len() {
                    let p1 = points[idx];
                    let p2 = points[(idx + 1) % points.len()];
                    draw_dashed_line(ui.painter(), p1, p2, stroke);
                }
            } else {
                for idx in 0..points.len() {
                    let p1 = points[idx];
                    let p2 = points[(idx + 1) % points.len()];
                    draw_dashed_line(ui.painter(), p1, p2, stroke);
                    ui.painter().rect_filled(
                        egui::Rect::from_center_size(p1, egui::vec2(2.0, 2.0)),
                        0.0,
                        icon_color,
                    );
                }
            }
            if btn_resp.clicked() {
                app.set_active_tool(active_lasso_tool);
                ctx.request_repaint();
            }
            btn_resp.context_menu(|ui| {
                if ui
                    .selectable_label(app.active_tool() == ToolId::Lasso, "Free Lasso Selection")
                    .clicked()
                {
                    app.set_active_tool(ToolId::Lasso);
                    ui.close_menu();
                }
                if ui
                    .selectable_label(
                        app.active_tool() == ToolId::PolygonLasso,
                        "Polygon Lasso Selection [Shift+L]",
                    )
                    .clicked()
                {
                    app.set_active_tool(ToolId::PolygonLasso);
                    ui.close_menu();
                }
            });
            btn_resp.on_hover_text("Lasso Tool (Right-click to switch to Polygon)");

            let is_active = app.active_tool() == ToolId::MagicWand;
            let btn = egui::Button::new("").selected(is_active);
            let btn_resp = ui.add_sized([26.0, 26.0], btn);
            let icon_color = if is_active {
                egui::Color32::from_rgb(0, 120, 215)
            } else {
                ui.style().visuals.widgets.inactive.text_color()
            };
            let center = btn_resp.rect.center();
            let stroke_wand = egui::Stroke::new(1.8, icon_color);
            ui.painter().line_segment(
                [
                    center + egui::vec2(-6.0, 6.0),
                    center + egui::vec2(1.0, -1.0),
                ],
                stroke_wand,
            );
            ui.painter()
                .circle_filled(center + egui::vec2(1.0, -1.0), 1.5, icon_color);
            let tip = center + egui::vec2(1.0, -1.0);
            let sparkle_stroke = egui::Stroke::new(1.0, icon_color);
            ui.painter().line_segment(
                [tip + egui::vec2(4.0, -4.0), tip + egui::vec2(6.0, -6.0)],
                sparkle_stroke,
            );
            ui.painter().line_segment(
                [tip + egui::vec2(0.0, -5.0), tip + egui::vec2(0.0, -7.0)],
                sparkle_stroke,
            );
            ui.painter().line_segment(
                [tip + egui::vec2(5.0, 0.0), tip + egui::vec2(7.0, 0.0)],
                sparkle_stroke,
            );
            if btn_resp.clicked() {
                app.set_active_tool(ToolId::MagicWand);
                ctx.request_repaint();
            }
            btn_resp.on_hover_text("Magic Wand Selection");

            let is_active = app.active_tool() == ToolId::Transform;
            let btn = egui::Button::new("").selected(is_active);
            let btn_resp = ui.add_sized([26.0, 26.0], btn);
            let icon_color = if is_active {
                egui::Color32::from_rgb(0, 120, 215)
            } else {
                ui.style().visuals.widgets.inactive.text_color()
            };
            let r = btn_resp.rect.shrink(6.0);
            let stroke = egui::Stroke::new(1.0, icon_color);
            ui.painter().rect_stroke(r, 0.0, stroke);
            let h_size = egui::vec2(2.5, 2.5);
            let corners = [
                r.left_top(),
                r.right_top(),
                r.left_bottom(),
                r.right_bottom(),
                r.center_top(),
                r.center_bottom(),
                r.left_center(),
                r.right_center(),
            ];
            for &c in &corners {
                ui.painter()
                    .rect_filled(egui::Rect::from_center_size(c, h_size), 0.0, icon_color);
            }
            if btn_resp.clicked() {
                app.set_active_tool(ToolId::Transform);
                ctx.request_repaint();
            }
            btn_resp.on_hover_text("Transform Tool [Ctrl+T]");

            let btn = egui::Button::new("").selected(false);
            let btn_resp = ui.add_sized([26.0, 26.0], btn);
            let icon_color = ui.style().visuals.widgets.noninteractive.text_color();
            let center = btn_resp.rect.center();
            ui.painter().text(
                center,
                egui::Align2::CENTER_CENTER,
                "T",
                egui::FontId::new(14.0, egui::FontFamily::Proportional),
                icon_color,
            );
            btn_resp.on_hover_text("Text Tool (Not implemented)");
            ui.end_row();

            // ROW 2
            let is_active = app.active_tool() == ToolId::Move;
            let btn = egui::Button::new("").selected(is_active);
            let btn_resp = ui.add_sized([26.0, 26.0], btn);
            let icon_color = if is_active {
                egui::Color32::from_rgb(0, 120, 215)
            } else {
                ui.style().visuals.widgets.inactive.text_color()
            };
            let center = btn_resp.rect.center();
            let stroke = egui::Stroke::new(1.5, icon_color);
            ui.painter().line_segment(
                [center - egui::vec2(7.0, 0.0), center + egui::vec2(7.0, 0.0)],
                stroke,
            );
            ui.painter().line_segment(
                [center - egui::vec2(0.0, 7.0), center + egui::vec2(0.0, 7.0)],
                stroke,
            );
            ui.painter().line_segment(
                [
                    center - egui::vec2(7.0, 0.0),
                    center - egui::vec2(4.0, -3.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [center - egui::vec2(7.0, 0.0), center - egui::vec2(4.0, 3.0)],
                stroke,
            );
            ui.painter().line_segment(
                [
                    center + egui::vec2(7.0, 0.0),
                    center + egui::vec2(4.0, -3.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [center + egui::vec2(7.0, 0.0), center + egui::vec2(4.0, 3.0)],
                stroke,
            );
            ui.painter().line_segment(
                [
                    center - egui::vec2(0.0, 7.0),
                    center - egui::vec2(-3.0, 4.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [center - egui::vec2(0.0, 7.0), center - egui::vec2(3.0, 4.0)],
                stroke,
            );
            ui.painter().line_segment(
                [
                    center + egui::vec2(0.0, 7.0),
                    center + egui::vec2(-3.0, 4.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [center + egui::vec2(0.0, 7.0), center + egui::vec2(3.0, 4.0)],
                stroke,
            );
            if btn_resp.clicked() {
                app.set_active_tool(ToolId::Move);
                ctx.request_repaint();
            }
            btn_resp.on_hover_text("Move Layer Tool");

            let is_active = app.active_tool() == ToolId::Zoom;
            let btn = egui::Button::new("").selected(is_active);
            let btn_resp = ui.add_sized([26.0, 26.0], btn);
            let icon_color = if is_active {
                egui::Color32::from_rgb(0, 120, 215)
            } else {
                ui.style().visuals.widgets.inactive.text_color()
            };
            let center = btn_resp.rect.center();
            let stroke = egui::Stroke::new(1.5, icon_color);
            let lens_center = center + egui::vec2(-2.0, -2.0);
            ui.painter().circle_stroke(lens_center, 4.0, stroke);
            ui.painter().line_segment(
                [
                    lens_center + egui::vec2(2.8, 2.8),
                    center + egui::vec2(6.0, 6.0),
                ],
                egui::Stroke::new(2.5, icon_color),
            );
            if btn_resp.clicked() {
                app.set_active_tool(ToolId::Zoom);
                ctx.request_repaint();
            }
            btn_resp.on_hover_text("Zoom Canvas");

            let is_active = app.active_tool() == ToolId::RotateView;
            let btn = egui::Button::new("").selected(is_active);
            let btn_resp = ui.add_sized([26.0, 26.0], btn);
            let icon_color = if is_active {
                egui::Color32::from_rgb(0, 120, 215)
            } else {
                ui.style().visuals.widgets.inactive.text_color()
            };
            let center = btn_resp.rect.center();
            let stroke = egui::Stroke::new(1.5, icon_color);
            let radius = 5.5;
            let tip = center + egui::vec2(0.0, -radius);
            let steps = 12;
            for i in 0..steps {
                let a1 = -std::f32::consts::FRAC_PI_2
                    + (i as f32 / steps as f32) * (1.5 * std::f32::consts::PI);
                let a2 = -std::f32::consts::FRAC_PI_2
                    + ((i + 1) as f32 / steps as f32) * (1.5 * std::f32::consts::PI);
                let p1 = center + egui::vec2(a1.cos(), a1.sin()) * radius;
                let p2 = center + egui::vec2(a2.cos(), a2.sin()) * radius;
                ui.painter().line_segment([p1, p2], stroke);
            }
            ui.painter()
                .line_segment([tip, tip + egui::vec2(-3.0, -3.0)], stroke);
            ui.painter()
                .line_segment([tip, tip + egui::vec2(-3.0, 3.0)], stroke);
            if btn_resp.clicked() {
                app.set_active_tool(ToolId::RotateView);
                ctx.request_repaint();
            }
            btn_resp.on_hover_text("Rotate View [R]");

            let is_active = app.active_tool() == ToolId::Hand;
            let btn = egui::Button::new("").selected(is_active);
            let btn_resp = ui.add_sized([26.0, 26.0], btn);
            let icon_color = if is_active {
                egui::Color32::from_rgb(0, 120, 215)
            } else {
                ui.style().visuals.widgets.inactive.text_color()
            };
            let center = btn_resp.rect.center();
            let stroke = egui::Stroke::new(1.2, icon_color);
            let p_base = center + egui::vec2(0.0, 3.0);
            ui.painter().line_segment(
                [p_base - egui::vec2(4.0, 0.0), p_base + egui::vec2(4.0, 0.0)],
                stroke,
            );
            ui.painter().line_segment(
                [
                    center + egui::vec2(-3.0, 3.0),
                    center + egui::vec2(-3.0, -4.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    center + egui::vec2(-1.0, 3.0),
                    center + egui::vec2(-1.0, -6.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    center + egui::vec2(1.0, 3.0),
                    center + egui::vec2(1.0, -5.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    center + egui::vec2(3.0, 3.0),
                    center + egui::vec2(3.0, -3.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    center + egui::vec2(-3.0, 1.0),
                    center + egui::vec2(-5.0, -1.0),
                ],
                stroke,
            );
            if btn_resp.clicked() {
                app.set_active_tool(ToolId::Hand);
                ctx.request_repaint();
            }
            btn_resp.on_hover_text("Hand Panning Tool [Space]");

            let is_active = app.active_tool() == ToolId::ColorPicker;
            let btn = egui::Button::new("").selected(is_active);
            let btn_resp = ui.add_sized([26.0, 26.0], btn);
            let icon_color = if is_active {
                egui::Color32::from_rgb(0, 120, 215)
            } else {
                ui.style().visuals.widgets.inactive.text_color()
            };
            let center = btn_resp.rect.center();
            let stroke = egui::Stroke::new(1.5, icon_color);
            ui.painter().line_segment(
                [
                    center + egui::vec2(5.0, -5.0),
                    center + egui::vec2(-1.0, 1.0),
                ],
                stroke,
            );
            ui.painter()
                .circle_filled(center + egui::vec2(5.0, -5.0), 2.5, icon_color);
            ui.painter().line_segment(
                [
                    center + egui::vec2(-1.0, 1.0),
                    center + egui::vec2(-4.0, 4.0),
                ],
                stroke,
            );
            if btn_resp.clicked() {
                app.set_active_tool(ToolId::ColorPicker);
                ctx.request_repaint();
            }
            btn_resp.on_hover_text("Color Picker (Eyedropper) [Alt/I]");
            ui.end_row();

            // ROW 3
            // 1. Fill Tool
            let is_active = app.active_tool() == ToolId::Fill;
            let btn = egui::Button::new("").selected(is_active);
            let btn_resp = ui.add_sized([26.0, 26.0], btn);
            let icon_color = if is_active {
                egui::Color32::from_rgb(0, 120, 215)
            } else {
                ui.style().visuals.widgets.inactive.text_color()
            };
            let center = btn_resp.rect.center();
            let stroke = egui::Stroke::new(1.5, icon_color);
            ui.painter().line_segment([center + egui::vec2(-4.0, 2.0), center + egui::vec2(2.0, -4.0)], stroke);
            ui.painter().line_segment([center + egui::vec2(-4.0, 2.0), center + egui::vec2(-1.0, 5.0)], stroke);
            ui.painter().line_segment([center + egui::vec2(-1.0, 5.0), center + egui::vec2(5.0, -1.0)], stroke);
            ui.painter().line_segment([center + egui::vec2(5.0, -1.0), center + egui::vec2(2.0, -4.0)], stroke);
            ui.painter().circle_stroke(center + egui::vec2(-4.0, -2.0), 3.0, egui::Stroke::new(1.0, icon_color));
            if btn_resp.clicked() {
                app.set_active_tool(ToolId::Fill);
                ctx.request_repaint();
            }
            btn_resp.on_hover_text("Fill Tool [G]");

            // 2. Gradient Tool
            let is_active = app.active_tool() == ToolId::Gradient;
            let btn = egui::Button::new("").selected(is_active);
            let btn_resp = ui.add_sized([26.0, 26.0], btn);
            let icon_color = if is_active {
                egui::Color32::from_rgb(0, 120, 215)
            } else {
                ui.style().visuals.widgets.inactive.text_color()
            };
            let center = btn_resp.rect.center();
            let stroke = egui::Stroke::new(1.5, icon_color);
            ui.painter().line_segment([center - egui::vec2(5.0, 0.0), center + egui::vec2(5.0, 0.0)], stroke);
            ui.painter().circle_filled(center - egui::vec2(5.0, 0.0), 2.5, icon_color);
            ui.painter().circle_stroke(center + egui::vec2(5.0, 0.0), 2.5, egui::Stroke::new(1.0, icon_color));
            if btn_resp.clicked() {
                app.set_active_tool(ToolId::Gradient);
                ctx.request_repaint();
            }
            btn_resp.on_hover_text("Gradient Tool [Shift+G]");

            // 3, 4, 5. Empty space cells
            ui.allocate_space(egui::vec2(26.0, 26.0));
            ui.allocate_space(egui::vec2(26.0, 26.0));
            ui.allocate_space(egui::vec2(26.0, 26.0));
            ui.end_row();
        });
    ui.separator();
    ui.label("VECTOR TOOLS");
    egui::Grid::new("vector_tools_grid")
        .num_columns(3)
        .spacing([4.0, 4.0])
        .show(ui, |ui| {
            let vec_tools: [(ToolId, &str, &str); 3] = [
                (
                    ToolId::VectorPen,
                    "✎",
                    "Vector Pen — draw smooth vector strokes",
                ),
                (
                    ToolId::Curve,
                    "〰",
                    "Curve — place 4 control points for a bezier curve",
                ),
                (
                    ToolId::EditCP,
                    "⬩",
                    "Edit CP — select and drag control points",
                ),
            ];
            for &(tool_id, label, tooltip) in &vec_tools {
                let is_active = app.active_tool() == tool_id;
                let btn =
                    egui::Button::new(egui::RichText::new(label).size(12.0)).selected(is_active);
                let r = ui.add_sized([26.0, 26.0], btn).on_hover_text(tooltip);
                if r.clicked() {
                    app.set_active_tool(tool_id);
                    if tool_id == ToolId::VectorPen {
                        let is_vector = app
                            .layers
                            .get(&app.active_layer_id)
                            .map(|l| l.kind == crate::canvas::LayerType::Vector)
                            .unwrap_or(false);
                        if !is_vector {
                            app.create_vector_layer();
                        }
                    }
                    ctx.request_repaint();
                }
            }
        });
    ui.separator();
    ui.label("BRUSH PRESETS");
    ui.dnd_drop_zone::<usize, _>(egui::Frame::none(), |ui| {
        egui::Grid::new("presets_grid")
            .num_columns(4)
            .spacing([4.0, 4.0])
            .show(ui, |ui| {
                let num_presets = app.presets.len();
                for i in 0..16 {
                    if i < num_presets {
                        let preset_icon = app.presets[i].icon;
                        let preset_name = app.presets[i].name.clone();
                        let is_selected = app.active_preset_index == i
                            && matches!(app.active_tool(), ToolId::Brush | ToolId::Eraser);

                        let type_tag = match preset_icon {
                            PresetIcon::Pencil => "P",
                            PresetIcon::InkPen => "I",
                            PresetIcon::PaintBrush => "B",
                            PresetIcon::Smudge => "S",
                            PresetIcon::Eraser => "E",
                            PresetIcon::AirBrush => "A",
                            PresetIcon::Water => "W",
                            PresetIcon::Marker => "M",
                            PresetIcon::BinaryPen => "1",
                        };

                        let id = egui::Id::new("preset_dnd").with(i);
                        let response = ui.dnd_drag_source(id, i, |ui| {
                            let (rect, btn_response) = ui
                                .allocate_exact_size(egui::vec2(34.0, 30.0), egui::Sense::click());
                            let (bg_color, stroke_color, text_color) = if is_selected {
                                (
                                    egui::Color32::from_rgb(215, 225, 255),
                                    egui::Color32::from_rgb(120, 150, 255),
                                    egui::Color32::from_rgb(0, 50, 150),
                                )
                            } else if btn_response.hovered() {
                                (
                                    egui::Color32::from_gray(245),
                                    egui::Color32::from_gray(200),
                                    egui::Color32::BLACK,
                                )
                            } else {
                                (
                                    egui::Color32::WHITE,
                                    egui::Color32::from_gray(225),
                                    egui::Color32::from_gray(60),
                                )
                            };

                            ui.painter().rect_filled(rect, 2.0, bg_color);
                            ui.painter().rect_stroke(
                                rect,
                                2.0,
                                egui::Stroke::new(1.0, stroke_color),
                            );

                            let name_len = preset_name.len();
                            let font_size = if name_len > 8 {
                                5.5
                            } else if name_len > 6 {
                                6.5
                            } else {
                                8.0
                            };
                            let name_font = egui::FontId::proportional(font_size);
                            let display_name = if name_len > 9 {
                                format!("{}..", &preset_name[0..7])
                            } else {
                                preset_name.clone()
                            };
                            let name_pos = rect.min + egui::vec2(3.0, 9.0);
                            ui.painter().text(
                                name_pos,
                                egui::Align2::LEFT_BOTTOM,
                                display_name,
                                name_font,
                                text_color,
                            );

                            let icon_font = egui::FontId::proportional(8.0);
                            let icon_pos = rect.max - egui::vec2(3.0, 3.0);
                            ui.painter().text(
                                icon_pos,
                                egui::Align2::RIGHT_BOTTOM,
                                type_tag,
                                icon_font,
                                text_color,
                            );
                            btn_response
                        });

                        if response.inner.clicked() {
                            app.select_preset(i);
                            response.inner.surrender_focus();
                        }

                        response.inner.context_menu(|ui| {
                            if ui.button("Rename").clicked() {
                                app.renaming_preset_index = Some(i);
                                app.rename_input = preset_name.clone();
                                ui.close_menu();
                            }
                            if ui.button("Duplicate").clicked() {
                                app.duplicate_preset(i);
                                ui.close_menu();
                            }
                            ui.separator();
                            let can_delete = num_presets > 1;
                            if ui
                                .add_enabled(can_delete, egui::Button::new("Delete"))
                                .clicked()
                            {
                                app.delete_preset(i);
                                ui.close_menu();
                            }
                        });

                        if let Some(source_idx) = response.response.dnd_hover_payload::<usize>() {
                            let source_idx = *source_idx;
                            if source_idx != i {
                                let rect = response.response.rect;
                                if let Some(hover_pos) = ui.input(|i| i.pointer.hover_pos()) {
                                    let is_left = hover_pos.x < rect.center().x;
                                    let line_segment_x =
                                        if is_left { rect.left() } else { rect.right() };
                                    ui.painter().line_segment(
                                        [
                                            egui::pos2(line_segment_x, rect.top()),
                                            egui::pos2(line_segment_x, rect.bottom()),
                                        ],
                                        egui::Stroke::new(
                                            2.5,
                                            egui::Color32::from_rgb(0, 120, 215),
                                        ),
                                    );
                                }
                            }
                        }

                        if let Some(source_idx) = response.response.dnd_release_payload::<usize>() {
                            let source_idx = *source_idx;
                            if source_idx != i {
                                if let Some(hover_pos) = response.response.interact_pointer_pos() {
                                    let is_left = hover_pos.x < response.response.rect.center().x;
                                    let mut target_idx = i;
                                    if !is_left {
                                        target_idx += 1;
                                    }
                                    app.reorder_preset(source_idx, target_idx);
                                }
                            }
                        }
                    } else {
                        let (rect, btn_response) =
                            ui.allocate_exact_size(egui::vec2(34.0, 30.0), egui::Sense::click());
                        let bg_color = if btn_response.hovered() {
                            egui::Color32::from_gray(245)
                        } else {
                            egui::Color32::WHITE
                        };
                        ui.painter().rect_filled(rect, 2.0, bg_color);
                        ui.painter().rect_stroke(
                            rect,
                            1.0,
                            egui::Stroke::new(1.0, egui::Color32::from_gray(225)),
                        );
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "+",
                            egui::FontId::proportional(14.0),
                            egui::Color32::GRAY,
                        );

                        let mut show_creation_menu = false;
                        if btn_response.clicked() {
                            show_creation_menu = true;
                        }
                        btn_response.context_menu(|ui| {
                            ui.label("Create New Brush:");
                            if ui.button("Pencil").clicked() {
                                app.create_preset(PresetIcon::Pencil);
                                ui.close_menu();
                            }
                            if ui.button("Ink Pen").clicked() {
                                app.create_preset(PresetIcon::InkPen);
                                ui.close_menu();
                            }
                            if ui.button("Paint Brush").clicked() {
                                app.create_preset(PresetIcon::PaintBrush);
                                ui.close_menu();
                            }
                            if ui.button("Smudge").clicked() {
                                app.create_preset(PresetIcon::Smudge);
                                ui.close_menu();
                            }
                            if ui.button("Eraser").clicked() {
                                app.create_preset(PresetIcon::Eraser);
                                ui.close_menu();
                            }
                            ui.separator();
                            ui.label("Import Brush Preset:");
                            ui.horizontal(|ui| {
                                ui.label("Path:");
                                ui.text_edit_singleline(&mut app.brush_import_path);
                            });
                            if ui.button("Load .artybrush").clicked() {
                                let path = std::path::Path::new(&app.brush_import_path);
                                match crate::brush_io::load_artybrush(path, &mut app.brush_textures)
                                {
                                    Ok(mut new_preset) => {
                                        app.preset_id_counter += 1;
                                        new_preset.id = app.preset_id_counter;
                                        let mut brush = Brush::new();
                                        PaintApp::set_constant(
                                            &mut brush,
                                            BrushSetting::Radius,
                                            new_preset.radius_log,
                                        );
                                        PaintApp::set_constant(
                                            &mut brush,
                                            BrushSetting::Opaque,
                                            new_preset.opacity,
                                        );
                                        PaintApp::set_constant(
                                            &mut brush,
                                            BrushSetting::Hardness,
                                            new_preset.hardness,
                                        );
                                        PaintApp::set_constant(
                                            &mut brush,
                                            BrushSetting::Smudge,
                                            new_preset.color_blending,
                                        );
                                        PaintApp::set_constant(
                                            &mut brush,
                                            BrushSetting::SmudgeLength,
                                            new_preset.dilution,
                                        );
                                        if new_preset.is_eraser {
                                            PaintApp::set_constant(
                                                &mut brush,
                                                BrushSetting::Eraser,
                                                1.0,
                                            );
                                        }
                                        app.presets.push(new_preset);
                                        app.brushes.push(brush);
                                        app.brush_states.push(BrushState::default());
                                        let new_idx = app.presets.len() - 1;
                                        app.select_preset(new_idx);
                                        log::info!("Imported .artybrush successfully!");
                                    }
                                    Err(e) => log::error!("Failed to import .artybrush: {:?}", e),
                                }
                                ui.close_menu();
                            }
                            if ui.button("⚡ Extract & Import .sut").clicked() {
                                let path = std::path::Path::new(&app.brush_import_path);
                                match crate::brush_io::extract_sut_texture(path) {
                                    Ok((gray_bytes, w, h)) => {
                                        let mut final_bytes = vec![255u8; 256 * 256];
                                        for y in 0..h.min(256) {
                                            for x in 0..w.min(256) {
                                                final_bytes[(y * 256 + x) as usize] =
                                                    gray_bytes[(y * w + x) as usize];
                                            }
                                        }
                                        let name = path
                                            .file_stem()
                                            .and_then(|s| s.to_str())
                                            .unwrap_or("SUT Brush")
                                            .to_string();
                                        app.brush_textures.push(crate::app::BrushTexture {
                                            name: format!("[sut] {}", name),
                                            width: 256,
                                            height: 256,
                                            pixels: final_bytes,
                                        });
                                        let texture_id = (app.brush_textures.len() - 1) as u32;
                                        app.preset_id_counter += 1;
                                        let new_preset = crate::app::BrushPreset {
                                            id: app.preset_id_counter,
                                            name: path
                                                .file_stem()
                                                .and_then(|s| s.to_str())
                                                .unwrap_or("SUT Brush")
                                                .to_string(),
                                            icon: PresetIcon::PaintBrush,
                                            radius_log: 2.0,
                                            opacity: 1.0,
                                            hardness: 0.8,
                                            min_size_fraction: 0.2,
                                            color_blending: 0.0,
                                            dilution: 0.0,
                                            is_eraser: false,
                                            texture_id,
                                            texture_scale: 1.0,
                                            bristle_id: 0,
                                            stabilizer_level: StabilizerLevel::default(),
                                            stabilizer_mode: StabilizerMode::SpringMassDamper,
                                            spacing: 2.0,
                                            density: 1.0,
                                        };
                                        let mut brush = Brush::new();
                                        PaintApp::set_constant(
                                            &mut brush,
                                            BrushSetting::Radius,
                                            new_preset.radius_log,
                                        );
                                        PaintApp::set_constant(
                                            &mut brush,
                                            BrushSetting::Opaque,
                                            new_preset.opacity,
                                        );
                                        PaintApp::set_constant(
                                            &mut brush,
                                            BrushSetting::Hardness,
                                            new_preset.hardness,
                                        );
                                        app.presets.push(new_preset);
                                        app.brushes.push(brush);
                                        app.brush_states.push(BrushState::default());
                                        let new_idx = app.presets.len() - 1;
                                        app.select_preset(new_idx);
                                        log::info!(
                                            "Extracted and imported SUT brush successfully!"
                                        );
                                    }
                                    Err(e) => log::error!("Failed to extract SUT: {:?}", e),
                                }
                                ui.close_menu();
                            }
                        });
                        if show_creation_menu {
                            ui.ctx().memory_mut(|mem| {
                                mem.open_popup(btn_response.id.with("context_menu"))
                            });
                        }
                    }
                    if i % 4 == 3 {
                        ui.end_row();
                    }
                }
            });
    });

    if let Some(idx) = app.renaming_preset_index {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Rename:");
            let res =
                ui.add(egui::TextEdit::singleline(&mut app.rename_input).desired_width(100.0));
            if res.lost_focus() || ui.button("OK").clicked() {
                if !app.rename_input.trim().is_empty() {
                    app.presets[idx].name = app.rename_input.trim().to_string();
                }
                app.renaming_preset_index = None;
            }
            if ui.button("✕").on_hover_text("Cancel rename").clicked() {
                app.renaming_preset_index = None;
            }
        });
    }

    ui.add_space(3.0);
}

pub(crate) fn draw_brush_settings_content(
    app: &mut PaintApp,
    ui: &mut egui::Ui,
    _ctx: &egui::Context,
) {
    if matches!(app.active_tool(), ToolId::Brush | ToolId::Eraser) {
        // --- 1. BRUSH STROKE PREVIEW ---
        let preview_height = 56.0;
        let (preview_resp, preview_painter) = ui.allocate_painter(
            egui::vec2(ui.available_width(), preview_height),
            egui::Sense::hover(),
        );
        let pr = preview_resp.rect;
        // Checkerboard
        let cell = 6.0;
        let cols = ((pr.width() / cell).ceil() as i32).max(1);
        let rows = ((pr.height() / cell).ceil() as i32).max(1);
        for yi in 0..rows {
            for xi in 0..cols {
                if (xi + yi) % 2 == 1 {
                    preview_painter.rect_filled(
                        egui::Rect::from_min_size(
                            egui::pos2(pr.min.x + xi as f32 * cell, pr.min.y + yi as f32 * cell),
                            egui::Vec2::splat(cell),
                        ),
                        0.0,
                        egui::Color32::from_gray(220),
                    );
                }
            }
        }
        preview_painter.rect_stroke(
            pr,
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(160)),
        );

        // Draw stylized brush stroke (pressure-sensitive magenta stroke)
        let num_dabs = 50;
        let max_r = (2.0 + app.brush_radius_log.max(0.0) * 3.0)
            .min(14.0)
            .max(1.5);
        let stroke_opacity = app.brush_opacity;
        for i in 0..num_dabs {
            let t = i as f32 / (num_dabs - 1) as f32;
            let pressure = (t * std::f32::consts::PI).sin().max(0.01);
            let r = (0.8 + pressure * max_r * app.brush_density).max(0.3);
            let x = pr.min.x + 8.0 + t * (pr.width() - 16.0);
            let y = pr.center().y + (t - 0.5).sin() * 10.0;
            let alpha = ((1.0 - app.brush_hardness * (1.0 - pressure) * 0.4)
                * stroke_opacity
                * 255.0) as u8;
            let col = egui::Color32::from_rgba_unmultiplied(220, 60, 100, alpha);
            preview_painter.circle_filled(egui::pos2(x, y), r, col);
        }
        // Also draw a thin stroke path in darker red
        for i in 0..num_dabs - 1 {
            let t1 = i as f32 / (num_dabs - 1) as f32;
            let t2 = (i + 1) as f32 / (num_dabs - 1) as f32;
            let p1 = egui::pos2(
                pr.min.x + 8.0 + t1 * (pr.width() - 16.0),
                pr.center().y + (t1 - 0.5).sin() * 10.0,
            );
            let p2 = egui::pos2(
                pr.min.x + 8.0 + t2 * (pr.width() - 16.0),
                pr.center().y + (t2 - 0.5).sin() * 10.0,
            );
            preview_painter.line_segment(
                [p1, p2],
                egui::Stroke::new(
                    0.5,
                    egui::Color32::from_rgba_unmultiplied(
                        180,
                        30,
                        70,
                        (stroke_opacity * 255.0) as u8,
                    ),
                ),
            );
        }

        ui.add_space(2.0);

        // --- 2. BRUSH MODE AND BRUSH TIP ---
        ui.horizontal(|ui| {
            ui.label("Mode:");
            egui::ComboBox::from_id_source("blend_mode")
                .selected_text("Normal")
                .width(80.0)
                .show_ui(ui, |ui| {
                    let _ = ui.selectable_label(true, "Normal");
                });
        });
        ui.horizontal(|ui| {
            ui.label("Tip:");
            let tip_types: [(&str, &str, bool); 4] = [
                ("◯", "Soft Round", app.brush_hardness < 0.6),
                (
                    "●",
                    "Hard Round",
                    app.brush_hardness >= 0.6 && app.brush_hardness < 0.9,
                ),
                (
                    "⬟",
                    "Marker",
                    app.brush_hardness >= 0.9 && app.brush_hardness < 1.0,
                ),
                ("■", "Square", app.brush_hardness >= 1.0),
            ];
            let mut selected_tip_idx = 0;
            for (idx, &(_icon, _name, is_sel)) in tip_types.iter().enumerate() {
                let is_selected = is_sel || (idx == 0 && tip_types.iter().all(|t| !t.2));
                if is_selected {
                    selected_tip_idx = idx;
                }
            }
            let tip_names = ["Soft Round", "Hard Round", "Marker", "Square"];
            for (idx, &(icon, _name, _)) in tip_types.iter().enumerate() {
                let is_sel = idx == selected_tip_idx;
                let btn = egui::Button::new(egui::RichText::new(icon).size(14.0)).selected(is_sel);
                if ui
                    .add_sized([24.0, 22.0], btn)
                    .on_hover_text(tip_names[idx])
                    .clicked()
                {
                    match idx {
                        0 => app.brush_hardness = 0.3,
                        1 => app.brush_hardness = 0.8,
                        2 => app.brush_hardness = 0.95,
                        3 => app.brush_hardness = 1.0,
                        _ => {}
                    }
                    app.brush_settings_dirty = true;
                }
            }
        });

        ui.add_space(2.0);

        // --- 3. BRUSH SIZE / MIN SIZE / DENSITY ---
        let pixel_radius = app.brush_radius_log.exp();
        ui.horizontal(|ui| {
            ui.label("Brush Size");
            if ui.add(egui::Slider::new(&mut app.brush_radius_log, -1.0..=5.0).show_value(false)).changed() {
                app.brush_settings_dirty = true;
            }
            let mut size_display = pixel_radius;
            if ui
                .add(
                    egui::DragValue::new(&mut size_display)
                        .speed(0.1)
                        .suffix("")
                        .clamp_range(0.1..=150.0),
                )
                .changed()
            {
                app.brush_radius_log = size_display.max(0.001).ln();
                app.brush_settings_dirty = true;
            }
        });

        // Min Size
        ui.horizontal(|ui| {
            ui.label("Min Size");
            if ui.add(
                egui::Slider::new(&mut app.brush_min_size_fraction, 0.0..=1.0).show_value(false),
            ).changed() {
                app.brush_settings_dirty = true;
            }
            if ui
                .add(
                    egui::DragValue::new(&mut app.brush_min_size_fraction)
                        .speed(0.01)
                        .suffix("%"),
                )
                .changed()
            {
                app.brush_min_size_fraction = app.brush_min_size_fraction.clamp(0.0, 1.0);
                app.brush_settings_dirty = true;
            }
        });

        // Density
        ui.horizontal(|ui| {
            ui.label("Density");
            if ui.add(egui::Slider::new(&mut app.brush_density, 0.0..=1.0).show_value(false)).changed() {
                app.brush_settings_dirty = true;
            }
            if ui
                .add(
                    egui::DragValue::new(&mut app.brush_density)
                        .speed(0.01)
                        .suffix("%"),
                )
                .changed()
            {
                app.brush_density = app.brush_density.clamp(0.0, 1.0);
                app.brush_settings_dirty = true;
            }
        });

        // Min Density (new SAI-style control - maps to opacity minimum behavior)
        let mut min_density = app.brush_opacity.min(0.1) * 100.0;
        ui.horizontal(|ui| {
            ui.label("Min Density");
            ui.add_enabled(
                false,
                egui::Slider::new(&mut min_density, 0.0..=100.0).show_value(false),
            );
            ui.add_enabled(
                false,
                egui::DragValue::new(&mut min_density)
                    .speed(1.0)
                    .suffix("%"),
            );
        });

        ui.add_space(2.0);

        // --- 4. BRUSH SHAPE SECTION ---
        egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            ui.next_auto_id(),
            true,
        )
        .show_header(ui, |ui| {
            ui.label("[ Simple Circle ]");
        })
        .body(|ui| {
            ui.horizontal(|ui| {
                ui.label("Shape:");
                ui.label("Simple Circle");
            });
            ui.horizontal(|ui| {
                ui.label("Scale:");
                let mut scale = 100.0;
                ui.add_enabled(
                    false,
                    egui::Slider::new(&mut scale, 10.0..=500.0).show_value(false),
                );
                ui.add_enabled(
                    false,
                    egui::DragValue::new(&mut scale).speed(1.0).suffix("%"),
                );
            });
            ui.checkbox(&mut false, "Invert");
            ui.checkbox(&mut false, "Invert for Transparency");
            ui.add_enabled(false, egui::Checkbox::new(&mut false, "Sharpen"));
        });

        ui.add_space(1.0);

        // --- 5. BRUSH TEXTURE SECTION ---
        let texture_name = app
            .brush_textures
            .get(app.brush_texture_id as usize)
            .map(|t| t.name.as_str())
            .unwrap_or("None");
        let has_texture = app.brush_texture_id > 0;
        egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            ui.next_auto_id(),
            has_texture,
        )
        .show_header(ui, |ui| {
            ui.label(format!(
                "[ {} ]",
                if has_texture {
                    texture_name
                } else {
                    "No Texture"
                }
            ));
        })
        .body(|ui| {
            ui.horizontal(|ui| {
                ui.label("Texture:");
                let mut selected_tex = app.brush_texture_id;
                let res = egui::ComboBox::from_id_source("brush_texture_combo_sai")
                    .selected_text(texture_name)
                    .width(100.0)
                    .show_ui(ui, |ui| {
                        let mut changed = false;
                        for (idx, tex) in app.brush_textures.iter().enumerate() {
                            if ui
                                .selectable_value(&mut selected_tex, idx as u32, &tex.name)
                                .clicked()
                            {
                                changed = true;
                            }
                        }
                        changed
                    });
                if res.inner.unwrap_or(false) {
                    app.brush_texture_id = selected_tex;
                    app.brush_settings_dirty = true;
                }
            });
            if has_texture {
                ui.horizontal(|ui| {
                    ui.label("Intensity");
                    let mut intensity = (app.brush_opacity * 100.0) as f32;
                    ui.add(egui::Slider::new(&mut intensity, 0.0..=100.0).show_value(false));
                    ui.add(egui::DragValue::new(&mut intensity).speed(1.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Scale");
                    ui.add(
                        egui::Slider::new(&mut app.brush_texture_scale, 0.1..=10.0)
                            .show_value(false),
                    );
                    ui.add(egui::DragValue::new(&mut app.brush_texture_scale).speed(0.1));
                });
            } else {
                ui.horizontal(|ui| {
                    ui.label("Intensity");
                    let mut dummy = 95.0;
                    ui.add_enabled(
                        false,
                        egui::Slider::new(&mut dummy, 0.0..=100.0).show_value(false),
                    );
                    ui.add_enabled(false, egui::DragValue::new(&mut dummy).speed(1.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Scale");
                    let mut dummy_s = 100.0;
                    ui.add_enabled(
                        false,
                        egui::Slider::new(&mut dummy_s, 10.0..=500.0).show_value(false),
                    );
                    ui.add_enabled(
                        false,
                        egui::DragValue::new(&mut dummy_s).speed(1.0).suffix("%"),
                    );
                });
            }
            ui.checkbox(&mut false, "Scratch");
            if has_texture {
                ui.checkbox(&mut false, "Invert");
                ui.checkbox(&mut false, "Invert for Transparency");
            } else {
                ui.add_enabled(false, egui::Checkbox::new(&mut false, "Invert"));
                ui.add_enabled(
                    false,
                    egui::Checkbox::new(&mut false, "Invert for Transparency"),
                );
            }
        });

        ui.add_space(1.0);

        // --- 6. BLENDING SECTION ---
        egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            ui.next_auto_id(),
            true,
        )
        .show_header(ui, |ui| {
            ui.label("Blending");
        })
        .body(|ui| {
            ui.horizontal(|ui| {
                ui.label("Blending");
                if ui
                    .add(
                        egui::Slider::new(&mut app.brush_color_blending, 0.0..=1.0)
                            .show_value(false),
                    )
                    .changed()
                {
                    app.brush_settings_dirty = true;
                }
                if ui
                    .add(egui::DragValue::new(&mut app.brush_color_blending).speed(0.01))
                    .changed()
                {
                    app.brush_settings_dirty = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Persistence");
                if ui
                    .add(egui::Slider::new(&mut app.brush_dilution, 0.0..=1.0).show_value(false))
                    .changed()
                {
                    app.brush_settings_dirty = true;
                }
                if ui
                    .add(egui::DragValue::new(&mut app.brush_dilution).speed(0.01))
                    .changed()
                {
                    app.brush_settings_dirty = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Opacity");
                if ui
                    .add(egui::Slider::new(&mut app.brush_opacity, 0.0..=1.0).show_value(false))
                    .changed()
                {
                    app.brush_settings_dirty = true;
                }
                let mut opacity_pct = (app.brush_opacity * 100.0) as i32;
                if ui
                    .add(
                        egui::DragValue::new(&mut opacity_pct)
                            .speed(1)
                            .clamp_range(0..=100),
                    )
                    .changed()
                {
                    app.brush_opacity = (opacity_pct as f32 / 100.0).clamp(0.0, 1.0);
                    app.brush_settings_dirty = true;
                }
            });
        });

        ui.add_space(1.0);

        // --- 7. MISCELLANEOUS SECTION ---
        egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            ui.next_auto_id(),
            true,
        )
        .show_header(ui, |ui| {
            ui.label("Miscellaneous");
        })
        .body(|ui| {
            ui.horizontal(|ui| {
                ui.label("Sharpness");
                if ui
                    .add(egui::Slider::new(&mut app.brush_hardness, 0.0..=1.0).show_value(false))
                    .changed()
                {
                    app.brush_settings_dirty = true;
                }
                if ui
                    .add(egui::DragValue::new(&mut app.brush_hardness).speed(0.01))
                    .changed()
                {
                    app.brush_settings_dirty = true;
                }
            });
            let mut amplify = 50.0;
            ui.horizontal(|ui| {
                ui.label("Amplify Dens");
                ui.add_enabled(
                    false,
                    egui::Slider::new(&mut amplify, 0.0..=100.0).show_value(false),
                );
                ui.add_enabled(false, egui::DragValue::new(&mut amplify).speed(1.0));
            });
            ui.horizontal(|ui| {
                ui.label("Spacing");
                if ui
                    .add(egui::Slider::new(&mut app.brush_spacing, 0.5..=10.0).show_value(false))
                    .changed()
                {
                    app.brush_settings_dirty = true;
                }
                if ui
                    .add(egui::DragValue::new(&mut app.brush_spacing).speed(0.1))
                    .changed()
                {
                    app.brush_settings_dirty = true;
                }
            });
            if !app.presets.is_empty() {
                let is_eraser = &mut app.presets[app.active_preset_index].is_eraser;
                if ui.checkbox(is_eraser, "Eraser Mode").changed() {
                    app.brush_settings_dirty = true;
                }
            }
            ui.checkbox(&mut app.lock_canvas_bounds, "Lock Canvas Bounds");
            ui.horizontal(|ui| {
                ui.label("Bristle ID");
                if ui
                    .add(
                        egui::DragValue::new(&mut app.brush_bristle_id)
                            .speed(1)
                            .clamp_range(0..=5),
                    )
                    .changed()
                {
                    app.brush_settings_dirty = true;
                }
            });
            ui.add_enabled(false, egui::Checkbox::new(&mut false, "Ver1 Prs Spec"));
            let mut anti_ripple = matches!(
                app.stabilizer.level,
                StabilizerLevel::Level(_) | StabilizerLevel::SLevel(_)
            );
            if ui.checkbox(&mut anti_ripple, "Anti-Ripple").changed() {
                app.stabilizer.set_level(if anti_ripple {
                    StabilizerLevel::Level(8)
                } else {
                    StabilizerLevel::Off
                });
                app.brush_settings_dirty = true;
            }
        });
    } // if brush/eraser
}

pub(crate) fn draw_stabilizer_content(app: &mut PaintApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    if !matches!(app.active_tool(), ToolId::Brush | ToolId::Eraser) {
        ui.colored_label(
            egui::Color32::from_gray(120),
            "Active with Brush/Eraser only",
        );
        return;
    }
    let _stabilizer_on = !matches!(app.stabilizer.level, StabilizerLevel::Off);
    ui.horizontal(|ui| {
        ui.label("Level");
        let current_level = app.stabilizer.level;
        let text = match current_level {
            StabilizerLevel::Off => "Off".to_string(),
            StabilizerLevel::Level(val) => format!("Level {}", val),
            StabilizerLevel::SLevel(val) => format!("S-{}", val),
        };
        let response = egui::ComboBox::from_id_source("sai_stabilizer_level")
            .selected_text(text)
            .width(80.0)
            .show_ui(ui, |ui| {
                let mut selected = false;
                if ui
                    .selectable_label(matches!(current_level, StabilizerLevel::Off), "Off")
                    .clicked()
                {
                    app.stabilizer.set_level(StabilizerLevel::Off);
                    selected = true;
                }
                for val in 1..=15 {
                    let is_sel = matches!(current_level, StabilizerLevel::Level(v) if v == val);
                    if ui
                        .selectable_label(is_sel, format!("Level {}", val))
                        .clicked()
                    {
                        app.stabilizer.set_level(StabilizerLevel::Level(val));
                        selected = true;
                    }
                }
                for val in 1..=5 {
                    let is_sel = matches!(current_level, StabilizerLevel::SLevel(v) if v == val);
                    if ui.selectable_label(is_sel, format!("S-{}", val)).clicked() {
                        app.stabilizer.set_level(StabilizerLevel::SLevel(val));
                        selected = true;
                    }
                }
                selected
            });
        if response.inner.unwrap_or(false) {
            ctx.request_repaint();
            app.brush_settings_dirty = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("Mode");
        let current_mode = app.stabilizer.mode;
        let mode_text = match current_mode {
            StabilizerMode::Ema => "EMA",
            StabilizerMode::SpringMassDamper => "Spring Physics",
        };
        let response = egui::ComboBox::from_id_source("sai_stabilizer_mode")
            .selected_text(mode_text)
            .width(100.0)
            .show_ui(ui, |ui| {
                let mut selected = false;
                if ui
                    .selectable_label(current_mode == StabilizerMode::Ema, "EMA")
                    .clicked()
                {
                    app.stabilizer.mode = StabilizerMode::Ema;
                    selected = true;
                }
                if ui
                    .selectable_label(
                        current_mode == StabilizerMode::SpringMassDamper,
                        "Spring Physics",
                    )
                    .clicked()
                {
                    app.stabilizer.mode = StabilizerMode::SpringMassDamper;
                    selected = true;
                }
                selected
            });
        if response.inner.unwrap_or(false) {
            ctx.request_repaint();
            app.brush_settings_dirty = true;
        }
    });
    ui.checkbox(&mut false, "Curve Interp.");
    let mut ps_hard_soft_size = 100.0;
    ui.horizontal(|ui| {
        ui.label("Prs. Hard:");
        ui.add(egui::Slider::new(&mut ps_hard_soft_size, 0.0..=100.0).show_value(false));
        ui.add(
            egui::DragValue::new(&mut ps_hard_soft_size)
                .speed(1.0)
                .suffix("%"),
        );
    });
    let mut ps_density = 100.0;
    ui.horizontal(|ui| {
        ui.label("Prs. Density:");
        ui.add(egui::Slider::new(&mut ps_density, 0.0..=100.0).show_value(false));
        ui.add(egui::DragValue::new(&mut ps_density).speed(1.0).suffix("%"));
    });
    let mut ps_blending = 100.0;
    ui.horizontal(|ui| {
        ui.label("Prs. Blending:");
        ui.add(egui::Slider::new(&mut ps_blending, 0.0..=100.0).show_value(false));
        ui.add(
            egui::DragValue::new(&mut ps_blending)
                .speed(1.0)
                .suffix("%"),
        );
    });
}

pub(crate) fn draw_symmetry_content(app: &mut PaintApp, ui: &mut egui::Ui, _ctx: &egui::Context) {
    if !matches!(app.active_tool(), ToolId::Brush | ToolId::Eraser) {
        ui.colored_label(
            egui::Color32::from_gray(120),
            "Active with Brush/Eraser only",
        );
        return;
    }
    ui.horizontal(|ui| {
        ui.label("Mode:");
        egui::ComboBox::from_id_source("symmetry_mode_sai")
            .selected_text(match app.symmetry_mode {
                crate::app::SymmetryMode::None => "Off",
                crate::app::SymmetryMode::Horizontal => "Horizontal",
                crate::app::SymmetryMode::Vertical => "Vertical",
                crate::app::SymmetryMode::Radial => "Radial",
            })
            .width(80.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(
                        matches!(app.symmetry_mode, crate::app::SymmetryMode::None),
                        "Off",
                    )
                    .clicked()
                {
                    app.symmetry_mode = crate::app::SymmetryMode::None;
                }
                if ui
                    .selectable_label(
                        matches!(app.symmetry_mode, crate::app::SymmetryMode::Horizontal),
                        "Horizontal",
                    )
                    .clicked()
                {
                    app.symmetry_mode = crate::app::SymmetryMode::Horizontal;
                }
                if ui
                    .selectable_label(
                        matches!(app.symmetry_mode, crate::app::SymmetryMode::Vertical),
                        "Vertical",
                    )
                    .clicked()
                {
                    app.symmetry_mode = crate::app::SymmetryMode::Vertical;
                }
                if ui
                    .selectable_label(
                        matches!(app.symmetry_mode, crate::app::SymmetryMode::Radial),
                        "Radial",
                    )
                    .clicked()
                {
                    app.symmetry_mode = crate::app::SymmetryMode::Radial;
                }
            });
    });
    if matches!(app.symmetry_mode, crate::app::SymmetryMode::Radial) {
        ui.horizontal(|ui| {
            ui.label("Segments:");
            ui.add(egui::DragValue::new(&mut app.symmetry_radial_count).clamp_range(2..=16));
        });
    }
    ui.horizontal(|ui| {
        ui.label("Center X:");
        ui.add(
            egui::DragValue::new(&mut app.symmetry_center.x)
                .clamp_range(0.0..=4096.0)
                .speed(1.0),
        );
    });
    ui.horizontal(|ui| {
        ui.label("Center Y:");
        ui.add(
            egui::DragValue::new(&mut app.symmetry_center.y)
                .clamp_range(0.0..=4096.0)
                .speed(1.0),
        );
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut app.shift_snap_enabled, "Shift-snap 15°");
    });
    if ui.button("Pressure Calibration...").clicked() {
        app.show_pressure_calibration = true;
    }
}

pub(crate) fn draw_advanced_debug_content(
    app: &mut PaintApp,
    ui: &mut egui::Ui,
    _ctx: &egui::Context,
) {
    if !matches!(app.active_tool(), ToolId::Brush | ToolId::Eraser) {
        ui.colored_label(
            egui::Color32::from_gray(120),
            "Active with Brush/Eraser only",
        );
        return;
    }
    ui.horizontal(|ui| {
        ui.label("Pressure");
        ui.add(egui::Slider::new(&mut app.pressure_curve, 0.25..=2.50).show_value(false));
        if ui
            .add(egui::DragValue::new(&mut app.pressure_curve).speed(0.05))
            .changed()
        {}
        ui.label("curve");
    });
    ui.horizontal(|ui| {
        ui.label("Min Pressure");
        ui.add(egui::Slider::new(&mut app.pressure_min, 0.00..=0.30).show_value(false));
        if ui
            .add(egui::DragValue::new(&mut app.pressure_min).speed(0.005))
            .changed()
        {
            app.pressure_min = app.pressure_min.clamp(0.0, 0.3);
        }
        ui.label("floor");
    });
    let raw_display = app
        .egui_touch_pressure
        .unwrap_or(app.tablet_axis.pressure)
        .clamp(0.0, 1.0);
    let raw_level = (raw_display * 8191.0).round() as u32;
    let smoothed_display = app
        .stabilizer
        .last_smoothed_pressure
        .unwrap_or(raw_display)
        .clamp(0.0, 1.0);
    let smoothed_level = (smoothed_display * 8191.0).round() as u32;
    let remapped_display = app.remap_pressure(smoothed_display);
    ui.label(format!(
        "Raw Pen:  {:.4} / 8192 ({})",
        raw_display, raw_level
    ));
    ui.label(format!(
        "Smoothed: {:.4} / 8192 ({})",
        smoothed_display, smoothed_level
    ));
    ui.label(format!("Remapped: {:.4}", remapped_display));
    let pressure_frac = remapped_display;
    let bar_width = ui.available_width().min(190.0);
    let (bar_response, bar_painter) =
        ui.allocate_painter(egui::vec2(bar_width, 8.0), egui::Sense::hover());
    let bar_rect = bar_response.rect;
    bar_painter.rect_filled(bar_rect, 0.0, egui::Color32::from_gray(60));
    let filled = egui::Rect::from_min_max(
        bar_rect.min,
        egui::pos2(
            bar_rect.min.x + bar_rect.width() * pressure_frac,
            bar_rect.max.y,
        ),
    );
    bar_painter.rect_filled(filled, 0.0, egui::Color32::from_rgb(100, 180, 255));
}

pub(crate) fn draw_tool_options_content(
    app: &mut PaintApp,
    ui: &mut egui::Ui,
    _ctx: &egui::Context,
) {
    match app.active_tool() {
        ToolId::Brush | ToolId::Eraser => {
            ui.colored_label(egui::Color32::from_gray(120), "Brush settings above");
        }
        ToolId::Fill => {
            ui.horizontal(|ui| {
                ui.label("Detection:");
                egui::ComboBox::from_id_source("fill_detection")
                    .selected_text(match app.fill_options.detection_mode {
                        crate::tools::fill::FillDetectionMode::TransparencyStrict => {
                            "Transp Strict"
                        }
                        crate::tools::fill::FillDetectionMode::TransparencyFuzzy => "Transp Fuzzy",
                        crate::tools::fill::FillDetectionMode::ColorDifference => "Color Diff",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.fill_options.detection_mode,
                            crate::tools::fill::FillDetectionMode::TransparencyStrict,
                            "Transp Strict",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.detection_mode,
                            crate::tools::fill::FillDetectionMode::TransparencyFuzzy,
                            "Transp Fuzzy",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.detection_mode,
                            crate::tools::fill::FillDetectionMode::ColorDifference,
                            "Color Diff",
                        );
                    });
            });
            match app.fill_options.detection_mode {
                crate::tools::fill::FillDetectionMode::ColorDifference => {
                    ui.horizontal(|ui| {
                        ui.label("Color Diff:");
                        ui.add(egui::Slider::new(&mut app.fill_options.tolerance, 0..=255));
                    });
                }
                crate::tools::fill::FillDetectionMode::TransparencyFuzzy => {
                    ui.horizontal(|ui| {
                        ui.label("Transp Diff:");
                        ui.add(egui::Slider::new(
                            &mut app.fill_options.transp_diff,
                            0..=255,
                        ));
                    });
                }
                crate::tools::fill::FillDetectionMode::TransparencyStrict => {}
            }
            ui.horizontal(|ui| {
                ui.label("Reference:");
                egui::ComboBox::from_id_source("fill_reference")
                    .selected_text(match app.fill_options.reference {
                        crate::tools::fill::FillReference::CurrentLayer => "Current Layer",
                        crate::tools::fill::FillReference::SelectionSourceLayers => {
                            "Reference Layers"
                        }
                        crate::tools::fill::FillReference::AllVisibleLayers => "All Visible",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.fill_options.reference,
                            crate::tools::fill::FillReference::CurrentLayer,
                            "Current Layer",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.reference,
                            crate::tools::fill::FillReference::SelectionSourceLayers,
                            "Reference Layers",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.reference,
                            crate::tools::fill::FillReference::AllVisibleLayers,
                            "All Visible",
                        );
                    });
            });
            let has_ref = app.layers.values().any(|l| l.selection_source);
            if app.fill_options.reference
                == crate::tools::fill::FillReference::SelectionSourceLayers
                && !has_ref
            {
                ui.colored_label(
                    egui::Color32::RED,
                    "⚠ No reference layer selected!\nEnable Ref (◎) on a lineart layer.",
                );
            }
            ui.horizontal(|ui| {
                ui.label("Expand:");
                ui.add(egui::Slider::new(&mut app.fill_options.expand_px, 0..=10));
            });
            ui.checkbox(&mut app.fill_options.contiguous, "Contiguous");
            ui.checkbox(&mut app.fill_options.antialias, "Anti-alias");
            ui.checkbox(&mut app.fill_options.respect_selection, "Respect selection");
            ui.checkbox(
                &mut app.fill_options.fill_transparent_only,
                "Fill transparent only",
            );
        }
        ToolId::RectSelect | ToolId::EllipseSelect | ToolId::Lasso | ToolId::PolygonLasso => {
            ui.horizontal(|ui| {
                ui.label("Mode:");
                egui::ComboBox::from_id_source("sel_mode")
                    .selected_text(match app.selection_mode {
                        crate::tools::selection::SelectionMode::Replace => "Replace",
                        crate::tools::selection::SelectionMode::Add => "Add",
                        crate::tools::selection::SelectionMode::Subtract => "Subtract",
                        crate::tools::selection::SelectionMode::Intersect => "Intersect",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Replace,
                            "Replace",
                        );
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Add,
                            "Add",
                        );
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Subtract,
                            "Subtract",
                        );
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Intersect,
                            "Intersect",
                        );
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Feather:");
                ui.add(egui::Slider::new(&mut app.selection_feather, 0.0..=100.0));
            });
        }
        ToolId::MagicWand => {
            ui.horizontal(|ui| {
                ui.label("Mode:");
                egui::ComboBox::from_id_source("wand_sel_mode")
                    .selected_text(match app.selection_mode {
                        crate::tools::selection::SelectionMode::Replace => "Replace",
                        crate::tools::selection::SelectionMode::Add => "Add",
                        crate::tools::selection::SelectionMode::Subtract => "Subtract",
                        crate::tools::selection::SelectionMode::Intersect => "Intersect",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Replace,
                            "Replace",
                        );
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Add,
                            "Add",
                        );
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Subtract,
                            "Subtract",
                        );
                        ui.selectable_value(
                            &mut app.selection_mode,
                            crate::tools::selection::SelectionMode::Intersect,
                            "Intersect",
                        );
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Detection:");
                egui::ComboBox::from_id_source("wand_detection")
                    .selected_text(match app.fill_options.detection_mode {
                        crate::tools::fill::FillDetectionMode::TransparencyStrict => {
                            "Transp Strict"
                        }
                        crate::tools::fill::FillDetectionMode::TransparencyFuzzy => "Transp Fuzzy",
                        crate::tools::fill::FillDetectionMode::ColorDifference => "Color Diff",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.fill_options.detection_mode,
                            crate::tools::fill::FillDetectionMode::TransparencyStrict,
                            "Transp Strict",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.detection_mode,
                            crate::tools::fill::FillDetectionMode::TransparencyFuzzy,
                            "Transp Fuzzy",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.detection_mode,
                            crate::tools::fill::FillDetectionMode::ColorDifference,
                            "Color Diff",
                        );
                    });
            });
            match app.fill_options.detection_mode {
                crate::tools::fill::FillDetectionMode::ColorDifference => {
                    ui.horizontal(|ui| {
                        ui.label("Color Diff:");
                        ui.add(egui::Slider::new(&mut app.fill_options.tolerance, 0..=255));
                    });
                }
                crate::tools::fill::FillDetectionMode::TransparencyFuzzy => {
                    ui.horizontal(|ui| {
                        ui.label("Transp Diff:");
                        ui.add(egui::Slider::new(
                            &mut app.fill_options.transp_diff,
                            0..=255,
                        ));
                    });
                }
                crate::tools::fill::FillDetectionMode::TransparencyStrict => {}
            }
            ui.horizontal(|ui| {
                ui.label("Reference:");
                egui::ComboBox::from_id_source("wand_reference")
                    .selected_text(match app.fill_options.reference {
                        crate::tools::fill::FillReference::CurrentLayer => "Current Layer",
                        crate::tools::fill::FillReference::SelectionSourceLayers => {
                            "Reference Layers"
                        }
                        crate::tools::fill::FillReference::AllVisibleLayers => "All Visible",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.fill_options.reference,
                            crate::tools::fill::FillReference::CurrentLayer,
                            "Current Layer",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.reference,
                            crate::tools::fill::FillReference::SelectionSourceLayers,
                            "Reference Layers",
                        );
                        ui.selectable_value(
                            &mut app.fill_options.reference,
                            crate::tools::fill::FillReference::AllVisibleLayers,
                            "All Visible",
                        );
                    });
            });
            let has_ref = app.layers.values().any(|l| l.selection_source);
            if app.fill_options.reference
                == crate::tools::fill::FillReference::SelectionSourceLayers
                && !has_ref
            {
                ui.colored_label(
                    egui::Color32::RED,
                    "⚠ No reference layer selected!\nEnable Ref (◎) on a lineart layer.",
                );
            }
            ui.horizontal(|ui| {
                ui.label("Expand:");
                ui.add(egui::Slider::new(&mut app.fill_options.expand_px, 0..=10));
            });
            ui.checkbox(&mut app.fill_options.contiguous, "Contiguous");
            ui.checkbox(&mut app.fill_options.antialias, "Anti-alias");
        }
        ToolId::Transform => {
            ui.horizontal(|ui| {
                ui.label("Interp:");
                egui::ComboBox::from_id_source("interp")
                    .selected_text(match app.transform_state.interpolation {
                        crate::tools::transform::InterpolationMode::Nearest => "Nearest",
                        crate::tools::transform::InterpolationMode::Bilinear => "Bilinear",
                        crate::tools::transform::InterpolationMode::Bicubic => "Bicubic",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.transform_state.interpolation,
                            crate::tools::transform::InterpolationMode::Nearest,
                            "Nearest",
                        );
                        ui.selectable_value(
                            &mut app.transform_state.interpolation,
                            crate::tools::transform::InterpolationMode::Bilinear,
                            "Bilinear",
                        );
                        ui.selectable_value(
                            &mut app.transform_state.interpolation,
                            crate::tools::transform::InterpolationMode::Bicubic,
                            "Bicubic",
                        );
                    });
            });
        }
        ToolId::Gradient => {
            ui.horizontal(|ui| {
                ui.label("Mode:");
                let mut gm = app.gradient_mode;
                egui::ComboBox::from_id_source("gradient_mode")
                    .selected_text(if gm == 0 { "Linear" } else { "Radial" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut gm, 0u32, "Linear");
                        ui.selectable_value(&mut gm, 1u32, "Radial");
                    });
                if gm != app.gradient_mode {
                    app.gradient_mode = gm;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Type:");
                let mut gt = app.gradient_type;
                egui::ComboBox::from_id_source("gradient_type")
                    .selected_text(match gt {
                        0 => "FG→BG",
                        1 => "FG→Transparent",
                        _ => "BG→Transparent",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut gt, 0u32, "FG→BG");
                        ui.selectable_value(&mut gt, 1u32, "FG→Transparent");
                        ui.selectable_value(&mut gt, 2u32, "BG→Transparent");
                    });
                if gt != app.gradient_type {
                    app.gradient_type = gt;
                }
            });
        }
        ToolId::ColorPicker => {
            ui.label("Picks color from canvas");
        }
        _ => {
            ui.label("No options for this tool");
        }
    }
}

pub fn render_floating_left_panels(app: &mut PaintApp, ctx: &egui::Context) {
    let panels = app.workspace_layout.panels.clone();
    for panel in &panels {
        if panel.location != PanelLocation::Floating || !panel.visible {
            continue;
        }
        let kind = panel.kind;
        let title = panel.title.clone();
        match kind {
            PanelKind::ToolsAndPresets
            | PanelKind::BrushSettings
            | PanelKind::ToolOptions
            | PanelKind::Stabilizer
            | PanelKind::Symmetry
            | PanelKind::AdvancedDebug => {}
            _ => continue,
        }
        let default_side = match kind {
            PanelKind::ToolsAndPresets
            | PanelKind::BrushSettings
            | PanelKind::ToolOptions
            | PanelKind::Stabilizer
            | PanelKind::Symmetry
            | PanelKind::AdvancedDebug => PanelLocation::Left,
            _ => PanelLocation::Right,
        };
        let window_resp = egui::Window::new(&title)
            .vscroll(true)
            .resizable(true)
            .min_size([200.0, 200.0])
            .default_size([300.0, 400.0])
            .id(egui::Id::new("floating_l").with(kind))
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
                    PanelKind::ToolsAndPresets => {
                        draw_tools_and_presets_content(app, ui, ctx);
                    }
                    PanelKind::BrushSettings => {
                        draw_brush_settings_content(app, ui, ctx);
                    }
                    PanelKind::ToolOptions => {
                        draw_tool_options_content(app, ui, ctx);
                    }
                    PanelKind::Stabilizer => {
                        draw_stabilizer_content(app, ui, ctx);
                    }
                    PanelKind::Symmetry => {
                        draw_symmetry_content(app, ui, ctx);
                    }
                    PanelKind::AdvancedDebug => {
                        draw_advanced_debug_content(app, ui, ctx);
                    }
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
