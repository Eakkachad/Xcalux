use crate::canvas::{Layer, SelectionMask};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum SelectionMode {
    Replace,
    Add,
    Subtract,
    Intersect,
}

#[derive(Debug, Clone, Copy)]
pub struct SelectionRect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl SelectionRect {
    pub fn normalized(&self) -> Self {
        Self {
            x0: self.x0.min(self.x1),
            y0: self.y0.min(self.y1),
            x1: self.x0.max(self.x1),
            y1: self.y0.max(self.y1),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LassoPoints {
    pub points: Vec<(f32, f32)>,
}

pub fn apply_rect_selection(
    mask: &mut SelectionMask,
    rect: SelectionRect,
    mode: SelectionMode,
    feather_radius: f32,
    antialias: bool,
) {
    let r = rect.normalized();

    match mode {
        SelectionMode::Replace => {
            mask.tiles.clear();
            mask.is_active = true;
        }
        SelectionMode::Add | SelectionMode::Subtract | SelectionMode::Intersect => {
            if !mask.is_active {
                mask.is_active = true;
            }
        }
    }

    let tx0 = (r.x0 as i32).div_euclid(64);
    let ty0 = (r.y0 as i32).div_euclid(64);
    let tx1 = (r.x1 as i32).div_euclid(64);
    let ty1 = (r.y1 as i32).div_euclid(64);

    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let tile = mask.tiles.entry((tx, ty)).or_insert_with(|| {
                Box::new([0u8; 4096])
            });

            for ly in 0..64 {
                for lx in 0..64 {
                    let wx = tx * 64 + lx;
                    let wy = ty * 64 + ly;
                    let inside = wx >= r.x0 as i32 && wx < r.x1 as i32
                        && wy >= r.y0 as i32 && wy < r.y1 as i32;

                    let mut val = if inside {
                        if feather_radius > 0.0 {
                            let dx = (wx as f32 - r.x0).min(r.x1 - wx as f32).min(feather_radius);
                            let dy = (wy as f32 - r.y0).min(r.y1 - wy as f32).min(feather_radius);
                            let d = dx.min(dy);
                            if d <= 0.0 { 255 } else { (255.0 * (1.0 - d / feather_radius)) as u8 }
                        } else {
                            255u8
                        }
                    } else {
                        0u8
                    };

                    if antialias && feather_radius == 0.0 {
                        let edge_dist = (wx as f32 - r.x0)
                            .min(r.x1 - wx as f32)
                            .min(wy as f32 - r.y0)
                            .min(r.y1 - wy as f32);
                        if edge_dist > 0.0 && edge_dist < 1.0 {
                            val = (val as f32 * edge_dist) as u8;
                        }
                    }

                    let idx = (ly * 64 + lx) as usize;
                    match mode {
                        SelectionMode::Replace => {
                            tile[idx] = val;
                        }
                        SelectionMode::Add => {
                            tile[idx] = tile[idx].saturating_add(val).min(255);
                        }
                        SelectionMode::Subtract => {
                            tile[idx] = if val > tile[idx] { 0 } else { tile[idx] - val };
                        }
                        SelectionMode::Intersect => {
                            tile[idx] = tile[idx].min(val);
                        }
                    }
                }
            }
        }
    }
}

pub fn apply_ellipse_selection(
    mask: &mut SelectionMask,
    rect: SelectionRect,
    mode: SelectionMode,
    feather_radius: f32,
    antialias: bool,
) {
    let r = rect.normalized();

    match mode {
        SelectionMode::Replace => {
            mask.tiles.clear();
            mask.is_active = true;
        }
        SelectionMode::Add | SelectionMode::Subtract | SelectionMode::Intersect => {
            if !mask.is_active {
                mask.is_active = true;
            }
        }
    }

    let tx0 = (r.x0 as i32).div_euclid(64);
    let ty0 = (r.y0 as i32).div_euclid(64);
    let tx1 = (r.x1 as i32).div_euclid(64);
    let ty1 = (r.y1 as i32).div_euclid(64);

    let center_x = (r.x0 + r.x1) / 2.0;
    let center_y = (r.y0 + r.y1) / 2.0;
    let rx = (r.x1 - r.x0) / 2.0;
    let ry = (r.y1 - r.y0) / 2.0;

    if rx <= 0.0 || ry <= 0.0 {
        return;
    }

    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let tile = mask.tiles.entry((tx, ty)).or_insert_with(|| {
                Box::new([0u8; 4096])
            });

            for ly in 0..64 {
                for lx in 0..64 {
                    let wx = tx * 64 + lx;
                    let wy = ty * 64 + ly;
                    
                    let dx = (wx as f32 - center_x) / rx;
                    let dy = (wy as f32 - center_y) / ry;
                    let dist_sq = dx * dx + dy * dy;
                    
                    let inside = dist_sq <= 1.0;

                    let mut val = if inside {
                        if feather_radius > 0.0 {
                            let dist = dist_sq.sqrt();
                            let edge_dist = (1.0 - dist) * rx.min(ry);
                            if edge_dist >= feather_radius {
                                255
                            } else {
                                (255.0 * (edge_dist / feather_radius)) as u8
                            }
                        } else {
                            255u8
                        }
                    } else {
                        0u8
                    };

                    if antialias && feather_radius == 0.0 {
                        let dist = dist_sq.sqrt();
                        let pixel_dist = (1.0 - dist) * rx.min(ry);
                        if pixel_dist > 0.0 && pixel_dist < 1.0 {
                            val = (val as f32 * pixel_dist) as u8;
                        }
                    }

                    let idx = (ly * 64 + lx) as usize;
                    match mode {
                        SelectionMode::Replace => {
                            tile[idx] = val;
                        }
                        SelectionMode::Add => {
                            tile[idx] = tile[idx].saturating_add(val).min(255);
                        }
                        SelectionMode::Subtract => {
                            tile[idx] = if val > tile[idx] { 0 } else { tile[idx] - val };
                        }
                        SelectionMode::Intersect => {
                            tile[idx] = tile[idx].min(val);
                        }
                    }
                }
            }
        }
    }
}


