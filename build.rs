//! Embeds the Windows application manifest (Common Controls v6 for themed
//! tooltips, per-monitor DPI awareness) and the version information shown in
//! the file's properties.

use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest(new_manifest("TrayJuice")).expect("unable to embed manifest");

        // Version numbers come from Cargo.toml by default.
        let mut resource = winresource::WindowsResource::new();
        resource
            .set("ProductName", "TrayJuice")
            .set("FileDescription", "TrayJuice")
            .set("OriginalFilename", "trayjuice.exe")
            .set("LegalCopyright", "Copyright (c) 2026 Vladislav Petrenko");
        resource
            .compile()
            .expect("unable to embed version information");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
