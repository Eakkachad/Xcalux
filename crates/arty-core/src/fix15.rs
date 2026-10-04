//! libmypaint-compatible 15-bit fixed point (`1.0 == 1 << 15`).

pub const ONE: u32 = 1 << 15;
pub const ONE_U16: u16 = 1 << 15;
const INV_ONE: f32 = 1.0 / ONE as f32;

#[inline(always)]
pub fn mul(a: u32, b: u32) -> u32 {
    (a * b) >> 15
}

#[inline(always)]
pub fn to_f32(v: u16) -> f32 {
    v as f32 * INV_ONE
}

#[inline(always)]
pub fn from_f32(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * ONE as f32 + 0.5) as u16
}

/// fix15 → 8-bit with rounding.
#[inline(always)]
pub fn to_u8(v: u16) -> u8 {
    ((v as u32 * 255 + (ONE >> 1)) >> 15).min(255) as u8
}

/// 8-bit → fix15 with rounding.
#[inline(always)]
pub fn from_u8(v: u8) -> u16 {
    ((v as u32 * ONE + 127) / 255) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u8_round_trip_is_exact() {
        for v in 0..=255u8 {
            assert_eq!(to_u8(from_u8(v)), v);
        }
    }

    #[test]
    fn endpoints() {
        assert_eq!(to_u8(ONE_U16), 255);
        assert_eq!(from_f32(1.0), ONE_U16);
        assert_eq!(from_f32(-3.0), 0);
    }
}
