use crate::canvas::SelectionMask;

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

    let min_x = lasso.points.iter().map(|p| p.0).min_by(|a, b| a.partial_cmp(b).unwrap()).unwrap_or(0.0) as i32 - 64;
    let min_y = lasso.points.iter().map(|p| p.1).min_by(|a, b| a.partial_cmp(b).unwrap()).unwrap_or(0.0) as i32 - 64;
    let max_x = lasso.points.iter().map(|p| p.0).max_by(|a, b| a.partial_cmp(b).unwrap()).unwrap_or(0.0) as i32 + 64;
    let max_y = lasso.points.iter().map(|p| p.1).max_by(|a, b| a.partial_cmp(b).unwrap()).unwrap_or(0.0) as i32 + 64;

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