pub fn apply_lasso_selection(
    mask: &mut SelectionMask,
    lasso: &LassoPoints,
    mode: SelectionMode,
    _feather_radius: f32,
    _antialias: bool,
) {
    if lasso.points.len() < 3 {
        return;
    }

    match mode {
        SelectionMode::Replace => {
            mask.tiles.clear();
            mask.is_active = true;
        }
        SelectionMode::Add | SelectionMode::Subtract | SelectionMode::Intersect => {
            if !mask.is_active {
                mask.is_active = true;
            }
        }
    }

    let min_x = lasso.points.iter().map(|p| p.0).min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0.0) as i32 - 64;
    let min_y = lasso.points.iter().map(|p| p.1).min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0.0) as i32 - 64;
    let max_x = lasso.points.iter().map(|p| p.0).max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0.0) as i32 + 64;
    let max_y = lasso.points.iter().map(|p| p.1).max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0.0) as i32 + 64;

    let tx0 = min_x.div_euclid(64);
    let ty0 = min_y.div_euclid(64);
    let tx1 = max_x.div_euclid(64);
    let ty1 = max_y.div_euclid(64);

    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let tile = mask.tiles.entry((tx, ty)).or_insert_with(|| {
                Box::new([0u8; 4096])
            });

            for ly in 0..64 {
                for lx in 0..64 {
                    let wx = (tx * 64 + lx) as f32 + 0.5;
                    let wy = (ty * 64 + ly) as f32 + 0.5;

                    let inside = point_in_polygon(wx, wy, &lasso.points);
                    let val = if inside { 255u8 } else { 0u8 };

                    let idx = (ly * 64 + lx) as usize;
                    match mode {
                        SelectionMode::Replace => {
                            tile[idx] = val;
                        }
                        SelectionMode::Add => {
                            tile[idx] = tile[idx].saturating_add(val).min(255);
                        }
                        SelectionMode::Subtract => {
                            tile[idx] = if val > tile[idx] { 0 } else { tile[idx] - val };
                        }
                        SelectionMode::Intersect => {
                            tile[idx] = tile[idx].min(val);
                        }
                    }
                }
            }
        }
    }
}

