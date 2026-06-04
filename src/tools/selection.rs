use crate::canvas::{Layer, SelectionMask};
use std::collections::VecDeque;
use crate::tools::fill::FillOptions;

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
                            tile[idx] = tile[idx].saturating_add(val);
                        }
                        SelectionMode::Subtract => {
                            tile[idx] = tile[idx].saturating_sub(val);
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
                            tile[idx] = tile[idx].saturating_add(val);
                        }
                        SelectionMode::Subtract => {
                            tile[idx] = tile[idx].saturating_sub(val);
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
                            tile[idx] = tile[idx].saturating_add(val);
                        }
                        SelectionMode::Subtract => {
                            tile[idx] = tile[idx].saturating_sub(val);
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

pub fn apply_polygon_lasso_selection(
    mask: &mut SelectionMask,
    points: &[(f32, f32)],
    mode: SelectionMode,
    _feather_radius: f32,
    _antialias: bool,
) {
    if points.len() < 3 {
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

    let min_x = points.iter().map(|p| p.0).min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0.0) as i32 - 64;
    let min_y = points.iter().map(|p| p.1).min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0.0) as i32 - 64;
    let max_x = points.iter().map(|p| p.0).max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0.0) as i32 + 64;
    let max_y = points.iter().map(|p| p.1).max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0.0) as i32 + 64;

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

                    let inside = point_in_polygon(wx, wy, points);
                    let val = if inside { 255u8 } else { 0u8 };

                    let idx = (ly * 64 + lx) as usize;
                    match mode {
                        SelectionMode::Replace => {
                            tile[idx] = val;
                        }
                        SelectionMode::Add => {
                            tile[idx] = tile[idx].saturating_add(val);
                        }
                        SelectionMode::Subtract => {
                            tile[idx] = tile[idx].saturating_sub(val);
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

#[allow(clippy::too_many_arguments)]
pub fn magic_wand_select(
    mask: &mut SelectionMask,
    layers: &[&Layer],
    active_layer: &Layer,
    start_x: i32,
    start_y: i32,
    options: &FillOptions,
    mode: SelectionMode,
    canvas_width: i32,
    canvas_height: i32,
) {
    if start_x < 0 || start_x >= canvas_width || start_y < 0 || start_y >= canvas_height {
        return;
    }

    let target_color = crate::tools::fill::sample_reference(layers, active_layer, options.reference, start_x, start_y);

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

    let mut selected_pixels: ahash::AHashMap<(i32, i32), f32> = ahash::AHashMap::default();

    if options.contiguous {
        // Contiguous scanline search
        let mut queue = VecDeque::new();
        queue.push_back((start_x, start_y));
        let mut visited_local = ahash::AHashSet::new();
        visited_local.insert((start_x, start_y));

        while let Some((cx, cy)) = queue.pop_front() {
            let factor = match crate::tools::fill::is_fillable(layers, active_layer, cx, cy, target_color, options) {
                Some(f) => f,
                None => continue,
            };

            selected_pixels.insert((cx, cy), factor);

            // Scan left
            let mut scan_x = cx - 1;
            while scan_x >= 0 && scan_x < canvas_width {
                if visited_local.insert((scan_x, cy)) {
                    if crate::tools::fill::is_fillable(layers, active_layer, scan_x, cy, target_color, options).is_some() {
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
                    if crate::tools::fill::is_fillable(layers, active_layer, scan_x, cy, target_color, options).is_some() {
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
                if crate::tools::fill::is_fillable(layers, active_layer, cx, ny, target_color, options).is_some()
                    && visited_local.insert((cx, ny)) {
                        queue.push_back((cx, ny));
                    }
            }
        }
    } else {
        // Non-contiguous: check all pixels in canvas bounds
        let tx0 = 0i32.div_euclid(64);
        let ty0 = 0i32.div_euclid(64);
        let tx1 = (canvas_width - 1).div_euclid(64);
        let ty1 = (canvas_height - 1).div_euclid(64);

        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                for ly in 0..64 {
                    for lx in 0..64 {
                        let wx = tx * 64 + lx;
                        let wy = ty * 64 + ly;
                        if wx < 0 || wx >= canvas_width || wy < 0 || wy >= canvas_height {
                            continue;
                        }
                        if let Some(f) = crate::tools::fill::is_fillable(layers, active_layer, wx, wy, target_color, options) {
                            selected_pixels.insert((wx, wy), f);
                        }
                    }
                }
            }
        }
    }

    // Apply Area Expansion (options.expand_px)
    if options.expand_px > 0 {
        let mut expanded_pixels = ahash::AHashMap::default();
        let expand = options.expand_px as i32;
        for (&(x, y), &f) in &selected_pixels {
            for dy in -expand..=expand {
                for dx in -expand..=expand {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx >= 0 && nx < canvas_width && ny >= 0 && ny < canvas_height {
                        let entry = expanded_pixels.entry((nx, ny)).or_insert(0.0f32);
                        if f > *entry {
                            *entry = f;
                        }
                    }
                }
            }
        }
        selected_pixels = expanded_pixels;
    }

    // Write to SelectionMask tiles
    if mode == SelectionMode::Intersect {
        for ((tx, ty), tile) in &mut mask.tiles {
            for ly in 0..64 {
                for lx in 0..64 {
                    let wx = tx * 64 + lx as i32;
                    let wy = ty * 64 + ly as i32;
                    let idx = ly * 64 + lx;
                    if let Some(&f) = selected_pixels.get(&(wx, wy)) {
                        let val = (255.0 * f) as u8;
                        tile[idx] = tile[idx].min(val);
                    } else {
                        tile[idx] = 0;
                    }
                }
            }
        }
    } else {
        for ((x, y), f) in selected_pixels {
            let tx = x.div_euclid(64);
            let ty = y.div_euclid(64);
            let lx = x.rem_euclid(64) as usize;
            let ly = y.rem_euclid(64) as usize;
            let tile = mask.tiles.entry((tx, ty)).or_insert_with(|| Box::new([0u8; 4096]));
            let idx = ly * 64 + lx;
            let val = (255.0 * f) as u8;
            match mode {
                SelectionMode::Replace => {
                    tile[idx] = val;
                }
                SelectionMode::Add => {
                    tile[idx] = tile[idx].saturating_add(val);
                }
                SelectionMode::Subtract => {
                    tile[idx] = tile[idx].saturating_sub(val);
                }
                SelectionMode::Intersect => unreachable!(),
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

pub fn grow_selection(mask: &mut SelectionMask, grow_px: i32, canvas_w: i32, canvas_h: i32) {
    if !mask.is_active || grow_px <= 0 { return; }
    let mut original = ahash::AHashMap::default();
    for (&(tx, ty), tile) in &mask.tiles {
        for ly in 0..64 {
            for lx in 0..64 {
                let val = tile[ly * 64 + lx];
                if val > 0 {
                    original.insert((tx * 64 + lx as i32, ty * 64 + ly as i32), val);
                }
            }
        }
    }
    
    let mut new_pixels = original.clone();
    for (&(x, y), &val) in &original {
        for dy in -grow_px..=grow_px {
            for dx in -grow_px..=grow_px {
                if dx * dx + dy * dy <= grow_px * grow_px {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx >= 0 && nx < canvas_w && ny >= 0 && ny < canvas_h {
                        let entry = new_pixels.entry((nx, ny)).or_insert(0);
                        if val > *entry {
                            *entry = val;
                        }
                    }
                }
            }
        }
    }
    
    mask.tiles.clear();
    for ((x, y), val) in new_pixels {
        if val > 0 {
            let tx = x.div_euclid(64);
            let ty = y.div_euclid(64);
            let lx = x.rem_euclid(64) as usize;
            let ly = y.rem_euclid(64) as usize;
            let tile = mask.tiles.entry((tx, ty)).or_insert_with(|| Box::new([0u8; 4096]));
            tile[ly * 64 + lx] = val;
        }
    }
}

pub fn shrink_selection(mask: &mut SelectionMask, shrink_px: i32, canvas_w: i32, canvas_h: i32) {
    if !mask.is_active || shrink_px <= 0 { return; }
    let mut original = ahash::AHashMap::default();
    for (&(tx, ty), tile) in &mask.tiles {
        for ly in 0..64 {
            for lx in 0..64 {
                let val = tile[ly * 64 + lx];
                if val > 0 {
                    original.insert((tx * 64 + lx as i32, ty * 64 + ly as i32), val);
                }
            }
        }
    }
    
    let mut new_pixels = ahash::AHashMap::default();
    for (&(x, y), &val) in &original {
        let mut min_val = val;
        for dy in -shrink_px..=shrink_px {
            for dx in -shrink_px..=shrink_px {
                if dx * dx + dy * dy <= shrink_px * shrink_px {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx < 0 || nx >= canvas_w || ny < 0 || ny >= canvas_h {
                        min_val = 0;
                        break;
                    } else {
                        let neighbor_val = *original.get(&(nx, ny)).unwrap_or(&0);
                        if neighbor_val < min_val {
                            min_val = neighbor_val;
                        }
                    }
                }
            }
            if min_val == 0 { break; }
        }
        if min_val > 0 {
            new_pixels.insert((x, y), min_val);
        }
    }
    
    mask.tiles.clear();
    for ((x, y), val) in new_pixels {
        let tx = x.div_euclid(64);
        let ty = y.div_euclid(64);
        let lx = x.rem_euclid(64) as usize;
        let ly = y.rem_euclid(64) as usize;
        let tile = mask.tiles.entry((tx, ty)).or_insert_with(|| Box::new([0u8; 4096]));
        tile[ly * 64 + lx] = val;
    }
}

pub fn feather_selection(mask: &mut SelectionMask, feather_px: i32, canvas_w: i32, canvas_h: i32) {
    if !mask.is_active || feather_px <= 0 { return; }
    let mut original = ahash::AHashMap::default();
    for (&(tx, ty), tile) in &mask.tiles {
        for ly in 0..64 {
            for lx in 0..64 {
                let val = tile[ly * 64 + lx];
                if val > 0 {
                    original.insert((tx * 64 + lx as i32, ty * 64 + ly as i32), val);
                }
            }
        }
    }
    
    let mut roi = ahash::AHashSet::default();
    for &(x, y) in original.keys() {
        for dy in -feather_px..=feather_px {
            for dx in -feather_px..=feather_px {
                if dx * dx + dy * dy <= feather_px * feather_px {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx >= 0 && nx < canvas_w && ny >= 0 && ny < canvas_h {
                        roi.insert((nx, ny));
                    }
                }
            }
        }
    }
    
    let mut new_pixels = ahash::AHashMap::default();
    for (x, y) in roi {
        let mut sum = 0u32;
        let mut count = 0u32;
        for dy in -feather_px..=feather_px {
            for dx in -feather_px..=feather_px {
                if dx * dx + dy * dy <= feather_px * feather_px {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx >= 0 && nx < canvas_w && ny >= 0 && ny < canvas_h {
                        let val = *original.get(&(nx, ny)).unwrap_or(&0);
                        sum += val as u32;
                    }
                    count += 1;
                }
            }
        }
        let avg = (sum as f32 / count as f32).round() as u8;
        if avg > 0 {
            new_pixels.insert((x, y), avg);
        }
    }
    
    mask.tiles.clear();
    for ((x, y), val) in new_pixels {
        let tx = x.div_euclid(64);
        let ty = y.div_euclid(64);
        let lx = x.rem_euclid(64) as usize;
        let ly = y.rem_euclid(64) as usize;
        let tile = mask.tiles.entry((tx, ty)).or_insert_with(|| Box::new([0u8; 4096]));
        tile[ly * 64 + lx] = val;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::SelectionMask;

    fn make_pixel_mask(center_x: i32, center_y: i32, radius: i32, canvas_w: i32, canvas_h: i32) -> SelectionMask {
        let mut mask = SelectionMask::new();
        mask.is_active = true;
        for y in 0..canvas_h {
            for x in 0..canvas_w {
                let dx = x - center_x;
                let dy = y - center_y;
                if dx * dx + dy * dy <= radius * radius {
                    let tx = x.div_euclid(64);
                    let ty = y.div_euclid(64);
                    let lx = x.rem_euclid(64) as usize;
                    let ly = y.rem_euclid(64) as usize;
                    let tile = mask.tiles.entry((tx, ty)).or_insert_with(|| Box::new([0u8; 4096]));
                    tile[ly * 64 + lx] = 255;
                }
            }
        }
        mask
    }

    fn count_selected(mask: &SelectionMask) -> i32 {
        let mut count = 0;
        for tile in mask.tiles.values() {
            for &v in tile.iter() {
                if v > 0 { count += 1; }
            }
        }
        count
    }

    #[test]
    fn test_grow_selection_expands() {
        let mut mask = make_pixel_mask(100, 100, 10, 200, 200);
        let before = count_selected(&mask);
        grow_selection(&mut mask, 3, 200, 200);
        let after = count_selected(&mask);
        assert!(after > before, "Grow should increase selected pixels ({} -> {})", before, after);
    }

    #[test]
    fn test_shrink_selection_contracts() {
        let mut mask = make_pixel_mask(100, 100, 20, 200, 200);
        let before = count_selected(&mask);
        shrink_selection(&mut mask, 3, 200, 200);
        let after = count_selected(&mask);
        assert!(after < before, "Shrink should decrease selected pixels ({} -> {})", before, after);
    }

    #[test]
    fn test_grow_then_shrink_roundtrip() {
        let mut mask = make_pixel_mask(100, 100, 10, 200, 200);
        let original = count_selected(&mask);
        grow_selection(&mut mask, 5, 200, 200);
        shrink_selection(&mut mask, 5, 200, 200);
        let after = count_selected(&mask);
        let diff = (after - original).abs();
        assert!(diff < 50, "Round-trip should approximately restore ({} -> {})", original, after);
    }

    #[test]
    fn test_inactive_mask_grow() {
        let mut mask = SelectionMask::new();
        grow_selection(&mut mask, 5, 200, 200);
        assert_eq!(count_selected(&mask), 0);
    }

    #[test]
    fn test_grow_shrink_basic() {
        let mut mask = make_pixel_mask(100, 100, 10, 200, 200);
        let before = count_selected(&mask);
        assert!(before > 0, "Initial mask should have pixels");
        grow_selection(&mut mask, 2, 200, 200);
        let grown = count_selected(&mask);
        assert!(grown > before, "Grow should expand");
    }
}
