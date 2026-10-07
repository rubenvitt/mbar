//! Colors with SketchyBar semantics: stored as clamped f32 channels plus the derived
//! `0xAARRGGBB` hex value (recomputed by truncation, exactly like `color_update_hex`).

use crate::value;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
    pub hex: u32,
}

impl Default for Color {
    fn default() -> Self {
        Color::TRANSPARENT
    }
}

impl Color {
    pub const TRANSPARENT: Color = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
        hex: 0,
    };

    pub fn from_hex(hex: u32) -> Self {
        let mut c = Color::TRANSPARENT;
        c.set_hex(hex);
        c
    }

    /// Parses a property value like `0xff00ff00` (`strtoul` base 0 semantics).
    pub fn parse(s: &str) -> Self {
        Color::from_hex(value::parse_u32(s))
    }

    fn update_hex(&mut self) -> bool {
        let prev = self.hex;
        self.hex = (((self.a * 255.0) as u32) << 24)
            .wrapping_add(((self.r * 255.0) as u32) << 16)
            .wrapping_add(((self.g * 255.0) as u32) << 8)
            .wrapping_add((self.b * 255.0) as u32);
        prev != self.hex
    }

    /// Returns true if the color changed.
    pub fn set_hex(&mut self, hex: u32) -> bool {
        self.a = ((hex >> 24) & 0xff) as f32 / 255.0;
        self.r = ((hex >> 16) & 0xff) as f32 / 255.0;
        self.g = ((hex >> 8) & 0xff) as f32 / 255.0;
        self.b = (hex & 0xff) as f32 / 255.0;
        self.update_hex()
    }

    pub fn set_alpha(&mut self, v: f32) -> bool {
        self.a = v.clamp(0.0, 1.0);
        self.update_hex()
    }
    pub fn set_red(&mut self, v: f32) -> bool {
        self.r = v.clamp(0.0, 1.0);
        self.update_hex()
    }
    pub fn set_green(&mut self, v: f32) -> bool {
        self.g = v.clamp(0.0, 1.0);
        self.update_hex()
    }
    pub fn set_blue(&mut self, v: f32) -> bool {
        self.b = v.clamp(0.0, 1.0);
        self.update_hex()
    }

    pub fn is_transparent(&self) -> bool {
        self.a <= 0.0
    }

    /// `0xAARRGGBB` as SketchyBar prints it in queries.
    pub fn to_hex_string(&self) -> String {
        format!("0x{:08x}", self.hex)
    }

    /// Premultiplied RGBA for GPU upload.
    pub fn premultiplied(&self) -> [f32; 4] {
        [self.r * self.a, self.g * self.a, self.b * self.a, self.a]
    }

    /// Byte-wise interpolation as used by `ANIMATE_BYTES` for hex color animation:
    /// every channel byte is interpolated independently.
    pub fn lerp_bytes(from: u32, to: u32, t: f64) -> u32 {
        let mut out = 0u32;
        for shift in [24u32, 16, 8, 0] {
            let a = ((from >> shift) & 0xff) as f64;
            let b = ((to >> shift) & 0xff) as f64;
            let v = (a + (b - a) * t).round().clamp(0.0, 255.0) as u32;
            out |= v << shift;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_hex() {
        for hex in [0u32, 0xffffffff, 0x40000000, 0xff123456, 0x80ff00ff] {
            assert_eq!(Color::from_hex(hex).hex, hex);
        }
        assert_eq!(Color::parse("0x40ffffff").to_hex_string(), "0x40ffffff");
    }

    #[test]
    fn alpha_changes_hex() {
        let mut c = Color::from_hex(0xffffffff);
        assert!(c.set_alpha(0.5));
        assert_eq!(c.hex >> 24, 127);
    }

    #[test]
    fn byte_lerp() {
        assert_eq!(Color::lerp_bytes(0x00000000, 0xff0000ff, 0.5), 0x80000080);
        assert_eq!(Color::lerp_bytes(0x11223344, 0x55667788, 1.0), 0x55667788);
    }
}
