//! Diagnostic: embeds the real widget with sample devices for 30 seconds and
//! reports every second whether its window is still alive.
//! Run with `cargo run --example widget_live`.

use std::time::{Duration, Instant};

use trayjuice::device::{BatteryState, DeviceKind, DeviceStatus};
use trayjuice::taskbar::Widget;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

fn main() {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let statuses = [
        DeviceStatus {
            name: "Sample headset".into(),
            kind: DeviceKind::Headset,
            state: BatteryState::Level {
                percent: 30,
                charging: false,
            },
        },
        DeviceStatus {
            name: "Sample mouse".into(),
            kind: DeviceKind::Mouse,
            state: BatteryState::Level {
                percent: 93,
                charging: false,
            },
        },
    ];
    let mut widget = Widget::new();
    println!("update -> {}", widget.update(&statuses));

    let start = Instant::now();
    let mut last_report = 0;
    while start.elapsed() < Duration::from_secs(30) {
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                DispatchMessageW(&msg);
            }
        }
        let second = start.elapsed().as_secs();
        if second != last_report {
            last_report = second;
            println!("{second:>2}s alive={}", widget.is_alive());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
