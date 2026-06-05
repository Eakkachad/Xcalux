use crate::app::{PaintApp, PresetIcon, ToolId};
use crate::input::{StabilizerLevel, StabilizerMode};
use hokusai::{Brush, BrushSetting, BrushState};

fn draw_dashed_line(painter: &egui::Painter, p1: egui::Pos2, p2: egui::Pos2, stroke: egui::Stroke) {
    let dist = p1.distance(p2);
    if dist < 0.1 { return; }
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

fn draw_dashed_circle(painter: &egui::Painter, center: egui::Pos2, radius: f32, stroke: egui::Stroke) {
    let circumference = 2.0 * std::f32::consts::PI * radius;
    if circumference < 0.1 { return; }
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
    if !app.show_minimal_ui {
        egui::SidePanel::left("left_sidebar")
            .resizable(false)
            .default_width(160.0)
            .min_width(120.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_source("left_sidebar_scroll")
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            // Combined TOOLS + BRUSH PRESETS (SAI-style)
                            ui.group(|ui| {
                                ui.label("TOOLS / BRUSH PRESETS");

                                 egui::Grid::new("tools_grid")
                                     .num_columns(5)
                                     .spacing([4.0, 4.0])
                                     .show(ui, |ui| {
                                          // ROW 1
                                          // 1. Rect Select (with right-click to Ellipse Select)
                                          let active_shape_tool = if app.active_tool == ToolId::EllipseSelect {
                                              ToolId::EllipseSelect
                                          } else {
                                              ToolId::RectSelect
                                          };
                                          let is_active = app.active_tool == ToolId::RectSelect || app.active_tool == ToolId::EllipseSelect;
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
                                              app.active_tool = active_shape_tool;
                                              ctx.request_repaint();
                                          }
                                          btn_resp.context_menu(|ui| {
                                              if ui.selectable_label(app.active_tool == ToolId::RectSelect, "Rectangle Selection").clicked() {
                                                  app.active_tool = ToolId::RectSelect;
                                                  ui.close_menu();
                                              }
                                              if ui.selectable_label(app.active_tool == ToolId::EllipseSelect, "Ellipse Selection").clicked() {
                                                  app.active_tool = ToolId::EllipseSelect;
                                                  ui.close_menu();
                                              }
                                          });
                                          btn_resp.on_hover_text("Selection Tool [Ctrl+A/D/I] (Right-click to change shape)");

                                          // 2. Lasso Select (with right-click to Polygon Lasso)
                                          let active_lasso_tool = if app.active_tool == ToolId::PolygonLasso {
                                              ToolId::PolygonLasso
                                          } else {
                                              ToolId::Lasso
                                          };
                                          let is_active = app.active_tool == ToolId::Lasso || app.active_tool == ToolId::PolygonLasso;
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
                                                  ui.painter().rect_filled(egui::Rect::from_center_size(p1, egui::vec2(2.0, 2.0)), 0.0, icon_color);
                                              }
                                          }
                                          if btn_resp.clicked() {
                                              app.active_tool = active_lasso_tool;
                                              ctx.request_repaint();
                                          }
                                          btn_resp.context_menu(|ui| {
                                              if ui.selectable_label(app.active_tool == ToolId::Lasso, "Free Lasso Selection").clicked() {
                                                  app.active_tool = ToolId::Lasso;
                                                  ui.close_menu();
                                              }
                                              if ui.selectable_label(app.active_tool == ToolId::PolygonLasso, "Polygon Lasso Selection [Shift+L]").clicked() {
                                                  app.active_tool = ToolId::PolygonLasso;
                                                  ui.close_menu();
                                              }
                                          });
                                          btn_resp.on_hover_text("Lasso Tool (Right-click to switch to Polygon)");

                                          // 3. Magic Wand
                                          let is_active = app.active_tool == ToolId::MagicWand;
                                          let btn = egui::Button::new("").selected(is_active);
                                          let btn_resp = ui.add_sized([26.0, 26.0], btn);
                                          let icon_color = if is_active {
                                              egui::Color32::from_rgb(0, 120, 215)
                                          } else {
                                              ui.style().visuals.widgets.inactive.text_color()
                                          };
                                          let center = btn_resp.rect.center();
                                          let stroke_wand = egui::Stroke::new(1.8, icon_color);
                                          ui.painter().line_segment([center + egui::vec2(-6.0, 6.0), center + egui::vec2(1.0, -1.0)], stroke_wand);
                                          ui.painter().circle_filled(center + egui::vec2(1.0, -1.0), 1.5, icon_color);
                                          let tip = center + egui::vec2(1.0, -1.0);
                                          let sparkle_stroke = egui::Stroke::new(1.0, icon_color);
                                          ui.painter().line_segment([tip + egui::vec2(4.0, -4.0), tip + egui::vec2(6.0, -6.0)], sparkle_stroke);
                                          ui.painter().line_segment([tip + egui::vec2(0.0, -5.0), tip + egui::vec2(0.0, -7.0)], sparkle_stroke);
                                          ui.painter().line_segment([tip + egui::vec2(5.0, 0.0), tip + egui::vec2(7.0, 0.0)], sparkle_stroke);
                                          if btn_resp.clicked() {
                                              app.active_tool = ToolId::MagicWand;
                                              ctx.request_repaint();
                                          }
                                          btn_resp.on_hover_text("Magic Wand Selection");

                                          // 4. Transform (Selection Move/Transform)
                                          let is_active = app.active_tool == ToolId::Transform;
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
                                              ui.painter().rect_filled(egui::Rect::from_center_size(c, h_size), 0.0, icon_color);
                                          }
                                          if btn_resp.clicked() {
                                              app.active_tool = ToolId::Transform;
                                              ctx.request_repaint();
                                          }
                                          btn_resp.on_hover_text("Transform Tool [Ctrl+T]");

                                          // 5. Text (Dummy)
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
                                          // 6. Move Layer
                                          let is_active = app.active_tool == ToolId::Move;
                                          let btn = egui::Button::new("").selected(is_active);
                                          let btn_resp = ui.add_sized([26.0, 26.0], btn);
                                          let icon_color = if is_active {
                                              egui::Color32::from_rgb(0, 120, 215)
                                          } else {
                                              ui.style().visuals.widgets.inactive.text_color()
                                          };
                                          let center = btn_resp.rect.center();
                                          let stroke = egui::Stroke::new(1.5, icon_color);
                                          ui.painter().line_segment([center - egui::vec2(7.0, 0.0), center + egui::vec2(7.0, 0.0)], stroke);
                                          ui.painter().line_segment([center - egui::vec2(0.0, 7.0), center + egui::vec2(0.0, 7.0)], stroke);
                                          // Arrowheads
                                          ui.painter().line_segment([center - egui::vec2(7.0, 0.0), center - egui::vec2(4.0, -3.0)], stroke);
                                          ui.painter().line_segment([center - egui::vec2(7.0, 0.0), center - egui::vec2(4.0, 3.0)], stroke);
                                          ui.painter().line_segment([center + egui::vec2(7.0, 0.0), center + egui::vec2(4.0, -3.0)], stroke);
                                          ui.painter().line_segment([center + egui::vec2(7.0, 0.0), center + egui::vec2(4.0, 3.0)], stroke);
                                          ui.painter().line_segment([center - egui::vec2(0.0, 7.0), center - egui::vec2(-3.0, 4.0)], stroke);
                                          ui.painter().line_segment([center - egui::vec2(0.0, 7.0), center - egui::vec2(3.0, 4.0)], stroke);
                                          ui.painter().line_segment([center + egui::vec2(0.0, 7.0), center + egui::vec2(-3.0, 4.0)], stroke);
                                          ui.painter().line_segment([center + egui::vec2(0.0, 7.0), center + egui::vec2(3.0, 4.0)], stroke);
                                          if btn_resp.clicked() {
                                              app.active_tool = ToolId::Move;
                                              ctx.request_repaint();
                                          }
                                          btn_resp.on_hover_text("Move Layer Tool");

                                          // 7. Zoom View
                                          let is_active = app.active_tool == ToolId::Zoom;
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
                                          ui.painter().line_segment([lens_center + egui::vec2(2.8, 2.8), center + egui::vec2(6.0, 6.0)], egui::Stroke::new(2.5, icon_color));
                                          if btn_resp.clicked() {
                                              app.active_tool = ToolId::Zoom;
                                              ctx.request_repaint();
                                          }
                                          btn_resp.on_hover_text("Zoom Canvas");

                                          // 8. Rotate View
                                          let is_active = app.active_tool == ToolId::RotateView;
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
                                              let a1 = -std::f32::consts::FRAC_PI_2 + (i as f32 / steps as f32) * (1.5 * std::f32::consts::PI);
                                              let a2 = -std::f32::consts::FRAC_PI_2 + ((i + 1) as f32 / steps as f32) * (1.5 * std::f32::consts::PI);
                                              let p1 = center + egui::vec2(a1.cos(), a1.sin()) * radius;
                                              let p2 = center + egui::vec2(a2.cos(), a2.sin()) * radius;
                                              ui.painter().line_segment([p1, p2], stroke);
                                          }
                                          ui.painter().line_segment([tip, tip + egui::vec2(-3.0, -3.0)], stroke);
                                          ui.painter().line_segment([tip, tip + egui::vec2(-3.0, 3.0)], stroke);
                                          if btn_resp.clicked() {
                                              app.active_tool = ToolId::RotateView;
                                              ctx.request_repaint();
                                          }
                                          btn_resp.on_hover_text("Rotate View [R]");

                                          // 9. Hand Panning
                                          let is_active = app.active_tool == ToolId::Hand;
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
                                          ui.painter().line_segment([p_base - egui::vec2(4.0, 0.0), p_base + egui::vec2(4.0, 0.0)], stroke);
                                          ui.painter().line_segment([center + egui::vec2(-3.0, 3.0), center + egui::vec2(-3.0, -4.0)], stroke);
                                          ui.painter().line_segment([center + egui::vec2(-1.0, 3.0), center + egui::vec2(-1.0, -6.0)], stroke);
                                          ui.painter().line_segment([center + egui::vec2(1.0, 3.0), center + egui::vec2(1.0, -5.0)], stroke);
                                          ui.painter().line_segment([center + egui::vec2(3.0, 3.0), center + egui::vec2(3.0, -3.0)], stroke);
                                          ui.painter().line_segment([center + egui::vec2(-3.0, 1.0), center + egui::vec2(-5.0, -1.0)], stroke);
                                          if btn_resp.clicked() {
                                              app.active_tool = ToolId::Hand;
                                              ctx.request_repaint();
                                          }
                                          btn_resp.on_hover_text("Hand Panning Tool [Space]");

                                          // 10. Color Picker (Eyedropper)
                                          let is_active = app.active_tool == ToolId::ColorPicker;
                                          let btn = egui::Button::new("").selected(is_active);
                                          let btn_resp = ui.add_sized([26.0, 26.0], btn);
                                          let icon_color = if is_active {
                                              egui::Color32::from_rgb(0, 120, 215)
                                          } else {
                                              ui.style().visuals.widgets.inactive.text_color()
                                          };
                                          let center = btn_resp.rect.center();
                                          let stroke = egui::Stroke::new(1.5, icon_color);
                                          ui.painter().line_segment([center + egui::vec2(5.0, -5.0), center + egui::vec2(-1.0, 1.0)], stroke);
                                          ui.painter().circle_filled(center + egui::vec2(5.0, -5.0), 2.5, icon_color);
                                          ui.painter().line_segment([center + egui::vec2(-1.0, 1.0), center + egui::vec2(-4.0, 4.0)], stroke);
                                          if btn_resp.clicked() {
                                              app.active_tool = ToolId::ColorPicker;
                                              ctx.request_repaint();
                                          }
                                          btn_resp.on_hover_text("Color Picker (Eyedropper) [Alt/I]");
                                          ui.end_row();
                                     });
                                 ui.separator();
                                 ui.label("VECTOR TOOLS");
                                 egui::Grid::new("vector_tools_grid")
                                     .num_columns(3)
                                     .spacing([4.0, 4.0])
                                     .show(ui, |ui| {
                                         let vec_tools: [(ToolId, &str, &str); 3] = [
                                             (ToolId::VectorPen, "✎", "Vector Pen — draw smooth vector strokes"),
                                             (ToolId::Curve, "〰", "Curve — place 4 control points for a bezier curve"),
                                             (ToolId::EditCP, "⬩", "Edit CP — select and drag control points"),
                                         ];
                                         for &(tool_id, label, tooltip) in &vec_tools {
                                             let is_active = app.active_tool == tool_id;
                                             let btn = egui::Button::new(
                                                 egui::RichText::new(label).size(12.0)
                                             )
                                             .selected(is_active);
                                             let r = ui.add_sized([26.0, 26.0], btn).on_hover_text(tooltip);
                                             if r.clicked() {
                                                 app.active_tool = tool_id;
                                                 if tool_id == ToolId::VectorPen {
                                                     let is_vector = app.layers.get(&app.active_layer_id)
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
                                                     let is_selected = app.active_preset_index == i && matches!(app.active_tool, ToolId::Brush | ToolId::Eraser);
                                                     
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
                                                         let (rect, btn_response) = ui.allocate_exact_size(egui::vec2(34.0, 30.0), egui::Sense::click());
                                                         
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
                                                         ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0, stroke_color));

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
                                                         if ui.add_enabled(can_delete, egui::Button::new("Delete")).clicked() {
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
                                                                 let line_segment_x = if is_left { rect.left() } else { rect.right() };
                                                                 ui.painter().line_segment(
                                                                     [egui::pos2(line_segment_x, rect.top()), egui::pos2(line_segment_x, rect.bottom())],
                                                                     egui::Stroke::new(2.5, egui::Color32::from_rgb(0, 120, 215))
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
                                                     let (rect, btn_response) = ui.allocate_exact_size(egui::vec2(34.0, 30.0), egui::Sense::click());
                                                     let bg_color = if btn_response.hovered() {
                                                         egui::Color32::from_gray(245)
                                                     } else {
                                                         egui::Color32::WHITE
                                                     };
                                                     ui.painter().rect_filled(rect, 2.0, bg_color);
                                                     ui.painter().rect_stroke(rect, 1.0, egui::Stroke::new(1.0, egui::Color32::from_gray(225)));

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
                                                             match crate::brush_io::load_artybrush(path, &mut app.brush_textures) {
                                                                 Ok(mut new_preset) => {
                                                                     app.preset_id_counter += 1;
                                                                     new_preset.id = app.preset_id_counter;
 
                                                                     let mut brush = Brush::new();
                                                                     PaintApp::set_constant(&mut brush, BrushSetting::Radius, new_preset.radius_log);
                                                                     PaintApp::set_constant(&mut brush, BrushSetting::Opaque, new_preset.opacity);
                                                                     PaintApp::set_constant(&mut brush, BrushSetting::Hardness, new_preset.hardness);
                                                                     PaintApp::set_constant(&mut brush, BrushSetting::Smudge, new_preset.color_blending);
                                                                     PaintApp::set_constant(&mut brush, BrushSetting::SmudgeLength, new_preset.dilution);
                                                                     if new_preset.is_eraser {
                                                                         PaintApp::set_constant(&mut brush, BrushSetting::Eraser, 1.0);
                                                                     }
 
                                                                     app.presets.push(new_preset);
                                                                     app.brushes.push(brush);
                                                                     app.brush_states.push(BrushState::default());
 
                                                                     let new_idx = app.presets.len() - 1;
                                                                     app.select_preset(new_idx);
                                                                     log::info!("Imported .artybrush successfully!");
                                                                 }
                                                                 Err(e) => {
                                                                     log::error!("Failed to import .artybrush: {:?}", e);
                                                                 }
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
                                                                             final_bytes[(y * 256 + x) as usize] = gray_bytes[(y * w + x) as usize];
                                                                         }
                                                                     }
                                                                     let name = path.file_stem()
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
                                                                         name: path.file_stem().and_then(|s| s.to_str()).unwrap_or("SUT Brush").to_string(),
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
                                                                     PaintApp::set_constant(&mut brush, BrushSetting::Radius, new_preset.radius_log);
                                                                     PaintApp::set_constant(&mut brush, BrushSetting::Opaque, new_preset.opacity);
                                                                     PaintApp::set_constant(&mut brush, BrushSetting::Hardness, new_preset.hardness);
 
                                                                     app.presets.push(new_preset);
                                                                     app.brushes.push(brush);
                                                                     app.brush_states.push(BrushState::default());
 
                                                                     let new_idx = app.presets.len() - 1;
                                                                     app.select_preset(new_idx);
                                                                     log::info!("Extracted and imported SUT brush successfully!");
                                                                 }
                                                                 Err(e) => {
                                                                     log::error!("Failed to extract SUT: {:?}", e);
                                                                 }
                                                             }
                                                             ui.close_menu();
                                                         }
                                                     });
                                                     
                                                     if show_creation_menu {
                                                         ui.ctx().memory_mut(|mem| mem.open_popup(btn_response.id.with("context_menu")));
                                                     }
                                                 }
                                                 
                                                 if i % 4 == 3 {
                                                     ui.end_row();
                                                 }
                                             }
                                         });
                                     });

                                // Inline renaming text box
                                if let Some(idx) = app.renaming_preset_index {
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        ui.label("Rename:");
                                        let res = ui.add(egui::TextEdit::singleline(&mut app.rename_input).desired_width(100.0));
                                        if res.lost_focus() || ui.button("OK").clicked() {
                                            if !app.rename_input.trim().is_empty() {
                                                app.presets[idx].name = app.rename_input.trim().to_string();
                                            }
                                            app.renaming_preset_index = None;
                                        }
                                        if ui.button("✕").clicked() {
                                            app.renaming_preset_index = None;
                                        }
                                    });
                                }

                                ui.add_space(6.0);

                                // Stabilizer configuration UI
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label("Stabilizer:");
                                        let current_level = app.stabilizer.level;
                                        let text = match current_level {
                                            StabilizerLevel::Off => "Off".to_string(),
                                            StabilizerLevel::Level(val) => format!("Level {}", val),
                                            StabilizerLevel::SLevel(val) => format!("S-{}", val),
                                        };
                                        let response = egui::ComboBox::from_id_source("side_stabilizer_level")
                                            .selected_text(text)
                                            .width(90.0)
                                            .show_ui(ui, |ui| {
                                                let mut selected = false;
                                                if ui.selectable_label(matches!(current_level, StabilizerLevel::Off), "Off").clicked() {
                                                    app.stabilizer.set_level(StabilizerLevel::Off);
                                                    selected = true;
                                                }
                                                for val in 1..=15 {
                                                    let is_sel = match current_level {
                                                        StabilizerLevel::Level(v) => v == val,
                                                        _ => false,
                                                    };
                                                    if ui.selectable_label(is_sel, format!("Level {}", val)).clicked() {
                                                        app.stabilizer.set_level(StabilizerLevel::Level(val));
                                                        selected = true;
                                                    }
                                                }
                                                for val in 1..=5 {
                                                    let is_sel = match current_level {
                                                        StabilizerLevel::SLevel(v) => v == val,
                                                        _ => false,
                                                    };
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
                                        ui.label("Mode:");
                                        let current_mode = app.stabilizer.mode;
                                        let mode_text = match current_mode {
                                            StabilizerMode::Ema => "EMA",
                                            StabilizerMode::SpringMassDamper => "Spring Physics",
                                        };
                                        let response = egui::ComboBox::from_id_source("side_stabilizer_mode")
                                            .selected_text(mode_text)
                                            .width(120.0)
                                            .show_ui(ui, |ui| {
                                                let mut selected = false;
                                                if ui.selectable_label(current_mode == StabilizerMode::Ema, "EMA").clicked() {
                                                    app.stabilizer.mode = StabilizerMode::Ema;
                                                    selected = true;
                                                }
                                                if ui.selectable_label(current_mode == StabilizerMode::SpringMassDamper, "Spring Physics").clicked() {
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
                                });
                            });

                            ui.add_space(5.0);

                            // Symmetry configuration UI
                            ui.group(|ui| {
                                ui.label("SYMMETRY");
                                ui.horizontal(|ui| {
                                    ui.label("Mode:");
                                    egui::ComboBox::from_id_source("symmetry_mode")
                                        .selected_text(match app.symmetry_mode {
                                            crate::app::SymmetryMode::None => "Off",
                                            crate::app::SymmetryMode::Horizontal => "Horizontal",
                                            crate::app::SymmetryMode::Vertical => "Vertical",
                                            crate::app::SymmetryMode::Radial => "Radial",
                                        })
                                        .show_ui(ui, |ui| {
                                            if ui.selectable_label(matches!(app.symmetry_mode, crate::app::SymmetryMode::None), "Off").clicked() {
                                                app.symmetry_mode = crate::app::SymmetryMode::None;
                                            }
                                            if ui.selectable_label(matches!(app.symmetry_mode, crate::app::SymmetryMode::Horizontal), "Horizontal").clicked() {
                                                app.symmetry_mode = crate::app::SymmetryMode::Horizontal;
                                            }
                                            if ui.selectable_label(matches!(app.symmetry_mode, crate::app::SymmetryMode::Vertical), "Vertical").clicked() {
                                                app.symmetry_mode = crate::app::SymmetryMode::Vertical;
                                            }
                                            if ui.selectable_label(matches!(app.symmetry_mode, crate::app::SymmetryMode::Radial), "Radial").clicked() {
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
                                    ui.add(egui::DragValue::new(&mut app.symmetry_center.x).clamp_range(0.0..=4096.0));
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Center Y:");
                                    ui.add(egui::DragValue::new(&mut app.symmetry_center.y).clamp_range(0.0..=4096.0));
                                });
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut app.shift_snap_enabled, "Shift-snap (15°)");
                                });
                                if ui.button("Pressure Calibration...").clicked() {
                                    app.show_pressure_calibration = true;
                                }
                            });

                            ui.add_space(5.0);

                            // Dynamic Tool Options - changes based on active tool
                            ui.group(|ui| {
                                ui.label("TOOL OPTIONS");
                                match app.active_tool {
                                    ToolId::Fill => {
                                        ui.horizontal(|ui| {
                                            ui.label("Detection:");
                                            egui::ComboBox::from_id_source("fill_detection")
                                                .selected_text(match app.fill_options.detection_mode {
                                                    crate::tools::fill::FillDetectionMode::TransparencyStrict => "Transp Strict",
                                                    crate::tools::fill::FillDetectionMode::TransparencyFuzzy => "Transp Fuzzy",
                                                    crate::tools::fill::FillDetectionMode::ColorDifference => "Color Diff",
                                                })
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(&mut app.fill_options.detection_mode, crate::tools::fill::FillDetectionMode::TransparencyStrict, "Transp Strict");
                                                    ui.selectable_value(&mut app.fill_options.detection_mode, crate::tools::fill::FillDetectionMode::TransparencyFuzzy, "Transp Fuzzy");
                                                    ui.selectable_value(&mut app.fill_options.detection_mode, crate::tools::fill::FillDetectionMode::ColorDifference, "Color Diff");
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
                                                    ui.add(egui::Slider::new(&mut app.fill_options.transp_diff, 0..=255));
                                                });
                                            }
                                            crate::tools::fill::FillDetectionMode::TransparencyStrict => {}
                                        }
                                        ui.horizontal(|ui| {
                                            ui.label("Reference:");
                                            egui::ComboBox::from_id_source("fill_reference")
                                                .selected_text(match app.fill_options.reference {
                                                    crate::tools::fill::FillReference::CurrentLayer => "Current Layer",
                                                    crate::tools::fill::FillReference::SelectionSourceLayers => "Reference Layers",
                                                    crate::tools::fill::FillReference::AllVisibleLayers => "All Visible",
                                                })
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(&mut app.fill_options.reference, crate::tools::fill::FillReference::CurrentLayer, "Current Layer");
                                                    ui.selectable_value(&mut app.fill_options.reference, crate::tools::fill::FillReference::SelectionSourceLayers, "Reference Layers");
                                                    ui.selectable_value(&mut app.fill_options.reference, crate::tools::fill::FillReference::AllVisibleLayers, "All Visible");
                                                });
                                        });

                                        let has_ref = app.layers.values().any(|l| l.selection_source);
                                        if app.fill_options.reference == crate::tools::fill::FillReference::SelectionSourceLayers && !has_ref {
                                            ui.colored_label(egui::Color32::RED, "⚠ No reference layer selected!\nEnable Ref (◎) on a lineart layer.");
                                        }

                                        ui.horizontal(|ui| {
                                            ui.label("Expand:");
                                            ui.add(egui::Slider::new(&mut app.fill_options.expand_px, 0..=10));
                                        });
                                        ui.checkbox(&mut app.fill_options.contiguous, "Contiguous");
                                        ui.checkbox(&mut app.fill_options.antialias, "Anti-alias");
                                        ui.checkbox(&mut app.fill_options.respect_selection, "Respect selection");
                                        ui.checkbox(&mut app.fill_options.fill_transparent_only, "Fill transparent only");
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
                                                    ui.selectable_value(&mut app.selection_mode, crate::tools::selection::SelectionMode::Replace, "Replace");
                                                    ui.selectable_value(&mut app.selection_mode, crate::tools::selection::SelectionMode::Add, "Add");
                                                    ui.selectable_value(&mut app.selection_mode, crate::tools::selection::SelectionMode::Subtract, "Subtract");
                                                    ui.selectable_value(&mut app.selection_mode, crate::tools::selection::SelectionMode::Intersect, "Intersect");
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
                                                    ui.selectable_value(&mut app.selection_mode, crate::tools::selection::SelectionMode::Replace, "Replace");
                                                    ui.selectable_value(&mut app.selection_mode, crate::tools::selection::SelectionMode::Add, "Add");
                                                    ui.selectable_value(&mut app.selection_mode, crate::tools::selection::SelectionMode::Subtract, "Subtract");
                                                    ui.selectable_value(&mut app.selection_mode, crate::tools::selection::SelectionMode::Intersect, "Intersect");
                                                });
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label("Detection:");
                                            egui::ComboBox::from_id_source("wand_detection")
                                                .selected_text(match app.fill_options.detection_mode {
                                                    crate::tools::fill::FillDetectionMode::TransparencyStrict => "Transp Strict",
                                                    crate::tools::fill::FillDetectionMode::TransparencyFuzzy => "Transp Fuzzy",
                                                    crate::tools::fill::FillDetectionMode::ColorDifference => "Color Diff",
                                                })
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(&mut app.fill_options.detection_mode, crate::tools::fill::FillDetectionMode::TransparencyStrict, "Transp Strict");
                                                    ui.selectable_value(&mut app.fill_options.detection_mode, crate::tools::fill::FillDetectionMode::TransparencyFuzzy, "Transp Fuzzy");
                                                    ui.selectable_value(&mut app.fill_options.detection_mode, crate::tools::fill::FillDetectionMode::ColorDifference, "Color Diff");
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
                                                    ui.add(egui::Slider::new(&mut app.fill_options.transp_diff, 0..=255));
                                                });
                                            }
                                            crate::tools::fill::FillDetectionMode::TransparencyStrict => {}
                                        }
                                        ui.horizontal(|ui| {
                                            ui.label("Reference:");
                                            egui::ComboBox::from_id_source("wand_reference")
                                                .selected_text(match app.fill_options.reference {
                                                    crate::tools::fill::FillReference::CurrentLayer => "Current Layer",
                                                    crate::tools::fill::FillReference::SelectionSourceLayers => "Reference Layers",
                                                    crate::tools::fill::FillReference::AllVisibleLayers => "All Visible",
                                                })
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(&mut app.fill_options.reference, crate::tools::fill::FillReference::CurrentLayer, "Current Layer");
                                                    ui.selectable_value(&mut app.fill_options.reference, crate::tools::fill::FillReference::SelectionSourceLayers, "Reference Layers");
                                                    ui.selectable_value(&mut app.fill_options.reference, crate::tools::fill::FillReference::AllVisibleLayers, "All Visible");
                                                });
                                        });

                                        let has_ref = app.layers.values().any(|l| l.selection_source);
                                        if app.fill_options.reference == crate::tools::fill::FillReference::SelectionSourceLayers && !has_ref {
                                            ui.colored_label(egui::Color32::RED, "⚠ No reference layer selected!\nEnable Ref (◎) on a lineart layer.");
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
                                                    ui.selectable_value(&mut app.transform_state.interpolation, crate::tools::transform::InterpolationMode::Nearest, "Nearest");
                                                    ui.selectable_value(&mut app.transform_state.interpolation, crate::tools::transform::InterpolationMode::Bilinear, "Bilinear");
                                                    ui.selectable_value(&mut app.transform_state.interpolation, crate::tools::transform::InterpolationMode::Bicubic, "Bicubic");
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
                                                .selected_text(match gt { 0 => "FG→BG", 1 => "FG→Transparent", _ => "BG→Transparent" })
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
                                        if matches!(app.active_tool, ToolId::Brush | ToolId::Eraser) {
                                            // Brush preview box + size slider
                                            let pixel_radius = app.brush_radius_log.exp();
                                            ui.horizontal(|ui| {
                                                let (resp, painter) = ui.allocate_painter(egui::Vec2::splat(56.0), egui::Sense::hover());
                                                let r = resp.rect;
                                                // Draw base background
                                                painter.rect_filled(r, 2.0, egui::Color32::WHITE);
                                                // Draw checkerboard
                                                let cell_size = 7.0;
                                                for yi in 0..8 {
                                                    for xi in 0..8 {
                                                        if (xi + yi) % 2 == 1 {
                                                            let cell_rect = egui::Rect::from_min_size(
                                                                egui::Pos2::new(r.min.x + xi as f32 * cell_size, r.min.y + yi as f32 * cell_size),
                                                                egui::Vec2::splat(cell_size),
                                                            );
                                                            painter.rect_filled(cell_rect, 0.0, egui::Color32::from_gray(220));
                                                        }
                                                    }
                                                }
                                                painter.rect_stroke(r, 2.0, egui::Stroke::new(1.0, egui::Color32::from_gray(180)));

                                                // Draw custom brush preview with hardness falloff
                                                let center = r.center();
                                                let h = app.brush_hardness;
                                                let o = app.brush_opacity;
                                                let num_steps = 15;
                                                for i in 0..=num_steps {
                                                    let t = i as f32 / num_steps as f32; // t goes from 0.0 to 1.0
                                                    let r_i = 22.0 * (1.0 - t * (1.0 - h)); // radius from 22.0 down to 22.0 * h
                                                    let alpha_i = o * t; // alpha from 0.0 to o
                                                    let col = egui::Color32::from_rgba_unmultiplied(
                                                        (app.foreground_color[0] * 255.0) as u8,
                                                        (app.foreground_color[1] * 255.0) as u8,
                                                        (app.foreground_color[2] * 255.0) as u8,
                                                        (alpha_i * 255.0) as u8,
                                                    );
                                                    painter.circle_filled(center, r_i, col);
                                                }

                                                ui.vertical(|ui| {
                                                    ui.label(format!("Size: {:.1} px", pixel_radius));
                                                    if ui.add(
                                                        egui::Slider::new(&mut app.brush_radius_log, -1.0..=5.0)
                                                            .show_value(false),
                                                    ).changed() {
                                                        app.brush_settings_dirty = true;
                                                    }
                                                });
                                            });

                                            // Opacity
                                            ui.horizontal(|ui| {
                                                ui.label("Opacity:");
                                                if ui.add(egui::Slider::new(&mut app.brush_opacity, 0.0..=1.0)).changed() {
                                                    app.brush_settings_dirty = true;
                                                }
                                            });

                                            // Hardness
                                            ui.horizontal(|ui| {
                                                ui.label("Hardness:");
                                                if ui.add(egui::Slider::new(&mut app.brush_hardness, 0.0..=1.0)).changed() {
                                                    app.brush_settings_dirty = true;
                                                }
                                            });

                                            // Min Size %
                                            ui.horizontal(|ui| {
                                                ui.label("Min Size %:");
                                                if ui.add(egui::Slider::new(&mut app.brush_min_size_fraction, 0.0..=1.0)).changed() {
                                                    app.brush_settings_dirty = true;
                                                }
                                            });

                                            // Blending
                                            ui.horizontal(|ui| {
                                                ui.label("Blending:");
                                                if ui.add(egui::Slider::new(&mut app.brush_color_blending, 0.0..=1.0)).changed() {
                                                    app.brush_settings_dirty = true;
                                                }
                                            });

                                            // Dilution
                                            ui.horizontal(|ui| {
                                                ui.label("Dilution:");
                                                if ui.add(egui::Slider::new(&mut app.brush_dilution, 0.0..=1.0)).changed() {
                                                    app.brush_settings_dirty = true;
                                                }
                                            });

                                            // Spacing
                                            ui.horizontal(|ui| {
                                                ui.label("Spacing:");
                                                if ui.add(egui::Slider::new(&mut app.brush_spacing, 0.5..=10.0)).changed() {
                                                    app.brush_settings_dirty = true;
                                                }
                                            });

                                            // Density
                                            ui.horizontal(|ui| {
                                                ui.label("Density:");
                                                if ui.add(egui::Slider::new(&mut app.brush_density, 0.0..=1.0)).changed() {
                                                    app.brush_settings_dirty = true;
                                                }
                                            });

                                            // Eraser Checkbox
                                            if !app.presets.is_empty() {
                                                let is_eraser = &mut app.presets[app.active_preset_index].is_eraser;
                                                if ui.checkbox(is_eraser, "Eraser Mode [E]").changed() {
                                                    app.brush_settings_dirty = true;
                                                }
                                            }

                                            // Texture Dropdown
                                            ui.horizontal(|ui| {
                                                ui.label("Texture:");
                                                let mut selected_tex = app.brush_texture_id;
                                                let current_name = app.brush_textures.get(selected_tex as usize)
                                                    .map(|t| t.name.as_str())
                                                    .unwrap_or("None");
                                                let res = egui::ComboBox::from_id_source("brush_texture_combo")
                                                    .selected_text(current_name)
                                                    .show_ui(ui, |ui| {
                                                        let mut changed = false;
                                                        for (idx, tex) in app.brush_textures.iter().enumerate() {
                                                            if ui.selectable_value(&mut selected_tex, idx as u32, &tex.name).clicked() {
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

                                            // Texture Scale Slider
                                            if app.brush_texture_id > 0 {
                                                ui.horizontal(|ui| {
                                                    ui.label("Tex Scale:");
                                                    if ui.add(egui::Slider::new(&mut app.brush_texture_scale, 0.1..=10.0)).changed() {
                                                        app.brush_settings_dirty = true;
                                                    }
                                                });
                                            }

                                            // Bristle ID Slider
                                            ui.horizontal(|ui| {
                                                ui.label("Bristle ID:");
                                                if ui.add(egui::Slider::new(&mut app.brush_bristle_id, 0..=5)).changed() {
                                                    app.brush_settings_dirty = true;
                                                }
                                            });

                                            // Lock Canvas Bounds
                                            ui.checkbox(&mut app.lock_canvas_bounds, "Lock Canvas Bounds");

                                            ui.add_space(5.0);

                                            // Advanced / debug Info
                                            ui.collapsing("Debug / Advanced Info", |ui| {
                                                ui.horizontal(|ui| {
                                                    ui.label("Pressure response:");
                                                    ui.add(
                                                        egui::Slider::new(&mut app.pressure_curve, 0.25..=2.50)
                                                            .text("curve"),
                                                    );
                                                });
                                                ui.horizontal(|ui| {
                                                    ui.label("Min pressure:");
                                                    ui.add(
                                                        egui::Slider::new(&mut app.pressure_min, 0.00..=0.30)
                                                            .text("floor"),
                                                    );
                                                });

                                                let raw_display = app.egui_touch_pressure.unwrap_or(app.tablet_axis.pressure).clamp(0.0, 1.0);
                                                let raw_level = (raw_display * 8191.0).round() as u32;

                                                let smoothed_display = app.stabilizer.last_smoothed_pressure.unwrap_or(raw_display).clamp(0.0, 1.0);
                                                let smoothed_level = (smoothed_display * 8191.0).round() as u32;

                                                let remapped_display = app.remap_pressure(smoothed_display);

                                                ui.label(format!("Raw Pen:  {:.4} / 8192 ({})", raw_display, raw_level));
                                                ui.label(format!("Smoothed: {:.4} / 8192 ({})", smoothed_display, smoothed_level));
                                                ui.label(format!("Remapped: {:.4}", remapped_display));

                                                // Visual pressure bar
                                                let pressure_frac = remapped_display;
                                                let bar_rect = ui.available_rect_before_wrap();
                                                let bar_width = bar_rect.width().min(190.0);
                                                let bar_height = 10.0;
                                                let (bar_response, painter) = ui.allocate_painter(
                                                    egui::Vec2::new(bar_width, bar_height), egui::Sense::hover()
                                                );
                                                let r = bar_response.rect;
                                                painter.rect_filled(r, 2.0, egui::Color32::from_gray(60));
                                                let filled = egui::Rect::from_min_max(
                                                    r.min,
                                                    egui::Pos2::new(r.min.x + r.width() * pressure_frac, r.max.y),
                                                );
                                                painter.rect_filled(filled, 2.0, egui::Color32::from_rgb(100, 180, 255));
                                            });
                                        } else {
                                            ui.label("No options for this tool");
                                        }
                                    }
                                }
                            });
                        });
                    });
            });
    }
}
