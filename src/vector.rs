use crate::canvas::VectorControlPoint;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct MeshVertex {
    pub x: f32,
    pub y: f32,
    pub u: f32,
    pub v: f32,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct StrokeMesh {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
}

/// Centripetal Catmull-Rom spline evaluation at parameter `t` in [0, 1]
/// for the segment between `p1` and `p2` (4 control points needed).
/// Alpha = 0.5 gives centripetal parameterization which prevents cusps
/// and self-intersections on unevenly spaced points.
pub fn centripetal_catmull_rom(
    p0: &VectorControlPoint,
    p1: &VectorControlPoint,
    p2: &VectorControlPoint,
    p3: &VectorControlPoint,
    t: f32,
    alpha: f32,
) -> VectorControlPoint {
    let tj0 = 0.0;
    let tj1 = chord_len(p0, p1).powf(alpha) + tj0;
    let tj2 = chord_len(p1, p2).powf(alpha) + tj1;
    let tj3 = chord_len(p2, p3).powf(alpha) + tj2;

    let t_local = tj1 + (tj2 - tj1) * t;

    let a1 = lerp_point(p0, p1, tj0, tj1, t_local);
    let a2 = lerp_point(p1, p2, tj1, tj2, t_local);
    let a3 = lerp_point(p2, p3, tj2, tj3, t_local);

    let b1 = lerp_point(&a1, &a2, tj0, tj2, t_local);
    let b2 = lerp_point(&a2, &a3, tj1, tj3, t_local);

    lerp_point(&b1, &b2, tj1, tj2, t_local)
}

fn chord_len(a: &VectorControlPoint, b: &VectorControlPoint) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dx * dx + dy * dy).sqrt()
}

fn lerp_point(
    a: &VectorControlPoint,
    b: &VectorControlPoint,
    ta: f32,
    tb: f32,
    t: f32,
) -> VectorControlPoint {
    let d = tb - ta;
    if d.abs() < 1e-8 {
        return b.clone();
    }
    let s = (t - ta) / d;
    VectorControlPoint {
        x: a.x + (b.x - a.x) * s,
        y: a.y + (b.y - a.y) * s,
        pressure: a.pressure + (b.pressure - a.pressure) * s,
        tilt_x: a.tilt_x + (b.tilt_x - a.tilt_x) * s,
        tilt_y: a.tilt_y + (b.tilt_y - a.tilt_y) * s,
    }
}

/// Sample a variable-pressure stroke along a centripetal Catmull-Rom spline.
/// Returns evenly-spaced interpolated control points along the curve.
pub fn sample_stroke(
    control_points: &[VectorControlPoint],
    min_segment_pixels: f32,
    alpha: f32,
) -> Vec<VectorControlPoint> {
    if control_points.len() < 2 {
        return control_points.to_vec();
    }
    if control_points.len() == 2 {
        let p0 = &control_points[0];
        let p1 = &control_points[1];
        let dist = chord_len(p0, p1);
        let steps = (dist / min_segment_pixels).ceil() as usize;
        let mut result = Vec::with_capacity(steps + 1);
        for i in 0..=steps {
            let t = i as f32 / steps.max(1) as f32;
            result.push(VectorControlPoint {
                x: p0.x + (p1.x - p0.x) * t,
                y: p0.y + (p1.y - p0.y) * t,
                pressure: p0.pressure + (p1.pressure - p0.pressure) * t,
                tilt_x: p0.tilt_x + (p1.tilt_x - p0.tilt_x) * t,
                tilt_y: p0.tilt_y + (p1.tilt_y - p0.tilt_y) * t,
            });
        }
        return result;
    }

    let mut result = Vec::new();
    for k in 3..=control_points.len() {
        let p0 = if k >= 4 {
            &control_points[k - 4]
        } else {
            &control_points[k - 3]
        };
        let p1 = &control_points[k - 3];
        let p2 = &control_points[k - 2];
        let p3 = &control_points[k - 1];

        let seg_len = chord_len(p1, p2);
        let steps = (seg_len / min_segment_pixels).ceil() as usize;
        let start_i = if k == 3 { 0 } else { 1 };

        for i in start_i..=steps {
            let t = i as f32 / steps.max(1) as f32;
            result.push(centripetal_catmull_rom(p0, p1, p2, p3, t, alpha));
        }
    }

    let len = control_points.len();
    let p0 = if len >= 3 {
        &control_points[len - 3]
    } else {
        &control_points[len - 2]
    };
    let p1 = &control_points[len - 2];
    let p2 = &control_points[len - 1];
    let p3 = p2;

    let seg_len = chord_len(p1, p2);
    let steps = (seg_len / min_segment_pixels).ceil() as usize;

    for i in 1..=steps {
        let t = i as f32 / steps.max(1) as f32;
        result.push(centripetal_catmull_rom(p0, p1, p2, p3, t, alpha));
    }

    result
}

