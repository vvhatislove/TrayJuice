//! Renders the tray icon: a coloured tile with the battery percentage on it.
//! Uses a built-in 5x7 bitmap font so no font files or text stack are needed.

use crate::device::BatteryState;

const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;

/// Rows top to bottom, 5 low bits per row, most significant bit on the left.
const DIGITS: [[u8; GLYPH_H]; 10] = [
    [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e], // 0
    [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e], // 1
    [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f], // 2
    [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e], // 3
    [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02], // 4
    [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e], // 5
    [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e], // 6
    [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08], // 7
    [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e], // 8
    [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c], // 9
];
const DASH: [u8; GLYPH_H] = [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00];
/// A plain vertical bar: the "1" of "100", which has to be squeezed in.
const BAR: [u8; GLYPH_H] = [0x1f; GLYPH_H];

const WHITE: [u8; 3] = [0xff, 0xff, 0xff];
const GREEN: [u8; 3] = [0x1e, 0x8e, 0x3e];
const AMBER: [u8; 3] = [0xc7, 0x7c, 0x02];
const RED: [u8; 3] = [0xd9, 0x30, 0x25];
const BLUE: [u8; 3] = [0x1a, 0x73, 0xe8];
const GREY: [u8; 3] = [0x5f, 0x63, 0x68];

/// Background colour for a state: blue while charging, otherwise by level.
fn background(state: &BatteryState) -> [u8; 3] {
    match *state {
        BatteryState::Offline => GREY,
        BatteryState::Level { charging: true, .. } => BLUE,
        BatteryState::Level { percent, .. } if percent > 50 => GREEN,
        BatteryState::Level { percent, .. } if percent > 20 => AMBER,
        BatteryState::Level { .. } => RED,
    }
}

/// Glyphs to draw, each with a relative width.
fn glyphs(state: &BatteryState) -> Vec<(&'static [u8; GLYPH_H], usize)> {
    match *state {
        BatteryState::Offline => vec![(&DASH, 5), (&DASH, 5)],
        BatteryState::Level { percent, .. } if percent >= 100 => {
            vec![(&BAR, 2), (&DIGITS[0], 5), (&DIGITS[0], 5)]
        }
        BatteryState::Level { percent, .. } => percent
            .to_string()
            .bytes()
            .map(|b| (&DIGITS[(b - b'0') as usize], 5))
            .collect(),
    }
}

/// Returns `size * size` RGBA pixels.
pub fn render(state: &BatteryState, size: usize) -> Vec<u8> {
    let mut px = vec![0u8; size * size * 4];
    let bg = background(state);
    let radius = (size / 8).max(1);

    for y in 0..size {
        for x in 0..size {
            if in_rounded_square(x, y, size, radius) {
                put(&mut px, size, x, y, bg);
            }
        }
    }

    let glyphs = glyphs(state);
    let n = glyphs.len();
    let margin = (size / 16).max(1);
    let gap = if n >= 3 { margin } else { (size / 8).max(1) };
    let glyph_h = size - 4 * margin;
    let avail = size - 2 * margin - gap * (n - 1);
    let weight_sum: usize = glyphs.iter().map(|g| g.1).sum();
    // A lone digit must not stretch across the whole tile.
    let max_w = glyph_h * GLYPH_W / GLYPH_H + 1;
    let widths: Vec<usize> = glyphs
        .iter()
        .map(|g| (avail * g.1 / weight_sum).min(max_w))
        .collect();
    let total_w = widths.iter().sum::<usize>() + gap * (n - 1);
    let top = (size - glyph_h) / 2;

    let mut x0 = (size - total_w) / 2;
    for ((glyph, _), &glyph_w) in glyphs.iter().zip(&widths) {
        for dy in 0..glyph_h {
            let row = glyph[dy * GLYPH_H / glyph_h];
            for dx in 0..glyph_w {
                let col = dx * GLYPH_W / glyph_w;
                if row & (0x10 >> col) != 0 {
                    put(&mut px, size, x0 + dx, top + dy, WHITE);
                }
            }
        }
        x0 += glyph_w + gap;
    }
    px
}

fn put(px: &mut [u8], size: usize, x: usize, y: usize, rgb: [u8; 3]) {
    let i = (y * size + x) * 4;
    px[i..i + 3].copy_from_slice(&rgb);
    px[i + 3] = 0xff;
}

fn in_rounded_square(x: usize, y: usize, size: usize, r: usize) -> bool {
    let edge = size - 1;
    let cx = if x < r { r } else if x > edge - r { edge - r } else { return true };
    let cy = if y < r { r } else if y > edge - r { edge - r } else { return true };
    let (dx, dy) = (x.abs_diff(cx), y.abs_diff(cy));
    dx * dx + dy * dy <= r * r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_level_at_common_sizes() {
        for size in [16, 20, 24, 32, 48] {
            for percent in 0..=100 {
                let state = BatteryState::Level { percent, charging: false };
                assert_eq!(render(&state, size).len(), size * size * 4);
            }
            render(&BatteryState::Offline, size);
        }
    }
}
