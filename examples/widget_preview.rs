//! Writes `widget_preview.bmp`: the taskbar widget for sample devices on a
//! dark and a light background. Run with `cargo run --example widget_preview`.

use trayjuice::device::{BatteryState, DeviceKind, DeviceStatus};
use trayjuice::taskbar::render;

const ZOOM: usize = 3;

fn main() {
    let device = |kind, state| DeviceStatus {
        name: String::new(),
        kind,
        state,
    };
    let level = |percent, charging| BatteryState::Level { percent, charging };
    let statuses = [
        device(DeviceKind::Headset, level(30, false)),
        device(DeviceKind::Mouse, level(93, false)),
        device(DeviceKind::Keyboard, level(100, true)),
        device(DeviceKind::Gamepad, level(16, false)),
        device(DeviceKind::Other, BatteryState::Offline),
    ];

    let themes = [(false, [0x1cu32, 0x1c, 0x1c]), (true, [0xf3, 0xf3, 0xf3])];
    let bitmaps: Vec<_> = themes
        .iter()
        .map(|t| render(&statuses, 48, 96, t.0))
        .collect();
    let (bw, bh) = (bitmaps[0].width as usize, bitmaps[0].height as usize);
    let (w, h) = (bw * ZOOM, bh * ZOOM * themes.len());

    let mut canvas = vec![[0u8; 3]; w * h];
    for (i, (bitmap, (_, bg))) in bitmaps.iter().zip(&themes).enumerate() {
        for y in 0..bh * ZOOM {
            for x in 0..bw * ZOOM {
                let p = bitmap.pixels[(y / ZOOM) * bw + x / ZOOM];
                let a = p >> 24;
                // Source is premultiplied: out = src + bg * (1 - a).
                let mix =
                    |shift: u32, bg: u32| (((p >> shift) & 0xff) + bg * (255 - a) / 255) as u8;
                canvas[(i * bh * ZOOM + y) * w + x] =
                    [mix(16, bg[0]), mix(8, bg[1]), mix(0, bg[2])];
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
    std::fs::write("widget_preview.bmp", bmp).expect("failed to write widget_preview.bmp");
    println!("wrote widget_preview.bmp ({w}x{h})");
}
