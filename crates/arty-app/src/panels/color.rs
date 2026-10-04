//! Color wheel (hue ring + SV square), sliders, color set and history.

use std::f32::consts::TAU;

use egui::{Color32, CornerRadius, Mesh, Pos2, Rect, Sense, Shape, Stroke, Vec2, pos2};
use egui_phosphor::regular as icon;

use super::section;
use crate::studio::{Rgb, Studio, hsv_to_rgb};

fn to32(c: Rgb) -> Color32 {
    Color32::from_rgb((c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8)
}

#[derive(Clone, Copy, PartialEq)]
enum WheelDrag {
    Ring,
    Square,
}

pub fn wheel_ui(ui: &mut egui::Ui, studio: &mut Studio) {
    let side = ui.available_width().clamp(120.0, 260.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(side), Sense::click_and_drag());
    let center = rect.center();
    let r_out = side * 0.5 - 2.0;
    let ring = (side * 0.075).clamp(10.0, 18.0);
    let r_in = r_out - ring;
    let half = (r_in - 6.0) / std::f32::consts::SQRT_2;
    let square = Rect::from_center_size(center, Vec2::splat(half * 2.0));
    let [h, s, v] = studio.color.hsv;

    // Interaction.
    let drag_id = ui.id().with("wheel-drag");
    if let Some(p) = resp.interact_pointer_pos() {
        let mut mode: Option<WheelDrag> = ui.data(|d| d.get_temp(drag_id));
        if resp.drag_started() || resp.clicked() || mode.is_none() {
            let d = p.distance(center);
            mode = if d >= r_in - 2.0 && d <= r_out + 4.0 {
                Some(WheelDrag::Ring)
            } else if square.expand(4.0).contains(p) {
                Some(WheelDrag::Square)
            } else {
                None
            };
            ui.data_mut(|d| {
                if let Some(m) = mode {
                    d.insert_temp(drag_id, m);
                } else {
                    d.remove::<WheelDrag>(drag_id);
                }
            });
        }
        match mode {
            Some(WheelDrag::Ring) => {
                let a = (p.y - center.y).atan2(p.x - center.x);
                studio.set_main_hsv([(a / TAU).rem_euclid(1.0), s, v]);
            }
            Some(WheelDrag::Square) => {
                let ns = ((p.x - square.left()) / square.width()).clamp(0.0, 1.0);
                let nv = 1.0 - ((p.y - square.top()) / square.height()).clamp(0.0, 1.0);
                studio.set_main_hsv([h, ns, nv]);
            }
            None => {}
        }
    }
    if !resp.dragged() && !resp.is_pointer_button_down_on() {
        ui.data_mut(|d| d.remove::<WheelDrag>(drag_id));
    }

    let [h, s, v] = studio.color.hsv;
    let painter = ui.painter();

    // Hue ring.
    let mut mesh = Mesh::default();
    let n = 120;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let a = t * TAU;
        let dir = Vec2::new(a.cos(), a.sin());
        let c = to32(hsv_to_rgb(t, 1.0, 1.0));
        mesh.colored_vertex(center + dir * r_in, c);
        mesh.colored_vertex(center + dir * r_out, c);
        if i > 0 {
            let b = (i as u32) * 2;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    painter.add(Shape::mesh(mesh));

    // Saturation/value square, as a grid so the gradient is exact.
    let mut mesh = Mesh::default();
    let g = 12;
    for yi in 0..=g {
        for xi in 0..=g {
            let (fx, fy) = (xi as f32 / g as f32, yi as f32 / g as f32);
            let pos = pos2(square.left() + fx * square.width(), square.top() + fy * square.height());
            mesh.colored_vertex(pos, to32(hsv_to_rgb(h, fx, 1.0 - fy)));
        }
    }
    for yi in 0..g {
        for xi in 0..g {
            let i = (yi * (g + 1) + xi) as u32;
            let w = (g + 1) as u32;
            mesh.add_triangle(i, i + 1, i + w);
            mesh.add_triangle(i + 1, i + w + 1, i + w);
        }
    }
    painter.add(Shape::mesh(mesh));

    // Markers.
    let a = h * TAU;
    let hue_pos = center + Vec2::new(a.cos(), a.sin()) * (r_in + ring * 0.5);
    marker(painter, hue_pos, ring * 0.38);
    let sv_pos = pos2(square.left() + s * square.width(), square.top() + (1.0 - v) * square.height());
    marker(painter, sv_pos, 5.0);

    ui.add_space(6.0);
    sliders(ui, studio);
}

fn marker(painter: &egui::Painter, p: Pos2, r: f32) {
    painter.circle_stroke(p, r, Stroke::new(2.0, Color32::BLACK));
    painter.circle_stroke(p, r - 1.5, Stroke::new(1.5, Color32::WHITE));
}

fn sliders(ui: &mut egui::Ui, studio: &mut Studio) {
    let mut hsv = studio.color.hsv;
    let mut changed = false;
    egui::Grid::new("hsv").num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
        for (label, i, max) in [("H", 0, 360.0f32), ("S", 1, 100.0), ("V", 2, 100.0)] {
            ui.label(label);
            let mut val = hsv[i] * max;
            if ui.add(egui::Slider::new(&mut val, 0.0..=max).max_decimals(0)).changed() {
                hsv[i] = (val / max).clamp(0.0, if i == 0 { 0.9999 } else { 1.0 });
                changed = true;
            }
            ui.end_row();
        }
    });
    if changed {
        studio.set_main_hsv(hsv);
    }

    let c = studio.color.main;
    let mut hex = format!("{:02X}{:02X}{:02X}", (c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8);
    ui.horizontal(|ui| {
        ui.label("#");
        let r = ui.add(egui::TextEdit::singleline(&mut hex).desired_width(64.0).char_limit(6));
        if r.changed() && hex.len() == 6
            && let Ok(v) = u32::from_str_radix(&hex, 16) {
                studio.set_main_color([(v >> 16) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0]);
            }
        let (r, _) = ui.allocate_exact_size(Vec2::new(28.0, 18.0), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(3), to32(studio.color.main));
        let (r2, resp) = ui.allocate_exact_size(Vec2::new(28.0, 18.0), Sense::click());
        ui.painter().rect_filled(r2, CornerRadius::same(3), to32(studio.color.sub));
        if resp.on_hover_text("Sub color — click to swap (X)").clicked() {
            studio.swap_colors();
        }
    });
}

pub fn swatches_ui(ui: &mut egui::Ui, studio: &mut Studio) {
    section(ui, "COLOR SET");
    let mut pick = None;
    let mut remove = None;
    let cell = 20.0;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(3.0);
        for (i, &c) in studio.color.swatches.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
            swatch(ui, r, c, c == studio.color.main);
            if resp.clicked() {
                pick = Some(c);
            }
            resp.context_menu(|ui| {
                if ui.button(format!("{}  Remove", icon::TRASH)).clicked() {
                    remove = Some(i);
                    ui.close();
                }
            });
        }
        if ui.add_sized([cell, cell], egui::Button::new(icon::PLUS)).on_hover_text("Add main color to set").clicked() {
            let c = studio.color.main;
            if !studio.color.swatches.contains(&c) {
                studio.color.swatches.push(c);
            }
        }
    });
    ui.add_space(8.0);
    section(ui, "HISTORY");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(3.0);
        for &c in &studio.color.recent {
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
            swatch(ui, r, c, c == studio.color.main);
            if resp.clicked() {
                pick = Some(c);
            }
        }
        if studio.color.recent.is_empty() {
            ui.weak("Colors you paint with appear here");
        }
    });
    if let Some(i) = remove {
        studio.color.swatches.remove(i);
    }
    if let Some(c) = pick {
        studio.set_main_color(c);
    }
}

fn swatch(ui: &egui::Ui, r: Rect, c: Rgb, selected: bool) {
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(3), to32(c));
    let stroke = if selected {
        Stroke::new(2.0, ui.visuals().selection.stroke.color)
    } else {
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
    };
    p.rect_stroke(r, CornerRadius::same(3), stroke, egui::StrokeKind::Inside);
}
