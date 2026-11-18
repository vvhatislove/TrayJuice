//! Delux / Attack Shark mice on the shared `1d57:fa60` 2.4 GHz receiver.
//!
//! The receiver forwards the battery level as a 5-byte input report
//! `03 <model> 40 <state> <percent>` on the `0x0A/0x00` collection. This
//! backend only listens and never writes to the device.
//!
//! Two constraints shape it:
//! - The Delux M800 sends the report only while the vendor app asks for it
//!   (about every 2 seconds while its window is open), so most polls hear
//!   nothing and the last level has to be remembered.
//! - The vendor app shows "No Device" while another process keeps this
//!   collection open, so it must be opened only for a short window per poll
//!   and never held.
//!
//! Protocol facts come from:
//! - OpenMouse, <https://github.com/OpenMouse-Project/mouse-protocol>
//!   (`docs/delux-m600-pro-testing.md`, `src/delux/index.ts`);
//! - mouse-battery-tray, <https://github.com/incconutwo/mouse-battery-tray>
//!   (`battery_tray.pyw`: charging states).

use std::time::{Duration, Instant};

use hidapi::HidApi;

use crate::device::{BatterySource, BatteryState, DeviceKind, DeviceStatus};

const VENDOR: u16 = 0x1d57;
const RECEIVER: u16 = 0xfa60;
const USAGE_PAGE: u16 = 0x000a;
const USAGE: u16 = 0x0000;

/// Listening window per poll. Long enough to catch one report while the
/// vendor app is asking, short enough not to lock the app out for long.
const LISTEN: Duration = Duration::from_millis(2500);
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
pub struct DeluxMice {
    last: Option<(Reading, Instant)>,
}

impl BatterySource for DeluxMice {
    fn poll(&mut self, api: &HidApi) -> Vec<DeviceStatus> {
        let Some(info) = api.device_list().find(|d| {
            d.vendor_id() == VENDOR
                && d.product_id() == RECEIVER
                && d.usage_page() == USAGE_PAGE
                && d.usage() == USAGE
        }) else {
            self.last = None;
            return Vec::new();
        };

        // The handle is dropped at the end of this block, see the module docs.
        let heard = info.open_device(api).ok().and_then(|device| {
            let deadline = Instant::now() + LISTEN;
            let mut buf = [0u8; 16];
            loop {
                let left = deadline.checked_duration_since(Instant::now())?;
                let n = device
                    .read_timeout(&mut buf, left.as_millis() as i32)
                    .ok()?;
                if let Some(reading) = parse_report(&buf[..n]) {
                    return Some(reading);
                }
            }
        });
        if let Some(reading) = heard {
            crate::log!("delux: {reading:?}");
            self.last = Some((reading, Instant::now()));
        }

        let (name, state) = match self.last {
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
