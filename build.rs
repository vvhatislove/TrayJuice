//! Embeds the Windows application manifest: Common Controls v6 (themed
//! tooltips) and per-monitor DPI awareness.

use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest(new_manifest("TrayJuice")).expect("unable to embed manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
