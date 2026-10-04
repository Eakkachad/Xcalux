//! Stable blend-mode ids for files, plus the keys other formats use.
//!
//! `keys` matches every `BlendMode` with no wildcard, so adding a mode does
//! not compile until it has a file id here. Ids are forever: never reuse
//! or renumber one.

use arty_core::BlendMode;

/// Ids in use are `0..BLEND_ID_COUNT`.
pub const BLEND_ID_COUNT: u8 = 14;

/// `(file id, Photoshop blend key)` of a mode. The PSD keys are for the
/// planned PSD export.
const fn keys(m: BlendMode) -> (u8, [u8; 4]) {
    match m {
        BlendMode::Normal => (0, *b"norm"),
        BlendMode::Multiply => (1, *b"mul "),
        BlendMode::Screen => (2, *b"scrn"),
        BlendMode::Overlay => (3, *b"over"),
        BlendMode::Darken => (4, *b"dark"),
        BlendMode::Lighten => (5, *b"lite"),
        BlendMode::ColorDodge => (6, *b"div "),
        BlendMode::ColorBurn => (7, *b"idiv"),
        BlendMode::LinearBurn => (8, *b"lbrn"),
        BlendMode::Add => (9, *b"lddg"),
        BlendMode::SoftLight => (10, *b"sLit"),
        BlendMode::HardLight => (11, *b"hLit"),
        BlendMode::Difference => (12, *b"diff"),
        BlendMode::PassThrough => (13, *b"pass"),
    }
}

/// The id a mode is stored as.
pub const fn blend_id(m: BlendMode) -> u8 {
    keys(m).0
}

/// The mode stored as `id`; `None` for ids this version does not know
/// (the reader loads those as Normal and warns).
pub const fn blend_from_id(id: u8) -> Option<BlendMode> {
    Some(match id {
        0 => BlendMode::Normal,
        1 => BlendMode::Multiply,
        2 => BlendMode::Screen,
        3 => BlendMode::Overlay,
        4 => BlendMode::Darken,
        5 => BlendMode::Lighten,
        6 => BlendMode::ColorDodge,
        7 => BlendMode::ColorBurn,
        8 => BlendMode::LinearBurn,
        9 => BlendMode::Add,
        10 => BlendMode::SoftLight,
        11 => BlendMode::HardLight,
        12 => BlendMode::Difference,
        13 => BlendMode::PassThrough,
        _ => return None,
    })
}

/// Photoshop's 4-byte key for a mode.
pub const fn psd_key(m: BlendMode) -> [u8; 4] {
    keys(m).1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_modes() -> Vec<BlendMode> {
        let mut v = BlendMode::LAYER_MODES.to_vec();
        v.push(BlendMode::PassThrough);
        v
    }

    #[test]
    fn blend_table_is_a_bijection() {
        let modes = all_modes();
        assert_eq!(modes.len(), BLEND_ID_COUNT as usize);
        let mut ids: Vec<u8> = modes.iter().map(|&m| blend_id(m)).collect();
        for &m in &modes {
            assert_eq!(blend_from_id(blend_id(m)), Some(m));
        }
        ids.sort();
        assert_eq!(ids, (0..BLEND_ID_COUNT).collect::<Vec<_>>());
        let mut psd: Vec<[u8; 4]> = modes.iter().map(|&m| psd_key(m)).collect();
        psd.sort();
        psd.dedup();
        assert_eq!(psd.len(), modes.len(), "PSD keys are distinct");
    }

    #[test]
    fn every_id_decodes_without_panic() {
        for id in 0..=255u8 {
            match blend_from_id(id) {
                Some(m) => assert_eq!(blend_id(m), id),
                None => assert!(id >= BLEND_ID_COUNT),
            }
        }
    }

    #[test]
    fn ids_are_stable() {
        // Pinned: these numbers are in every saved file.
        assert_eq!(blend_id(BlendMode::Normal), 0);
        assert_eq!(blend_id(BlendMode::Add), 9);
        assert_eq!(blend_id(BlendMode::PassThrough), 13);
    }
}
