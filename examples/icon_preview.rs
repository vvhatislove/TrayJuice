//! Writes `icon_preview.bmp`: tray icons for several states, enlarged, so the
//! rendering can be checked by eye. Run with `cargo run --example icon_preview`.

use trayjuice::device::BatteryState;
use trayjuice::icon::render;

const ZOOM: usize = 6;
const PAD: usize = 4;

fn main() {
    let states: Vec<BatteryState> = [100, 87, 64, 35, 16, 9, 0]
        .into_iter()
        .map(|percent| BatteryState::Level { percent, charging: false })
        .chain([
            BatteryState::Level { percent: 42, charging: true },
            BatteryState::Offline,
        ])
        .collect();
    let sizes = [16usize, 24, 32];

    let cell = 32 * ZOOM + PAD;
    let (w, h) = (states.len() * cell + PAD, sizes.len() * cell + PAD);
    let mut canvas = vec![[0x20u8, 0x20, 0x20]; w * h];

    for (row, &size) in sizes.iter().enumerate() {
        for (col, state) in states.iter().enumerate() {
            let px = render(state, size);
            for y in 0..size * ZOOM {
                for x in 0..size * ZOOM {
                    let i = ((y / ZOOM) * size + x / ZOOM) * 4;
                    if px[i + 3] != 0 {
                        let (cx, cy) = (PAD + col * cell + x, PAD + row * cell + y);
                        canvas[cy * w + cx] = [px[i], px[i + 1], px[i + 2]];
                    }
                }
            }
        }
    }

    // Minimal 24-bit BMP: bottom-up rows, BGR, each row padded to 4 bytes.
    let row_len = (w * 3 + 3) & !3;
    let mut bmp = Vec::with_capacity(54 + row_len * h);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((54 + row_len * h) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(w as i32).to_le_bytes());
    bmp.extend_from_slice(&(h as i32).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&24u16.to_le_bytes());
    bmp.extend_from_slice(&[0; 24]);
    for y in (0..h).rev() {
        for x in 0..w {
            let [r, g, b] = canvas[y * w + x];
            bmp.extend_from_slice(&[b, g, r]);
        }
        bmp.resize(bmp.len() + row_len - w * 3, 0);
    }

    std::fs::write("icon_preview.bmp", bmp).expect("failed to write icon_preview.bmp");
    println!("wrote icon_preview.bmp ({w}x{h})");
}
