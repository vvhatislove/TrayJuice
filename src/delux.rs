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
//! - With the charging cable in, the mouse stays on the receiver and the
//!   state byte changes from `01` to `03`; the level keeps being reported
//!   (`03 3a 40 03 5c` at 92 %).
//! - Working theory, not proven: Windows idles the receiver's interface once
//!   its last handle is closed, and the receiver does not resume the reports
//!   afterwards. This backend therefore keeps its handle open for as long as
//!   the receiver is plugged in: a listener thread owns it and reads every
//!   report, so a change shows up within the 2 seconds between reports. With that change the reports kept coming
//!   for 9 minutes after the vendor app was closed (10 polls out of 10);
//!   before it they had stopped within 1 to 4 minutes.
//!
//! Protocol facts come from:
//! - OpenMouse, <https://github.com/OpenMouse-Project/mouse-protocol>
//!   (`docs/delux-m600-pro-testing.md`, `src/delux/index.ts`);
//! - mouse-battery-tray, <https://github.com/incconutwo/mouse-battery-tray>
//!   (`battery_tray.pyw`: charging states).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use hidapi::{HidApi, HidDevice};

use crate::device::{BatterySource, BatteryState, DeviceKind, DeviceStatus, Waker};

const VENDOR: u16 = 0x1d57;
/// Product IDs of the receiver. The same dongle was seen as `fa60` (all
/// tests were done in this mode) and, earlier, as `fa7d` (never listened to).
/// Listening on an unverified ID is safe: nothing is written, and only
/// reports of the exact battery format are accepted.
const RECEIVERS: [u16; 2] = [0xfa60, 0xfa7d];
const USAGE_PAGE: u16 = 0x000a;
const USAGE: u16 = 0x0000;

/// How long the last level is still shown after the reports stop. After
/// that the mouse counts as off.
const REMEMBER: Duration = Duration::from_secs(10 * 60);
/// A report after a silence this long means the mouse is back (woke up or
/// was switched on), which is worth showing at once.
const BACK_AFTER: Duration = Duration::from_secs(30);

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

/// What the listener thread shares with the poll thread.
#[derive(Default)]
struct Shared {
    last: Mutex<Option<(Reading, Instant)>>,
    /// Set once the receiver is gone and the thread has ended.
    dead: AtomicBool,
}

pub struct DeluxMice {
    wake: Waker,
    /// The listener of the receiver that is plugged in. Its thread owns the
    /// handle and keeps it open on purpose, see the module docs.
    listener: Option<Arc<Shared>>,
}

impl DeluxMice {
    pub fn new(wake: Waker) -> Self {
        Self {
            wake,
            listener: None,
        }
    }
}

/// Reads the reports as they come, about one every 2 seconds, and keeps the
/// newest. Asks for a poll when the level or the charging state changes or
/// the mouse is heard again after a silence. Sends nothing. Returns when the
/// receiver is unplugged.
fn listen(device: &HidDevice, shared: &Shared, wake: &Waker) {
    let mut buf = [0u8; 16];
    loop {
        let Ok(n) = device.read_timeout(&mut buf, -1) else {
            return;
        };
        let Some(reading) = parse_report(&buf[..n]) else {
            continue;
        };
        let before = shared
            .last
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace((reading, Instant::now()));
        if is_news(before, reading) {
            crate::log!("delux: {reading:?}");
            wake();
        }
    }
}

/// Whether `reading` differs from what was known `before` in a way the user
/// should see right away.
fn is_news(before: Option<(Reading, Instant)>, reading: Reading) -> bool {
    before.is_none_or(|(old, at)| old != reading || at.elapsed() > BACK_AFTER)
}

impl BatterySource for DeluxMice {
    fn poll(&mut self, api: &HidApi) -> Vec<DeviceStatus> {
        let Some(info) = api.device_list().find(|d| {
            d.vendor_id() == VENDOR
                && RECEIVERS.contains(&d.product_id())
                && d.usage_page() == USAGE_PAGE
                && d.usage() == USAGE
        }) else {
            self.listener = None;
            return Vec::new();
        };

        if self
            .listener
            .as_ref()
            .is_none_or(|l| l.dead.load(Ordering::Relaxed))
        {
            self.listener = info.open_device(api).ok().map(|device| {
                let shared = Arc::new(Shared::default());
                let (theirs, wake) = (shared.clone(), self.wake.clone());
                thread::spawn(move || {
                    listen(&device, &theirs, &wake);
                    theirs.dead.store(true, Ordering::Relaxed);
                });
                shared
            });
            crate::log!("delux: opened receiver: {}", self.listener.is_some());
        }
        let last = self
            .listener
            .as_ref()
            .and_then(|l| *l.last.lock().unwrap_or_else(|e| e.into_inner()));

        let (name, state) = match last {
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
        // On the charger, as an M800 sent it.
        assert_eq!(
            parse_report(&[0x03, 0x3a, 0x40, 0x03, 0x5c]),
            reading(0x3a, 92, true)
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

    #[test]
    fn asks_for_a_poll_only_on_news() {
        let now = Instant::now();
        let known = reading(0x3a, 92, false).unwrap();
        let on_charger = reading(0x3a, 92, true).unwrap();
        assert!(is_news(None, known));
        assert!(!is_news(Some((known, now)), known));
        assert!(is_news(Some((known, now)), on_charger));
        assert!(is_news(
            Some((known, now)),
            reading(0x3a, 91, false).unwrap()
        ));
        if let Some(long_ago) = now.checked_sub(Duration::from_secs(60)) {
            assert!(is_news(Some((known, long_ago)), known));
        }
    }
}
