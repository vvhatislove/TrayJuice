//! The app's icon: a white battery on a green tile, drawn in code so no
//! image decoders are needed. The same drawing is the tray icon and, through
//! `assets/trayjuice.ico` (written by the `make_icon` example), the icon of
//! the executable and of the installer.
//!
//! Small sizes are drawn on a 16x16 grid so that every line falls on whole
//! pixels. From 24 pixels up the shapes are smooth, with rounded corners and
//! softened edges.

const WHITE: [u8; 3] = [0xff, 0xff, 0xff];
const GREEN: [u8; 3] = [0x1e, 0x8e, 0x3e];
/// Top of the tile in the smooth drawing; it darkens to [`GREEN`] downwards.
const GREEN_TOP: [u8; 3] = [0x34, 0xc7, 0x59];

/// Below this size the grid drawing is used.
const SMOOTH_FROM: usize = 24;
/// Samples per pixel along each axis for the smooth drawing.
const SAMPLES: usize = 4;

/// Returns `size * size` RGBA pixels (not premultiplied).
pub fn app_icon(size: usize) -> Vec<u8> {
    if size < SMOOTH_FROM {
        grid_icon(size)
    } else {
        smooth_icon(size)
    }
}

fn grid_icon(size: usize) -> Vec<u8> {
    let mut px = vec![0u8; size * size * 4];
    let radius = (size / 8).max(1);
    // Battery drawn on a 16x16 grid: body outline, charge fill, terminal nub.
    let on_battery = |gx: usize, gy: usize| {
        let body = (2..=12).contains(&gx) && (5..=10).contains(&gy);
        let hollow = (3..=11).contains(&gx) && (6..=9).contains(&gy);
        let fill = (4..=8).contains(&gx) && (7..=8).contains(&gy);
        let nub = gx == 13 && (7..=8).contains(&gy);
        (body && !hollow) || fill || nub
    };
    for y in 0..size {
        for x in 0..size {
            if !in_rounded_square(x, y, size, radius) {
                continue;
            }
            let rgb = if on_battery(x * 16 / size, y * 16 / size) {
                WHITE
            } else {
                GREEN
            };
            let i = (y * size + x) * 4;
            px[i..i + 3].copy_from_slice(&rgb);
            px[i + 3] = 0xff;
        }
    }
    px
}

fn in_rounded_square(x: usize, y: usize, size: usize, r: usize) -> bool {
    let edge = size - 1;
    let cx = if x < r {
        r
    } else if x > edge - r {
        edge - r
    } else {
        return true;
    };
    let cy = if y < r {
        r
    } else if y > edge - r {
        edge - r
    } else {
        return true;
    };
    let (dx, dy) = (x.abs_diff(cx), y.abs_diff(cy));
    dx * dx + dy * dy <= r * r
}

/// A rectangle with rounded corners, in units of the icon's side.
struct Shape {
    centre: (f32, f32),
    half: (f32, f32),
    radius: f32,
}

impl Shape {
    /// Distance from the edge: negative inside.
    fn distance(&self, x: f32, y: f32) -> f32 {
        let qx = (x - self.centre.0).abs() - (self.half.0 - self.radius);
        let qy = (y - self.centre.1).abs() - (self.half.1 - self.radius);
        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
        outside + qx.max(qy).min(0.0) - self.radius
    }
}

const TILE: Shape = Shape {
    centre: (0.5, 0.5),
    half: (0.5, 0.5),
    radius: 0.22,
};
const BODY: Shape = Shape {
    centre: (0.46, 0.5),
    half: (0.33, 0.2),
    radius: 0.08,
};
/// Thickness of the body's outline.
const OUTLINE: f32 = 0.06;
/// The charge inside the body: about two thirds full.
const CHARGE: Shape = Shape {
    centre: (0.37, 0.5),
    half: (0.145, 0.105),
    radius: 0.03,
};
const NUB: Shape = Shape {
    centre: (0.825, 0.5),
    half: (0.035, 0.085),
    radius: 0.025,
};

/// What is at a point of the unit square: nothing, or a colour.
fn smooth_at(x: f32, y: f32) -> Option<[u8; 3]> {
    if TILE.distance(x, y) > 0.0 {
        return None;
    }
    let body = BODY.distance(x, y);
    let white = (-OUTLINE..=0.0).contains(&body)
        || CHARGE.distance(x, y) <= 0.0
        || NUB.distance(x, y) <= 0.0;
    if white {
        return Some(WHITE);
    }
    let mix = |top: u8, bottom: u8| (top as f32 + (bottom as f32 - top as f32) * y) as u8;
    Some([
        mix(GREEN_TOP[0], GREEN[0]),
        mix(GREEN_TOP[1], GREEN[1]),
        mix(GREEN_TOP[2], GREEN[2]),
    ])
}

fn smooth_icon(size: usize) -> Vec<u8> {
    let mut px = vec![0u8; size * size * 4];
    let step = 1.0 / (size * SAMPLES) as f32;
    for y in 0..size {
        for x in 0..size {
            // Average of the samples that hit something; the share of them
            // that did is the pixel's opacity.
            let (mut sum, mut hits) = ([0u32; 3], 0u32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let at = |pixel: usize, sample: usize| {
                        ((pixel * SAMPLES + sample) as f32 + 0.5) * step
                    };
                    if let Some(rgb) = smooth_at(at(x, sx), at(y, sy)) {
                        hits += 1;
                        for (total, part) in sum.iter_mut().zip(rgb) {
                            *total += part as u32;
                        }
                    }
                }
            }
            if hits == 0 {
                continue;
            }
            let i = (y * size + x) * 4;
            for (out, total) in px[i..i + 3].iter_mut().zip(sum) {
                *out = (total / hits) as u8;
            }
            px[i + 3] = (hits * 255 / (SAMPLES * SAMPLES) as u32) as u8;
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_at_common_sizes() {
        for size in [16, 20, 24, 32, 48, 256] {
            let px = app_icon(size);
            assert_eq!(px.len(), size * size * 4);
            // The centre is inside the tile, the very corner is transparent.
            assert_eq!(px[(size / 2 * size + size / 2) * 4 + 3], 0xff);
            assert_eq!(px[3], 0);
        }
    }

    #[test]
    fn the_smooth_drawing_has_a_battery_on_green() {
        let size = 64;
        let px = app_icon(size);
        let at = |x: usize, y: usize| &px[(y * size + x) * 4..(y * size + x) * 4 + 4];
        // Inside the charge: white.
        assert_eq!(at(22, 32), [0xff, 0xff, 0xff, 0xff]);
        // Between the charge and the right end of the body: green.
        let gap = at(42, 32);
        assert!(gap[1] > gap[0] && gap[1] > gap[2] && gap[3] == 0xff);
        // The terminal nub: white.
        assert_eq!(at(51, 32), [0xff, 0xff, 0xff, 0xff]);
        // Above the battery the tile is lighter than below it.
        assert!(at(32, 6)[1] > at(32, 58)[1]);
    }

    #[test]
    fn rounded_shapes_measure_from_their_edge() {
        let square = Shape {
            centre: (0.5, 0.5),
            half: (0.25, 0.25),
            radius: 0.05,
        };
        assert!(square.distance(0.5, 0.5) < 0.0);
        assert!(square.distance(0.74, 0.5) < 0.0);
        assert!(square.distance(0.76, 0.5) > 0.0);
        // The sharp corner of the bounding box is cut off.
        assert!(square.distance(0.745, 0.745) > 0.0);
    }
}
