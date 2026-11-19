//! Shows a sample low-battery notification on a tray icon of its own for
//! 10 seconds. Run with `cargo run --example alert_preview` to check how
//! the notification looks on a Windows build.

use std::time::Duration;

use tray_icon::{Icon, TrayIconBuilder};
use trayjuice::alert::TITLE;
use trayjuice::notify::Notifier;

fn main() {
    let size = 32;
    let image = Icon::from_rgba(trayjuice::icon::app_icon(size), size as u32, size as u32)
        .expect("icon buffer has the right size");
    let tray = TrayIconBuilder::new()
        .with_icon(image)
        .with_tooltip("TrayJuice alert preview")
        .build()
        .expect("failed to create tray icon");
    let mut notifier = Notifier::new(tray.window_handle());
    let shown = notifier.show(TITLE, "Logitech PRO X Wireless: 20%\nDelux M800: 9%");
    println!("notification accepted by Windows: {shown}");
    std::thread::sleep(Duration::from_secs(10));
}
