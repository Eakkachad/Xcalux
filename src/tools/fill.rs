use crate::canvas::{Layer, SelectionMask, Tile};
use std::collections::VecDeque;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FillOptions {
    pub tolerance: u8,
    pub expand_px: u8,
    pub close_gap: u8,
    pub antialias: bool,
    pub sample_all_layers: bool,
    pub respect_selection: bool,
}

impl Default for FillOptions {
    fn default() -> Self {
        Self {
            tolerance: 32,
            expand_px: 1,
            close_gap: 0,
            antialias: true,
            sample_all_layers: false,
            respect_selection: true,
        }
    }
}

#[derive(Clone, Copy)]
struct FillSpan {
    y: i32,
    x_start: i32,
    x_end: i32,
}

pub fn flood_fill(
    layer: &mut Layer,
    layers: Option<&[Layer]>,
    selection: &SelectionMask,
    start_x: i32,
    start_y: i32,
    fill_color: [u16; 4], // fix15 premultiplied RGBA
    options: &FillOptions,
) -> Vec<(i32, i32)> {
    let sample_layer = if options.sample_all_layers {
        layers
    } else {
        None
    };

    let target_color = sample_pixel(sample_layer, layer, start_x, start_y);
    if color_distance(target_color, fill_color, options.tolerance) {
        return Vec::new();
    }

    if options.respect_selection && selection.is_active {
        let sel_val = selection.get_value(start_x, start_y);
        if sel_val == 0 {
            return Vec::new();
        }
    }

    let mut dirty_tiles: Vec<(i32, i32)> = Vec::new();
    let mut visited: std::collections::HashMap<(i32, i32), [u64; 64]> = std::collections::HashMap::new();
    let mut spans: VecDeque<FillSpan> = VecDeque::new();

    if !check_and_mark_visited(&mut visited, start_x, start_y) {
        return dirty_tiles;
    }

    spans.push_back(FillSpan { y: start_y, x_start: start_x, x_end: start_x });

    while let Some(span) = spans.pop_front() {
        let y = span.y;

        let mut x = span.x_start;
        while x >= span.x_start - 1 {
            if x < 0 { break; }
            if !color_match(sample_layer, layer, x, y, target_color, options) {
                break;
            }
            if options.respect_selection && selection.is_active && selection.get_value(x, y) == 0 {
                break;
            }
            if !check_and_mark_visited(&mut visited, x, y) {
                break;
            }
            x -= 1;
        }
        let left = x + 1;

        x = span.x_start.max(span.x_end);
        while x <= span.x_end + 1 {
            if !color_match(sample_layer, layer, x, y, target_color, options) {
                break;
            }
            if options.respect_selection && selection.is_active && selection.get_value(x, y) == 0 {
                break;
            }
            if !check_and_mark_visited(&mut visited, x, y) {
                break;
            }
            x += 1;
        }
        let right = x - 1;

        let tx = left.div_euclid(64);
        let ty = y.div_euclid(64);
        if !dirty_tiles.contains(&(tx, ty)) {
            dirty_tiles.push((tx, ty));
        }
        let tx2 = right.div_euclid(64);
        if tx2 != tx && !dirty_tiles.contains(&(tx2, ty)) {
            dirty_tiles.push((tx2, ty));
        }

        for fx in left..=right {
            set_pixel(layer, fx, y, fill_color, options);

            // Check row above
            if !check_and_mark_visited(&mut visited, fx, y - 1)
                && color_match(sample_layer, layer, fx, y - 1, target_color, options)
                && (!options.respect_selection || !selection.is_active || selection.get_value(fx, y - 1) > 0)
            {
                check_and_mark_visited(&mut visited, fx, y - 1);
                spans.push_back(FillSpan { y: y - 1, x_start: fx, x_end: fx });
            }

            // Check row below
            if !check_and_mark_visited(&mut visited, fx, y + 1)
                && color_match(sample_layer, layer, fx, y + 1, target_color, options)
                && (!options.respect_selection || !selection.is_active || selection.get_value(fx, y + 1) > 0)
            {
                check_and_mark_visited(&mut visited, fx, y + 1);
                spans.push_back(FillSpan { y: y + 1, x_start: fx, x_end: fx });
            }
        }
    }

    dirty_tiles
}

fn check_and_mark_visited(visited: &mut std::collections::HashMap<(i32, i32), [u64; 64]>, x: i32, y: i32) -> bool {
    let tx = x.div_euclid(64);
    let ty = y.div_euclid(64);
    let lx = x.rem_euclid(64) as usize;
    let ly = y.rem_euclid(64) as usize;

    let tile = visited.entry((tx, ty)).or_insert([0u64; 64]);
    let bit = 1u64 << lx;
    if tile[ly] & bit != 0 {
        return false;
    }
    tile[ly] |= bit;
    true
}

fn sample_pixel(sample_layers: Option<&[Layer]>, layer: &Layer, x: i32, y: i32) -> [u16; 4] {
    if let Some(layers) = sample_layers {
        let mut acc = [0u16; 4];
        let mut count = 0u32;
        for l in layers {
            if !l.visible { continue; }
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
            return [acc[0] / count as u16, acc[1] / count as u16, acc[2] / count as u16, acc[3] / count as u16];
        }
    }

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

fn set_pixel(layer: &mut Layer, x: i32, y: i32, color: [u16; 4], options: &FillOptions) {
    let tx = x.div_euclid(64);
    let ty = y.div_euclid(64);
    let lx = x.rem_euclid(64) as usize;
    let ly = y.rem_euclid(64) as usize;

    let tile = layer.tiles.entry((tx, ty)).or_insert_with(Tile::new);

    if options.expand_px > 0 {
        for dy in -(options.expand_px as i32)..=(options.expand_px as i32) {
            for dx in -(options.expand_px as i32)..=(options.expand_px as i32) {
                let nx = x + dx;
                let ny = y + dy;
                let ntx = nx.div_euclid(64);
                let nty = ny.div_euclid(64);
                let nlx = nx.rem_euclid(64) as usize;
                let nly = ny.rem_euclid(64) as usize;
                let ntile = layer.tiles.entry((ntx, nty)).or_insert_with(Tile::new);
                ntile.pixels[nly][nlx] = color;
                ntile.is_dirty = true;
            }
        }
    } else {
        tile.pixels[ly][lx] = color;
        tile.is_dirty = true;
    }
}

fn color_match(
    sample_layers: Option<&[Layer]>,
    layer: &Layer,
    x: i32,
    y: i32,
    target: [u16; 4],
    options: &FillOptions,
) -> bool {
    let pixel = sample_pixel(sample_layers, layer, x, y);
    let dr = (pixel[0] as i32 - target[0] as i32).abs();
    let dg = (pixel[1] as i32 - target[1] as i32).abs();
    let db = (pixel[2] as i32 - target[2] as i32).abs();
    let da = (pixel[3] as i32 - target[3] as i32).abs();

    let max_diff = options.tolerance as i32 * 256 / 255;
    let dist = dr + dg + db + da;
    dist <= max_diff * 4
}

fn color_distance(a: [u16; 4], b: [u16; 4], tolerance: u8) -> bool {
    let dr = (a[0] as i32 - b[0] as i32).abs();
    let dg = (a[1] as i32 - b[1] as i32).abs();
    let db = (a[2] as i32 - b[2] as i32).abs();
    let da = (a[3] as i32 - b[3] as i32).abs();
    let max_diff = tolerance as i32 * 256 / 255;
    dr + dg + db + da <= max_diff * 4
}