/// Generate a triangle strip mesh for a vector stroke with pressure-controlled width.
/// The stroke is rendered as a ribbon whose width at each sample point is
/// proportional to `pressure * base_radius`.
#[allow(dead_code)]
pub fn generate_stroke_mesh(
    samples: &[VectorControlPoint],
    base_radius: f32,
    min_radius: f32,
) -> StrokeMesh {
    if samples.len() < 2 {
        return StrokeMesh {
            vertices: Vec::new(),
            indices: Vec::new(),
        };
    }

    let mut vertices = Vec::with_capacity(samples.len() * 2);
    let mut indices = Vec::with_capacity((samples.len() - 1) * 6);

    let mut total_len = 0.0f32;
    let mut seg_lens = Vec::with_capacity(samples.len());
    seg_lens.push(0.0);
    for i in 1..samples.len() {
        let d = chord_len(&samples[i - 1], &samples[i]);
        total_len += d;
        seg_lens.push(total_len);
    }

    for (i, pt) in samples.iter().enumerate() {
        let radius = (pt.pressure * base_radius).max(min_radius);

        let tangent = if i == 0 {
            let dx = samples[1].x - samples[0].x;
            let dy = samples[1].y - samples[0].y;
            let len = (dx * dx + dy * dy).sqrt().max(1e-8);
            (dx / len, dy / len)
        } else if i == samples.len() - 1 {
            let dx = samples[i].x - samples[i - 1].x;
            let dy = samples[i].y - samples[i - 1].y;
            let len = (dx * dx + dy * dy).sqrt().max(1e-8);
            (dx / len, dy / len)
        } else {
            let dx = samples[i + 1].x - samples[i - 1].x;
            let dy = samples[i + 1].y - samples[i - 1].y;
            let len = (dx * dx + dy * dy).sqrt().max(1e-8);
            (dx / len, dy / len)
        };

        let nx = -tangent.1;
        let ny = tangent.0;

        let u = if total_len > 0.0 {
            seg_lens[i] / total_len
        } else {
            i as f32 / (samples.len() - 1).max(1) as f32
        };

        vertices.push(MeshVertex {
            x: pt.x + nx * radius,
            y: pt.y + ny * radius,
            u,
            v: 0.0,
        });
        vertices.push(MeshVertex {
            x: pt.x - nx * radius,
            y: pt.y - ny * radius,
            u,
            v: 1.0,
        });
    }

    for i in 0..samples.len() - 1 {
        let a = (i * 2) as u32;
        let b = (i * 2 + 1) as u32;
        let c = (i * 2 + 2) as u32;
        let d = (i * 2 + 3) as u32;
        indices.push(a);
        indices.push(b);
        indices.push(c);
        indices.push(b);
        indices.push(d);
        indices.push(c);
    }

    StrokeMesh { vertices, indices }
}

pub const CONTROL_POINT_HIT_RADIUS: f32 = 6.0;

/// Find the nearest control point to a given world position within hit radius.
/// Returns (stroke_index, point_index) if found.
pub fn hit_test_control_point(
    strokes: &[crate::canvas::VectorStroke],
    world_x: f32,
    world_y: f32,
) -> Option<(usize, usize)> {
    for (si, stroke) in strokes.iter().enumerate() {
        for (pi, cp) in stroke.control_points.iter().enumerate() {
            let dx = cp.x - world_x;
            let dy = cp.y - world_y;
            if dx * dx + dy * dy <= CONTROL_POINT_HIT_RADIUS * CONTROL_POINT_HIT_RADIUS {
                return Some((si, pi));
            }
        }
    }
    None
}

