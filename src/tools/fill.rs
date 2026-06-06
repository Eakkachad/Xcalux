use crate::canvas::{Layer, SelectionMask};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillDetectionMode {
    TransparencyStrict,
    TransparencyFuzzy,
    ColorDifference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillReference {
    CurrentLayer,
    SelectionSourceLayers,
    AllVisibleLayers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillWith {
    ForegroundColor,
    #[allow(dead_code)]
    Transparent,
}

#[derive(Debug, Clone)]
pub struct FillOptions {
    pub tolerance: u8,
    pub transp_diff: u8,
    pub expand_px: u8,
    #[allow(dead_code)]
    pub close_gap: u8,
    pub antialias: bool,
    pub respect_selection: bool,
    pub fill_transparent_only: bool,
    #[allow(dead_code)]
    pub treat_alpha_as_boundary: bool,
    pub detection_mode: FillDetectionMode,
    pub reference: FillReference,
    #[allow(dead_code)]
    pub fill_with: FillWith,
    pub contiguous: bool,
}

impl Default for FillOptions {
    fn default() -> Self {
        Self {
            tolerance: 32,
            transp_diff: 32,
            expand_px: 1,
            close_gap: 0,
            antialias: true,
            respect_selection: true,
            fill_transparent_only: false,
            treat_alpha_as_boundary: true,
            detection_mode: FillDetectionMode::ColorDifference,
            reference: FillReference::CurrentLayer,
            fill_with: FillWith::ForegroundColor,
            contiguous: true,
        }
    }
}

pub fn blend_colors(src: [u16; 4], dst: [u16; 4]) -> [u16; 4] {
    let src_a = src[3] as f32 / 32768.0;
    let dst_a = dst[3] as f32 / 32768.0;

    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0 {
        return [0, 0, 0, 0];
    }

    let out_r = (src[0] as f32 * src_a + dst[0] as f32 * dst_a * (1.0 - src_a)) / out_a;
    let out_g = (src[1] as f32 * src_a + dst[1] as f32 * dst_a * (1.0 - src_a)) / out_a;
    let out_b = (src[2] as f32 * src_a + dst[2] as f32 * dst_a * (1.0 - src_a)) / out_a;

    [
        out_r.clamp(0.0, 32768.0) as u16,
        out_g.clamp(0.0, 32768.0) as u16,
        out_b.clamp(0.0, 32768.0) as u16,
        (out_a * 32768.0).clamp(0.0, 32768.0) as u16,
    ]
}

pub fn sample_reference(
    layers: &[&Layer],
    layer: &Layer,
    reference: FillReference,
    x: i32,
    y: i32,
) -> [u16; 4] {
    match reference {
        FillReference::CurrentLayer => sample_pixel(layer, x, y),
        FillReference::SelectionSourceLayers => {
            let mut acc = [0u16; 4];
            let mut count = 0u32;
            for l in layers {
                if !l.visible || !l.selection_source {
                    continue;
                }
                let tx = x.div_euclid(64);
                let ty = y.div_euclid(64);
                let lx = x.rem_euclid(64) as usize;
                let ly = y.rem_euclid(64) as usize;
                if let Some(tile) = l.tiles.get(&(tx, ty)) {
                    let p = tile.pixels[ly][lx];
                    acc[0] = acc[0].saturating_add(p[0]);
                    acc[1] = acc[1].saturating_add(p[1]);
                    acc[2] = acc[2].saturating_add(p[2]);
                    acc[3] = acc[3].saturating_add(p[3]);
                    count += 1;
                }
            }
            if count > 0 {
                return [
                    acc[0] / count as u16,
                    acc[1] / count as u16,
                    acc[2] / count as u16,
                    acc[3] / count as u16,
                ];
            }
            sample_pixel(layer, x, y)
        }
        FillReference::AllVisibleLayers => {
            let mut acc = [0u16; 4];
            let mut count = 0u32;
            for l in layers {
                if !l.visible {
                    continue;
                }
                let tx = x.div_euclid(64);
                let ty = y.div_euclid(64);
                let lx = x.rem_euclid(64) as usize;
                let ly = y.rem_euclid(64) as usize;
                if let Some(tile) = l.tiles.get(&(tx, ty)) {
                    let p = tile.pixels[ly][lx];
                    acc[0] = acc[0].saturating_add(p[0]);
                    acc[1] = acc[1].saturating_add(p[1]);
                    acc[2] = acc[2].saturating_add(p[2]);
                    acc[3] = acc[3].saturating_add(p[3]);
                    count += 1;
                }
            }
            if count > 0 {
                return [
                    acc[0] / count as u16,
                    acc[1] / count as u16,
                    acc[2] / count as u16,
                    acc[3] / count as u16,
                ];
            }
            sample_pixel(layer, x, y)
        }
    }
}

fn sample_pixel(layer: &Layer, x: i32, y: i32) -> [u16; 4] {
    let tx = x.div_euclid(64);
    let ty = y.div_euclid(64);
    let lx = x.rem_euclid(64) as usize;
    let ly = y.rem_euclid(64) as usize;
    if let Some(tile) = layer.tiles.get(&(tx, ty)) {
        tile.pixels[ly][lx]
    } else {
        [0, 0, 0, 0]
    }
}

pub fn is_fillable(
    layers: &[&Layer],
    layer: &Layer,
    x: i32,
    y: i32,
    seed: [u16; 4],
    options: &FillOptions,
) -> Option<f32> {
    let pixel = sample_reference(layers, layer, options.reference, x, y);

    match options.detection_mode {
        FillDetectionMode::TransparencyStrict => {
            if pixel[3] == 0 {
                Some(1.0)
            } else {
                None
            }
        }
        FillDetectionMode::TransparencyFuzzy => {
            let limit = ((options.transp_diff as u32 * 32768) / 255) as u16;
            if pixel[3] <= limit {
                if options.antialias && limit > 0 {
                    let limit_core = (limit as f32 * 0.8) as u16;
                    if pixel[3] <= limit_core {
                        Some(1.0)
                    } else {
                        let factor = (limit - pixel[3]) as f32 / (limit - limit_core) as f32;
                        Some(factor.clamp(0.0, 1.0))
                    }
                } else {
                    Some(1.0)
                }
            } else {
                None
            }
        }
        FillDetectionMode::ColorDifference => {
            let dr = (pixel[0] as i32 - seed[0] as i32).abs();
            let dg = (pixel[1] as i32 - seed[1] as i32).abs();
            let db = (pixel[2] as i32 - seed[2] as i32).abs();
            let da = (pixel[3] as i32 - seed[3] as i32).abs();
            let max_diff = (options.tolerance as i32 * 32768) / 255;
            let dist = dr + dg + db + da;
            let limit = max_diff * 4;
            if dist <= limit {
                if options.antialias && limit > 0 {
                    let limit_core = (limit as f32 * 0.8) as i32;
                    if dist <= limit_core {
                        Some(1.0)
                    } else {
                        let factor = (limit - dist) as f32 / (limit - limit_core) as f32;
                        Some(factor.clamp(0.0, 1.0))
                    }
                } else {
                    Some(1.0)
                }
            } else {
                None
            }
        }
    }
}

fn is_seed_same_as_fill(target: [u16; 4], fill: [u16; 4], options: &FillOptions) -> bool {
    match options.detection_mode {
        FillDetectionMode::TransparencyStrict | FillDetectionMode::TransparencyFuzzy => {
            target[3] >= 32768
        }
        FillDetectionMode::ColorDifference => {
            let dr = (target[0] as i32 - fill[0] as i32).abs();
            let dg = (target[1] as i32 - fill[1] as i32).abs();
            let db = (target[2] as i32 - fill[2] as i32).abs();
            let da = (target[3] as i32 - fill[3] as i32).abs();
            let max_diff = (options.tolerance as i32 * 32768) / 255;
            let dist = dr + dg + db + da;
            dist <= max_diff * 4
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn flood_fill(
    layer: &mut Layer,
    all_layers: &[&Layer],
    selection: &SelectionMask,
    start_x: i32,
    start_y: i32,
    fill_color: [u16; 4],
    options: &FillOptions,
    canvas_width: i32,
    canvas_height: i32,
) -> Vec<(i32, i32)> {
    if start_x < 0 || start_x >= canvas_width || start_y < 0 || start_y >= canvas_height {
        return Vec::new();
    }

    let target_color = sample_reference(all_layers, layer, options.reference, start_x, start_y);
    if is_seed_same_as_fill(target_color, fill_color, options) {
        return Vec::new();
    }

    if options.respect_selection && selection.is_active {
        let sel_val = selection.get_value(start_x, start_y);
        if sel_val == 0 {
            return Vec::new();
        }
    }

    let mut visited: std::collections::HashMap<(i32, i32), [u64; 64]> =
        std::collections::HashMap::new();
    let mut queue: VecDeque<(i32, i32)> = VecDeque::new();
    let mut pixels_to_fill: Vec<((i32, i32), f32)> = Vec::new();

    let is_visited =
        |visited: &std::collections::HashMap<(i32, i32), [u64; 64]>, x: i32, y: i32| -> bool {
            let tx = x.div_euclid(64);
            let ty = y.div_euclid(64);
            let lx = x.rem_euclid(64) as usize;
            let ly = y.rem_euclid(64) as usize;
            if let Some(tile) = visited.get(&(tx, ty)) {
                (tile[ly] & (1u64 << lx)) != 0
            } else {
                false
            }
        };

    let mark_visited =
        |visited: &mut std::collections::HashMap<(i32, i32), [u64; 64]>, x: i32, y: i32| {
            let tx = x.div_euclid(64);
            let ty = y.div_euclid(64);
            let lx = x.rem_euclid(64) as usize;
            let ly = y.rem_euclid(64) as usize;
            let tile = visited.entry((tx, ty)).or_insert([0u64; 64]);
            tile[ly] |= 1u64 << lx;
        };

    queue.push_back((start_x, start_y));

    while let Some((cx, cy)) = queue.pop_front() {
        if is_visited(&visited, cx, cy) {
            continue;
        }
        let factor = match is_fillable(all_layers, layer, cx, cy, target_color, options) {
            Some(f) => f,
            None => continue,
        };
        if options.respect_selection && selection.is_active && selection.get_value(cx, cy) == 0 {
            continue;
        }

        // Find left limit
        let mut left = cx;
        let mut left_factor = factor;
        while left > 0 {
            let nx = left - 1;
            if is_visited(&visited, nx, cy) {
                break;
            }
            let f = match is_fillable(all_layers, layer, nx, cy, target_color, options) {
                Some(val) => val,
                None => break,
            };
            if options.respect_selection && selection.is_active && selection.get_value(nx, cy) == 0
            {
                break;
            }
            left = nx;
            left_factor = f;
        }

        // Find right limit
        let mut right = cx;
        let mut right_factor = factor;
        while right < canvas_width - 1 {
            let nx = right + 1;
            if is_visited(&visited, nx, cy) {
                break;
            }
            let f = match is_fillable(all_layers, layer, nx, cy, target_color, options) {
                Some(val) => val,
                None => break,
            };
            if options.respect_selection && selection.is_active && selection.get_value(nx, cy) == 0
            {
                break;
            }
            right = nx;
            right_factor = f;
        }

        // Mark visited and collect coordinates
        for x in left..=right {
            let f = if x == left {
                left_factor
            } else if x == right {
                right_factor
            } else {
                is_fillable(all_layers, layer, x, cy, target_color, options).unwrap_or(1.0)
            };
            pixels_to_fill.push(((x, cy), f));
            mark_visited(&mut visited, x, cy);
        }

        // Scan row above
        if cy > 0 {
            let ny = cy - 1;
            let mut in_span = false;
            for x in left..=right {
                let fillable = is_fillable(all_layers, layer, x, ny, target_color, options)
                    .is_some()
                    && (!options.respect_selection
                        || !selection.is_active
                        || selection.get_value(x, ny) > 0)
                    && !is_visited(&visited, x, ny);
                if fillable {
                    if !in_span {
                        queue.push_back((x, ny));
                        in_span = true;
                    }
                } else {
                    in_span = false;
                }
            }
        }

        // Scan row below
        if cy < canvas_height - 1 {
            let ny = cy + 1;
            let mut in_span = false;
            for x in left..=right {
                let fillable = is_fillable(all_layers, layer, x, ny, target_color, options)
                    .is_some()
                    && (!options.respect_selection
                        || !selection.is_active
                        || selection.get_value(x, ny) > 0)
                    && !is_visited(&visited, x, ny);
                if fillable {
                    if !in_span {
                        queue.push_back((x, ny));
                        in_span = true;
                    }
                } else {
                    in_span = false;
                }
            }
        }
    }

    // Now write to the layer using the collected coordinates
    let mut dirty_tiles: Vec<(i32, i32)> = Vec::new();
    for ((x, y), f) in pixels_to_fill {
        let mut color = fill_color;
        color[3] = (fill_color[3] as f32 * f) as u16;
        set_pixel(layer, x, y, color, options, canvas_width, canvas_height);

        if options.expand_px > 0 {
            for dy in -(options.expand_px as i32)..=options.expand_px as i32 {
                for dx in -(options.expand_px as i32)..=options.expand_px as i32 {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx >= 0 && nx < canvas_width && ny >= 0 && ny < canvas_height {
                        let tx = nx.div_euclid(64);
                        let ty = ny.div_euclid(64);
                        if !dirty_tiles.contains(&(tx, ty)) {
                            dirty_tiles.push((tx, ty));
                        }
                    }
                }
            }
        } else {
            let tx = x.div_euclid(64);
            let ty = y.div_euclid(64);
            if !dirty_tiles.contains(&(tx, ty)) {
                dirty_tiles.push((tx, ty));
            }
        }
    }

    dirty_tiles
}

fn set_pixel(
    layer: &mut Layer,
    x: i32,
    y: i32,
    color: [u16; 4],
    options: &FillOptions,
    canvas_width: i32,
    canvas_height: i32,
) {
    if x < 0 || x >= canvas_width || y < 0 || y >= canvas_height {
        return;
    }

    let tx = x.div_euclid(64);
    let ty = y.div_euclid(64);
    let lx = x.rem_euclid(64) as usize;
    let ly = y.rem_euclid(64) as usize;

    let tile = layer.tiles.entry((tx, ty)).or_default();

    if options.fill_transparent_only {
        let old = tile.pixels[ly][lx];
        if old[3] > 0 {
            return;
        }
    }

    if options.expand_px > 0 {
        for dy in -(options.expand_px as i32)..=(options.expand_px as i32) {
            for dx in -(options.expand_px as i32)..=(options.expand_px as i32) {
                let nx = x + dx;
                let ny = y + dy;
                if nx >= 0 && nx < canvas_width && ny >= 0 && ny < canvas_height {
                    let ntx = nx.div_euclid(64);
                    let nty = ny.div_euclid(64);
                    let nlx = nx.rem_euclid(64) as usize;
                    let nly = ny.rem_euclid(64) as usize;
                    let ntile = layer.tiles.entry((ntx, nty)).or_default();
                    ntile.pixels[nly][nlx] = blend_colors(color, ntile.pixels[nly][nlx]);
                    ntile.is_dirty = true;
                }
            }
        }
    } else {
        tile.pixels[ly][lx] = blend_colors(color, tile.pixels[ly][lx]);
        tile.is_dirty = true;
    }
}
