//! Delux / Attack Shark mice on the shared `1d57:fa60` 2.4 GHz receiver.
//!
//! The receiver forwards the battery level as a 5-byte input report
//! `03 <model> 40 <state> <percent>` on the `0x0A/0x00` collection. The mouse
//! sends it when it wakes up and whenever the vendor app asks, not on a fixed
//! schedule, so a short listening window once a minute misses it. This
//! backend therefore keeps the collection open on its own thread and
//! remembers the last report. It never writes to the device.
//!
//! Protocol facts come from:
//! - OpenMouse, <https://github.com/OpenMouse-Project/mouse-protocol>
//!   (`docs/delux-m600-pro-testing.md`, `src/delux/index.ts`);
//! - mouse-battery-tray, <https://github.com/incconutwo/mouse-battery-tray>
//!   (`battery_tray.pyw`: charging states, passive listening).

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use hidapi::HidApi;

use crate::device::{BatterySource, BatteryState, DeviceKind, DeviceStatus};

const VENDOR: u16 = 0x1d57;
const RECEIVER: u16 = 0xfa60;
const USAGE_PAGE: u16 = 0x000a;
const USAGE: u16 = 0x0000;

/// How long to wait before looking for the receiver again.
const RETRY: Duration = Duration::from_secs(5);
/// Upper bound for one blocking read, so an unplugged receiver is noticed.
const READ_TIMEOUT_MS: i32 = 5000;
/// Reports are rare, so the last level stays valid for a long time.
const REMEMBER: Duration = Duration::from_secs(2 * 60 * 60);

/// The receiver is shared between brands; the model byte names the mouse.
fn model_name(model: u8) -> &'static str {
    match model {
        0x3a => "Delux M800",
        0x20 => "Delux M600 Pro",
        0x55 => "Attack Shark X11",
        _ => "2.4G wireless mouse",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Reading {
    model: u8,
    percent: u8,
    charging: bool,
}

/// Parses `03 <model> 40 <state> <percent>`.
/// State `01` is discharging; `02`, `03` and `80` mean charging.
fn parse_report(report: &[u8]) -> Option<Reading> {
    match *report {
        [0x03, model, 0x40, state, percent, ..] if percent <= 100 => {
            let charging = match state {
                0x01 => false,
                0x02 | 0x03 | 0x80 => true,
                _ => return None,
            };
            Some(Reading {
                model,
                percent,
                charging,
            })
        }
        _ => None,
    }
}

#[derive(Default)]
struct Shared {
    /// The receiver is plugged in and its battery collection is open.
    present: bool,
    last: Option<(Reading, Instant)>,
}

#[derive(Default)]
pub struct DeluxMice {
    shared: Arc<Mutex<Shared>>,
    listening: bool,
}

impl BatterySource for DeluxMice {
    fn poll(&mut self, _api: &HidApi) -> Vec<DeviceStatus> {
        if !self.listening {
            self.listening = true;
            let shared = Arc::clone(&self.shared);
            thread::spawn(move || listen(&shared));
            // Give the listener a moment to find the receiver, so the mouse
            // is already part of the very first result.
            for _ in 0..20 {
                thread::sleep(Duration::from_millis(50));
                if self.shared.lock().is_ok_and(|s| s.present) {
                    break;
                }
            }
        }

        let Ok(shared) = self.shared.lock() else {
            return Vec::new();
        };
        if !shared.present {
            return Vec::new();
        }
        let (name, state) = match shared.last {
            Some((reading, seen)) if seen.elapsed() < REMEMBER => (
                model_name(reading.model),
                BatteryState::Level {
                    percent: reading.percent,
                    charging: reading.charging,
                },
            ),
            Some((reading, _)) => (model_name(reading.model), BatteryState::Offline),
            None => (model_name(0), BatteryState::Offline),
        };
        vec![DeviceStatus {
            name: name.to_string(),
            kind: DeviceKind::Mouse,
            state,
        }]
    }
}

/// Runs for the lifetime of the process: waits for the receiver, then blocks
/// on its battery collection and records every report.
fn listen(shared: &Mutex<Shared>) {
    let mut api = HidApi::new().ok();
    loop {
        let device = api.as_mut().and_then(|api| {
            api.refresh_devices().ok()?;
            let info = api.device_list().find(|d| {
                d.vendor_id() == VENDOR
                    && d.product_id() == RECEIVER
                    && d.usage_page() == USAGE_PAGE
                    && d.usage() == USAGE
            })?;
            info.open_device(api).ok()
        });
        if api.is_none() {
            api = HidApi::new().ok();
        }

        if let Some(device) = device {
            crate::log!("delux: listening on the receiver");
            if let Ok(mut shared) = shared.lock() {
                shared.present = true;
            }
            let mut buf = [0u8; 16];
            // Ends with a read error when the receiver is unplugged.
            while let Ok(n) = device.read_timeout(&mut buf, READ_TIMEOUT_MS) {
                if let Some(reading) = parse_report(&buf[..n]) {
                    crate::log!("delux: {reading:?}");
                    if let Ok(mut shared) = shared.lock() {
                        shared.last = Some((reading, Instant::now()));
                    }
                }
            }
            crate::log!("delux: receiver lost");
        }

        if let Ok(mut shared) = shared.lock() {
            shared.present = false;
        }
        thread::sleep(RETRY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(model: u8, percent: u8, charging: bool) -> Option<Reading> {
        Some(Reading {
            model,
            percent,
            charging,
        })
    }

    #[test]
    fn parses_battery_reports() {
        assert_eq!(
            parse_report(&[0x03, 0x3a, 0x40, 0x01, 0x64]),
            reading(0x3a, 100, false)
        );
        assert_eq!(
            parse_report(&[0x03, 0x20, 0x40, 0x01, 0x4d]),
            reading(0x20, 77, false)
        );
        assert_eq!(
            parse_report(&[0x03, 0x3a, 0x40, 0x02, 0x32]),
            reading(0x3a, 50, true)
        );
        // Too short, wrong report id, wrong event, unknown state, impossible level.
        assert_eq!(parse_report(&[0x03, 0x3a, 0x40, 0x01]), None);
        assert_eq!(parse_report(&[0x02, 0x3a, 0x40, 0x01, 0x64]), None);
        assert_eq!(parse_report(&[0x03, 0x3a, 0x41, 0x01, 0x64]), None);
        assert_eq!(parse_report(&[0x03, 0x3a, 0x40, 0x07, 0x64]), None);
        assert_eq!(parse_report(&[0x03, 0x3a, 0x40, 0x01, 0x65]), None);
    }
}