/// Compute the bounding box of a set of control points
#[allow(dead_code)]
pub fn stroke_bounds(stroke: &crate::canvas::VectorStroke) -> Option<(f32, f32, f32, f32)> {
    let mut iter = stroke.control_points.iter();
    let first = iter.next()?;
    let mut min_x = first.x;
    let mut min_y = first.y;
    let mut max_x = first.x;
    let mut max_y = first.y;
    for cp in iter {
        min_x = min_x.min(cp.x);
        min_y = min_y.min(cp.y);
        max_x = max_x.max(cp.x);
        max_y = max_y.max(cp.y);
    }
    Some((min_x, min_y, max_x, max_y))
}

/// Generate egui meshes for rendering a vector layer's strokes as spline ribbons.
/// Each stroke uses its own color with the given layer opacity applied.
/// All vertices are in world coordinates.
pub fn generate_layer_egui_meshes(
    strokes: &[crate::canvas::VectorStroke],
    base_radius: f32,
    min_radius: f32,
    layer_opacity: f32,
) -> Vec<egui::Mesh> {
    let mut meshes = Vec::new();

    for stroke in strokes {
        if stroke.control_points.len() < 2 {
            continue;
        }
        let samples = sample_stroke(&stroke.control_points, 2.0, 0.5);
        let radius = base_radius * stroke.width;
        let stroke_mesh = generate_stroke_mesh(&samples, radius, min_radius);
        if stroke_mesh.vertices.is_empty() {
            continue;
        }

        let stroke_color = egui::Color32::from_rgba_premultiplied(
            (stroke.color[0] * 255.0) as u8,
            (stroke.color[1] * 255.0) as u8,
            (stroke.color[2] * 255.0) as u8,
            (layer_opacity * 255.0) as u8,
        );

        let mut egui_mesh = egui::Mesh::default();
        for v in &stroke_mesh.vertices {
            egui_mesh.vertices.push(egui::epaint::Vertex {
                pos: egui::pos2(v.x, v.y),
                uv: egui::pos2(0.0, 0.0),
                color: stroke_color,
            });
        }
        for i in (0..stroke_mesh.indices.len()).step_by(3) {
            egui_mesh.add_triangle(
                stroke_mesh.indices[i],
                stroke_mesh.indices[i + 1],
                stroke_mesh.indices[i + 2],
            );
        }
        meshes.push(egui_mesh);
    }

    meshes
}

