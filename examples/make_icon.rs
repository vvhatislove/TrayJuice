//! Writes `assets/trayjuice.ico`, the icon of the executable and of the
//! installer, from the drawing in `src/icon.rs`. Run it again with
//! `cargo run --example make_icon` whenever that drawing changes, and commit
//! the file.

use trayjuice::icon::app_icon;

/// Sizes Windows picks from for lists, the taskbar, tiles and large views.
const SIZES: [usize; 7] = [16, 20, 24, 32, 48, 64, 256];

/// One image of an icon file: a 32-bit bitmap with the unused 1-bit mask
/// the format still asks for.
fn image(size: usize) -> Vec<u8> {
    let rgba = app_icon(size);
    let mask_row = size.div_ceil(32) * 4;
    let mut out = Vec::with_capacity(40 + size * size * 4 + mask_row * size);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    // Colour rows and mask rows together.
    out.extend_from_slice(&(2 * size as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    // Bottom row first, blue-green-red-alpha.
    for y in (0..size).rev() {
        for x in 0..size {
            let p = &rgba[(y * size + x) * 4..][..4];
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    out.resize(out.len() + mask_row * size, 0);
    out
}

fn main() {
    let images: Vec<Vec<u8>> = SIZES.iter().map(|size| image(*size)).collect();

    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * SIZES.len();
    for (size, image) in SIZES.iter().zip(&images) {
        // A side of 256 is written as 0.
        let side = if *size >= 256 { 0 } else { *size as u8 };
        ico.extend_from_slice(&[side, side, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(image.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += image.len();
    }
    for image in &images {
        ico.extend_from_slice(image);
    }

    std::fs::create_dir_all("assets").expect("failed to create assets");
    std::fs::write("assets/trayjuice.ico", &ico).expect("failed to write the icon");
    println!("wrote assets/trayjuice.ico ({} bytes)", ico.len());
}
