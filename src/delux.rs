//! Delux / Attack Shark mice on the shared `1d57:fa60` 2.4 GHz receiver.
//!
//! The receiver pushes the battery level by itself every couple of seconds as
//! a 5-byte input report `03 <model> 40 01 <percent>` on the `0x0A/0x00`
//! collection, so this backend only listens and never writes to the device.
//!
//! Protocol facts come from the OpenMouse project:
//! <https://github.com/OpenMouse-Project/mouse-protocol>
//! (`docs/delux-m600-pro-testing.md`, `src/delux/index.ts`).

use std::time::{Duration, Instant};

use hidapi::HidApi;

use crate::device::{BatterySource, BatteryState, DeviceKind, DeviceStatus};

const VENDOR: u16 = 0x1d57;
const RECEIVER: u16 = 0xfa60;
const USAGE_PAGE: u16 = 0x000a;
const USAGE: u16 = 0x0000;

/// Reports arrive about every 2 seconds while the mouse is awake.
const LISTEN: Duration = Duration::from_millis(3500);
/// A sleeping mouse sends nothing; keep showing its last level for a while.
const REMEMBER: Duration = Duration::from_secs(15 * 60);

/// The receiver is shared between brands; the model byte names the mouse.
fn model_name(model: u8) -> &'static str {
    match model {
        0x3a => "Delux M800",
        0x20 => "Delux M600 Pro",
        0x55 => "Attack Shark X11",
        _ => "2.4G wireless mouse",
    }
}

/// Parses `03 <model> 40 01 <percent>` into `(model, percent)`.
fn parse_report(report: &[u8]) -> Option<(u8, u8)> {
    match *report {
        [0x03, model, 0x40, 0x01, percent, ..] if percent <= 100 => Some((model, percent)),
        _ => None,
    }
}

#[derive(Default)]
pub struct DeluxMice {
    last: Option<(u8, u8, Instant)>,
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

        if let Some((model, percent)) = info.open_device(api).ok().and_then(|dev| {
            let deadline = Instant::now() + LISTEN;
            let mut buf = [0u8; 16];
            loop {
                let left = deadline.checked_duration_since(Instant::now())?;
                let n = dev.read_timeout(&mut buf, left.as_millis() as i32).ok()?;
                if let Some(found) = parse_report(&buf[..n]) {
                    return Some(found);
                }
            }
        }) {
            self.last = Some((model, percent, Instant::now()));
        }

        let (name, state) = match self.last {
            Some((model, percent, seen)) if seen.elapsed() < REMEMBER => (
                model_name(model),
                BatteryState::Level {
                    percent,
                    charging: false,
                },
            ),
            Some((model, ..)) => (model_name(model), BatteryState::Offline),
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

    #[test]
    fn parses_battery_reports() {
        assert_eq!(
            parse_report(&[0x03, 0x3a, 0x40, 0x01, 0x64]),
            Some((0x3a, 100))
        );
        assert_eq!(
            parse_report(&[0x03, 0x20, 0x40, 0x01, 0x4d]),
            Some((0x20, 77))
        );
        // Too short, wrong report id, wrong event, impossible level.
        assert_eq!(parse_report(&[0x03, 0x3a, 0x40, 0x01]), None);
        assert_eq!(parse_report(&[0x02, 0x3a, 0x40, 0x01, 0x64]), None);
        assert_eq!(parse_report(&[0x03, 0x3a, 0x41, 0x01, 0x64]), None);
        assert_eq!(parse_report(&[0x03, 0x3a, 0x40, 0x01, 0x65]), None);
    }
}
