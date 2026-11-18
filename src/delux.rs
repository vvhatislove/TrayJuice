//! Delux / Attack Shark mice on the shared `1d57` 2.4 GHz receiver.
//!
//! The receiver forwards the battery level as a 5-byte input report
//! `03 <model> 40 <state> <percent>` on the `0x0A/0x00` collection. This
//! backend only listens and never writes to the device.
//!
//! What was observed with a Delux M800 (listening tests and USB captures):
//! - While the reports flow, one arrives about every 2 seconds.
//! - The vendor app sends nothing to the mouse (two USB captures, 45 s and
//!   5 min). It finds the mouse through these same reports and shows
//!   "No Device" when they stop.
//! - The reports stopped three times, each time while no program kept the
//!   receiver open (vendor app closed, TrayJuice opening it only for a short
//!   window per poll), and came back only after a replug or reboot. With the
//!   vendor app running they never stopped.
//! - Working theory, not proven: Windows idles the receiver's interface once
//!   its last handle is closed, and the receiver does not resume the reports
//!   afterwards. This backend therefore keeps its handle open for as long as
//!   the receiver is plugged in. With that change the reports kept coming
//!   for 9 minutes after the vendor app was closed (10 polls out of 10);
//!   before it they had stopped within 1 to 4 minutes.
//!
//! Protocol facts come from:
//! - OpenMouse, <https://github.com/OpenMouse-Project/mouse-protocol>
//!   (`docs/delux-m600-pro-testing.md`, `src/delux/index.ts`);
//! - mouse-battery-tray, <https://github.com/incconutwo/mouse-battery-tray>
//!   (`battery_tray.pyw`: charging states).

use std::time::{Duration, Instant};

use hidapi::{HidApi, HidDevice};

use crate::device::{BatterySource, BatteryState, DeviceKind, DeviceStatus};

const VENDOR: u16 = 0x1d57;
/// Product IDs of the receiver. The same dongle was seen as `fa60` (all
/// tests were done in this mode) and, earlier, as `fa7d` (never listened to).
/// Listening on an unverified ID is safe: nothing is written, and only
/// reports of the exact battery format are accepted.
const RECEIVERS: [u16; 2] = [0xfa60, 0xfa7d];
const USAGE_PAGE: u16 = 0x000a;
const USAGE: u16 = 0x0000;

/// How long one poll waits for a report when none is queued: a bit longer
/// than the 2 seconds between reports.
const LISTEN: Duration = Duration::from_millis(2500);
/// How long the last level is still shown after the reports stop. After
/// that the mouse counts as off.
const REMEMBER: Duration = Duration::from_secs(10 * 60);

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
    /// Kept open between polls on purpose, see the module docs.
    device: Option<HidDevice>,
    last: Option<(Reading, Instant)>,
}

impl DeluxMice {
    /// Returns the newest report: first whatever Windows queued since the
    /// last poll, otherwise the next one to arrive within [`LISTEN`].
    /// `Err` means the handle is dead (receiver unplugged).
    fn listen(device: &HidDevice) -> Result<Option<Reading>, ()> {
        let mut buf = [0u8; 16];
        let mut newest = None;
        loop {
            match device.read_timeout(&mut buf, 0) {
                Ok(0) => break,
                Ok(n) => newest = parse_report(&buf[..n]).or(newest),
                Err(_) => return Err(()),
            }
        }
        let deadline = Instant::now() + LISTEN;
        while newest.is_none() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            let n = device
                .read_timeout(&mut buf, left.as_millis() as i32)
                .map_err(|_| ())?;
            newest = parse_report(&buf[..n]);
        }
        Ok(newest)
    }
}

impl BatterySource for DeluxMice {
    fn poll(&mut self, api: &HidApi) -> Vec<DeviceStatus> {
        let Some(info) = api.device_list().find(|d| {
            d.vendor_id() == VENDOR
                && RECEIVERS.contains(&d.product_id())
                && d.usage_page() == USAGE_PAGE
                && d.usage() == USAGE
        }) else {
            self.device = None;
            self.last = None;
            return Vec::new();
        };

        if self.device.is_none() {
            self.device = info.open_device(api).ok();
            crate::log!("delux: opened receiver: {}", self.device.is_some());
        }
        match self.device.as_ref().map(Self::listen) {
            Some(Ok(Some(reading))) => {
                crate::log!("delux: {reading:?}");
                self.last = Some((reading, Instant::now()));
            }
            Some(Ok(None)) => crate::log!("delux: no report"),
            // Reopen on the next poll.
            Some(Err(())) => self.device = None,
            None => {}
        }

        let (name, state) = match self.last {
            Some((reading, seen)) if seen.elapsed() < REMEMBER => (
                display_name(reading.model, seen.elapsed()),
                BatteryState::Level {
                    percent: reading.percent,
                    charging: reading.charging,
                },
            ),
            Some((reading, _)) => (model_name(reading.model).to_string(), BatteryState::Offline),
            None => (model_name(0).to_string(), BatteryState::Offline),
        };
        vec![DeviceStatus {
            name,
            kind: DeviceKind::Mouse,
            state,
        }]
    }
}

/// Device name for the tooltip. A level that was not heard during this poll
/// says how old it is, so a stale number is not mistaken for a live one.
fn display_name(model: u8, age: Duration) -> String {
    let name = model_name(model);
    match age.as_secs() / 60 {
        0 => name.to_string(),
        minutes => format!("{name} (heard {minutes} min ago)"),
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
        assert_eq!(display_name(0x3a, Duration::from_secs(20)), "Delux M800");
        assert_eq!(
            display_name(0x3a, Duration::from_secs(200)),
            "Delux M800 (heard 3 min ago)"
        );
        // Too short, wrong report id, wrong event, unknown state, impossible level.
        assert_eq!(parse_report(&[0x03, 0x3a, 0x40, 0x01]), None);
        assert_eq!(parse_report(&[0x02, 0x3a, 0x40, 0x01, 0x64]), None);
        assert_eq!(parse_report(&[0x03, 0x3a, 0x41, 0x01, 0x64]), None);
        assert_eq!(parse_report(&[0x03, 0x3a, 0x40, 0x07, 0x64]), None);
        assert_eq!(parse_report(&[0x03, 0x3a, 0x40, 0x01, 0x65]), None);
    }
}