fn point_in_polygon(px: f32, py: f32, polygon: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let yi = polygon[i].1;
        let yj = polygon[j].1;
        if ((yi > py) != (yj > py))
            && (px < (polygon[j].0 - polygon[i].0) * (py - yi) / (yj - yi) + polygon[i].0)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn color_diff(c1: &[u16; 4], c2: &[u16; 4]) -> u32 {
    let dr = (c1[0] as i32 - c2[0] as i32).abs();
    let dg = (c1[1] as i32 - c2[1] as i32).abs();
    let db = (c1[2] as i32 - c2[2] as i32).abs();
    let da = (c1[3] as i32 - c2[3] as i32).abs();
    let max_chan = 32768u32;
    let r2 = (dr as u64 * dr as u64) / (max_chan as u64);
    let g2 = (dg as u64 * dg as u64) / (max_chan as u64);
    let b2 = (db as u64 * db as u64) / (max_chan as u64);
    let a2 = (da as u64 * da as u64) / (max_chan as u64);
    ((r2 + g2 + b2 + a2) as u32).min(65535)
}

fn get_pixel_at(layers: &[&Layer], layer_order: &[u32], x: i32, y: i32, sample_all: bool) -> [u16; 4] {
    let tx = x.div_euclid(64);
    let ty = y.div_euclid(64);
    let lx = x.rem_euclid(64).unsigned_abs() as usize;
    let ly = y.rem_euclid(64).unsigned_abs() as usize;

    if sample_all {
        let mut accumulated = [0u16; 4];
        let mut count = 0u16;
        for &id in layer_order {
            if let Some(layer) = layers.iter().find(|l| l.id == id) {
                if !layer.visible { continue; }
                if let Some(tile) = layer.tiles.get(&(tx, ty)) {
                    accumulated[0] = accumulated[0].saturating_add(tile.pixels[ly][lx][0]);
                    accumulated[1] = accumulated[1].saturating_add(tile.pixels[ly][lx][1]);
                    accumulated[2] = accumulated[2].saturating_add(tile.pixels[ly][lx][2]);
                    accumulated[3] = accumulated[3].saturating_add(tile.pixels[ly][lx][3]);
                    count += 1;
                }
            }
        }
        if count > 0 {
            [accumulated[0] / count, accumulated[1] / count, accumulated[2] / count, accumulated[3] / count]
        } else {
            [0, 0, 0, 0]
        }
    } else {
        let layer = layers.first();
        match layer {
            Some(l) => {
                if let Some(tile) = l.tiles.get(&(tx, ty)) {
                    tile.pixels[ly][lx]
                } else {
                    [0, 0, 0, 0]
                }
            }
            None => [0, 0, 0, 0],
        }
    }
}

pub fn magic_wand_select(
    mask: &mut SelectionMask,
    layers: &[&Layer],
    layer_order: &[u32],
    start_x: i32,
    start_y: i32,
    tolerance: u32,
    contiguous: bool,
    sample_all: bool,
    mode: SelectionMode,
    canvas_width: i32,
    canvas_height: i32,
) {
    if start_x < 0 || start_x >= canvas_width || start_y < 0 || start_y >= canvas_height {
        return;
    }

    let target_color = get_pixel_at(layers, layer_order, start_x, start_y, sample_all);

    match mode {
        SelectionMode::Replace => {
            mask.tiles.clear();
            mask.is_active = true;
        }
        SelectionMode::Add | SelectionMode::Subtract | SelectionMode::Intersect => {
            if !mask.is_active {
                mask.is_active = true;
            }
        }
    }

    if contiguous {
        // Scanline flood fill for contiguous mode
        let mut queue = VecDeque::new();
        queue.push_back((start_x, start_y));
        let mut visited_local = ahash::AHashSet::new();
        visited_local.insert((start_x, start_y));

        let tolerance_sq = tolerance * tolerance;

        while let Some((cx, cy)) = queue.pop_front() {
            let pixel = get_pixel_at(layers, layer_order, cx, cy, sample_all);
            if color_diff(&pixel, &target_color) > tolerance_sq {
                continue;
            }

            let tx = cx.div_euclid(64);
            let ty = cy.div_euclid(64);
            let lx = cx.rem_euclid(64).unsigned_abs() as usize;
            let ly = cy.rem_euclid(64).unsigned_abs() as usize;
            let tile = mask.tiles.entry((tx, ty)).or_insert_with(|| Box::new([0u8; 4096]));
            let idx = ly * 64 + lx;

            match mode {
                SelectionMode::Replace => tile[idx] = 255,
                SelectionMode::Add => tile[idx] = tile[idx].saturating_add(255).min(255),
                SelectionMode::Subtract => tile[idx] = tile[idx].saturating_sub(255),
                SelectionMode::Intersect => { if tile[idx] > 0 { tile[idx] = 255; } }
            }

            // Scan left
            let mut scan_x = cx - 1;
            while scan_x >= 0 && scan_x < canvas_width {
                if visited_local.insert((scan_x, cy)) {
                    let p = get_pixel_at(layers, layer_order, scan_x, cy, sample_all);
                    if color_diff(&p, &target_color) <= tolerance_sq {
                        queue.push_back((scan_x, cy));
                    } else {
                        break;
                    }
                } else {
                    break;
                }
                scan_x -= 1;
            }

            // Scan right
            let mut scan_x = cx + 1;
            while scan_x < canvas_width {
                if visited_local.insert((scan_x, cy)) {
                    let p = get_pixel_at(layers, layer_order, scan_x, cy, sample_all);
                    if color_diff(&p, &target_color) <= tolerance_sq {
                        queue.push_back((scan_x, cy));
                    } else {
                        break;
                    }
                } else {
                    break;
                }
                scan_x += 1;
            }

            // Check rows above and below
            for &ny in &[cy - 1, cy + 1] {
                if ny < 0 || ny >= canvas_height { continue; }
                if visited_local.contains(&(cx, ny)) { continue; }
                let p = get_pixel_at(layers, layer_order, cx, ny, sample_all);
                if color_diff(&p, &target_color) <= tolerance_sq {
                    if visited_local.insert((cx, ny)) {
                        queue.push_back((cx, ny));
                    }
                }
            }
        }
    } else {
        // Non-contiguous: simply select all pixels within tolerance across the canvas
        let tx0 = 0i32.div_euclid(64);
        let ty0 = 0i32.div_euclid(64);
        let tx1 = (canvas_width - 1).div_euclid(64);
        let ty1 = (canvas_height - 1).div_euclid(64);

        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let tile = mask.tiles.entry((tx, ty)).or_insert_with(|| Box::new([0u8; 4096]));
                for ly in 0..64 {
                    for lx in 0..64 {
                        let wx = tx * 64 + lx as i32;
                        let wy = ty * 64 + ly as i32;
                        if wx < 0 || wx >= canvas_width || wy < 0 || wy >= canvas_height {
                            continue;
                        }
                        let pixel = get_pixel_at(layers, layer_order, wx, wy, sample_all);
                        if color_diff(&pixel, &target_color) <= tolerance * tolerance {
                            let idx = ly * 64 + lx;
                            match mode {
                                SelectionMode::Replace => tile[idx] = 255,
                                SelectionMode::Add => tile[idx] = tile[idx].saturating_add(255).min(255),
                                SelectionMode::Subtract => tile[idx] = tile[idx].saturating_sub(255),
                                SelectionMode::Intersect => { if tile[idx] > 0 { tile[idx] = 255; } }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn clear_selection(mask: &mut SelectionMask) {
    mask.tiles.clear();
    mask.is_active = false;
}

pub fn invert_selection(mask: &mut SelectionMask, canvas_w: u32, canvas_h: u32) {
    if !mask.is_active {
        mask.is_active = true;
        let tx0 = 0;
        let ty0 = 0;
        let tx1 = (canvas_w as i32).div_euclid(64);
        let ty1 = (canvas_h as i32).div_euclid(64);
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let tile = mask.tiles.entry((tx, ty)).or_insert_with(|| Box::new([0u8; 4096]));
                for ly in 0..64 {
                    for lx in 0..64 {
                        let idx = (ly * 64 + lx) as usize;
                        tile[idx] = 255 - tile[idx];
                    }
                }
            }
        }
    } else {
        for tile in mask.tiles.values_mut() {
            for i in 0..4096 {
                tile[i] = 255 - tile[i];
            }
        }
    }
}