/// Export vector strokes to an SVG string.
/// Samples each stroke at a high resolution and outputs polylines with stroke colors.
pub fn export_strokes_svg(strokes: &[crate::canvas::VectorStroke], width: u32, height: u32) -> String {
    let mut svg = String::new();
    svg.push_str(&format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {} {}" width="{}" height="{}">
"#,
        width, height, width, height
    ));

    for stroke in strokes {
        if stroke.control_points.len() < 2 {
            continue;
        }
        let samples = sample_stroke(&stroke.control_points, 1.0, 0.5);
        if samples.is_empty() {
            continue;
        }

        let r = (stroke.color[0] * 255.0) as u8;
        let g = (stroke.color[1] * 255.0) as u8;
        let b = (stroke.color[2] * 255.0) as u8;
        let stroke_width = stroke.width * 2.0;

        svg.push_str(&format!(
            r#"  <polyline fill="none" stroke="rgb({},{},{})" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round" points=""#,
            r, g, b, stroke_width
        ));

        for (i, pt) in samples.iter().enumerate() {
            if i > 0 {
                svg.push(' ');
            }
            svg.push_str(&format!("{},{}", pt.x, pt.y));
        }
        svg.push_str("\"/>\n");
    }

    svg.push_str("</svg>\n");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cp(x: f32, y: f32, pressure: f32) -> VectorControlPoint {
        VectorControlPoint { x, y, pressure, tilt_x: 0.0, tilt_y: 0.0 }
    }

    #[test]
    fn test_centripetal_catmull_rom_evaluates() {
        let p0 = make_cp(0.0, 0.0, 0.5);
        let p1 = make_cp(50.0, 50.0, 0.6);
        let p2 = make_cp(100.0, 100.0, 0.7);
        let p3 = make_cp(150.0, 100.0, 0.8);
        let pt = centripetal_catmull_rom(&p0, &p1, &p2, &p3, 0.5, 0.5);
        assert!((pt.x - 75.0).abs() < 30.0);
        assert!((pt.pressure - 0.65).abs() < 0.2);
    }

    #[test]
    fn test_sample_stroke_two_points() {
        let pts = vec![make_cp(0.0, 0.0, 1.0), make_cp(100.0, 0.0, 1.0)];
        let sampled = sample_stroke(&pts, 10.0, 0.5);
        assert!(sampled.len() >= 2);
        assert_eq!(sampled[0].x, 0.0);
        assert_eq!(sampled.last().unwrap().x, 100.0);
    }

    #[test]
    fn test_generate_stroke_mesh() {
        let pts = vec![
            make_cp(0.0, 0.0, 0.5),
            make_cp(50.0, 50.0, 0.8),
            make_cp(100.0, 0.0, 0.5),
        ];
        let mesh = generate_stroke_mesh(&pts, 10.0, 1.0);
        assert_eq!(mesh.vertices.len(), 6);
        assert_eq!(mesh.indices.len(), 12);
    }

    #[test]
    fn test_hit_test_control_point() {
        let stroke = crate::canvas::VectorStroke {
            control_points: vec![
                make_cp(10.0, 10.0, 1.0),
                make_cp(100.0, 100.0, 1.0),
            ],
            brush_preset_id: 1,
            color: [0.0, 0.0, 0.0],
            width: 1.0,
        };
        let result = hit_test_control_point(&[stroke], 11.0, 11.0);
        assert!(result.is_some());
        let (si, pi) = result.unwrap();
        assert_eq!(si, 0);
        assert_eq!(pi, 0);
    }

    #[test]
    fn test_generate_empty_mesh() {
        let mesh = generate_stroke_mesh(&[], 10.0, 1.0);
        assert!(mesh.vertices.is_empty());
        assert!(mesh.indices.is_empty());

        let mesh = generate_stroke_mesh(&[make_cp(0.0, 0.0, 1.0)], 10.0, 1.0);
        assert!(mesh.vertices.is_empty());
        assert!(mesh.indices.is_empty());
    }

    #[test]
    fn test_stroke_bounds() {
        let stroke = crate::canvas::VectorStroke {
            control_points: vec![
                make_cp(10.0, 20.0, 1.0),
                make_cp(100.0, 200.0, 1.0),
            ],
            brush_preset_id: 1,
            color: [0.0, 0.0, 0.0],
            width: 1.0,
        };
        let bounds = stroke_bounds(&stroke).unwrap();
        assert!((bounds.0 - 10.0).abs() < 0.01);
        assert!((bounds.1 - 20.0).abs() < 0.01);
        assert!((bounds.2 - 100.0).abs() < 0.01);
        assert!((bounds.3 - 200.0).abs() < 0.01);
    }

    #[test]
    fn test_centripetal_interpolation_preserves_endpoints() {
        let p0 = make_cp(0.0, 0.0, 0.0);
        let p1 = make_cp(50.0, 0.0, 0.5);
        let p2 = make_cp(100.0, 0.0, 1.0);
        let p3 = make_cp(150.0, 0.0, 1.0);
        let pt_start = centripetal_catmull_rom(&p0, &p1, &p2, &p3, 0.0, 0.5);
        let pt_end = centripetal_catmull_rom(&p0, &p1, &p2, &p3, 1.0, 0.5);
        assert!((pt_start.x - 50.0).abs() < 0.1);
        assert!((pt_end.x - 100.0).abs() < 0.1);
    }
}
