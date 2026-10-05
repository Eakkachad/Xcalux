//! Manuscript page setup: trim, bleed, safe area and inner frame guides.
//!
//! Lengths are stored in document px. Millimetres exist only in the UI
//! (`px = mm / 25.4 × dpi`).

use crate::geom::RectF;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSetup {
    /// The finished (cut) page.
    pub trim: RectF,
    /// Bleed past the trim on every side.
    pub bleed: f32,
    /// Safe margin inside the trim on every side.
    pub safe: f32,
    /// Inner frame (基本枠); `w == 0` means none.
    pub inner: RectF,
    /// Display unit: 0 mm, 1 in, 2 px.
    pub unit: u8,
}

/// A New-dialog manuscript preset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MangaPreset {
    pub name: &'static str,
    /// Canvas size; `None` = trim + 2 × bleed.
    pub paper_mm: Option<(f32, f32)>,
    pub trim_mm: (f32, f32),
    pub bleed_mm: f32,
    pub safe_mm: f32,
    pub inner_mm: (f32, f32),
    pub dpi: u32,
}

// FRAMES: the presets of the spec's table.
pub const MANGA_PRESETS: &[MangaPreset] = &[];

impl PageSetup {
    /// The trim grown by the bleed, clipped to the `w`×`h` canvas.
    pub fn bleed_rect(&self, w: u32, h: u32) -> RectF {
        let canvas = RectF { x: 0.0, y: 0.0, w: w as f32, h: h as f32 };
        self.trim.expand(self.bleed).intersect(canvas).unwrap_or_default()
    }

    /// The trim shrunk by the safe margin.
    pub fn safe_rect(&self) -> RectF {
        self.trim.expand(-self.safe)
    }

    /// Canvas size and page setup of `p` at `dpi`.
    pub fn from_mm(_p: &MangaPreset, _dpi: u32) -> (u32, u32, PageSetup) {
        // FRAMES. Unreachable while MANGA_PRESETS is empty.
        let zero = RectF::default();
        (1, 1, PageSetup { trim: zero, bleed: 0.0, safe: 0.0, inner: zero, unit: 0 })
    }

    /// This setup if it is valid for a `w`×`h` canvas (finite, trim inside
    /// the canvas, …), possibly with small fixes; `None` otherwise.
    pub fn sanitized(self, _w: u32, _h: u32) -> Option<PageSetup> {
        // FRAMES
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_rects() {
        let trim = RectF { x: 100.0, y: 50.0, w: 800.0, h: 1200.0 };
        let p = PageSetup { trim, bleed: 60.0, safe: 40.0, inner: RectF::default(), unit: 0 };
        assert_eq!(p.bleed_rect(1000, 1300), RectF { x: 40.0, y: 0.0, w: 920.0, h: 1300.0 }, "clipped to the canvas");
        assert_eq!(p.safe_rect(), RectF { x: 140.0, y: 90.0, w: 720.0, h: 1120.0 });
    }
}
