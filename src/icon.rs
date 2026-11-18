//! The app's tray icon: a white battery on a green tile, drawn in code so no
//! image files or decoders are needed.

const WHITE: [u8; 3] = [0xff, 0xff, 0xff];
const GREEN: [u8; 3] = [0x1e, 0x8e, 0x3e];

/// Returns `size * size` RGBA pixels.
pub fn app_icon(size: usize) -> Vec<u8> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_at_common_sizes() {
        for size in [16, 20, 24, 32, 48] {
            let px = app_icon(size);
            assert_eq!(px.len(), size * size * 4);
            // The centre is inside the tile, the very corner is transparent.
            assert_eq!(px[(size / 2 * size + size / 2) * 4 + 3], 0xff);
            assert_eq!(px[3], 0);
        }
    }
}
