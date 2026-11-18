//! Logitech wireless headsets that speak HID++ over their USB receiver.
//!
//! Protocol details (request bytes, response layout, discharge polynomial)
//! follow the HeadsetControl project: <https://github.com/Sapd/HeadsetControl>
//! (`src/devices/logitech_gpro.c` in release 3.0.0 for the polynomial).
//!
//! Observed on a PRO X Wireless (`046d:0aba`), not taken from a source:
//!
//! - The headset announces a change of its charge state by itself. Pulling
//!   the charging cable gave two reports `11 ff 06 00 <mV> 01` a second
//!   apart, plugging it back two reports `11 ff 06 00 <mV> 03` three seconds
//!   apart. Nothing else arrived unasked in 100 seconds. A listener thread
//!   keeps the receiver open for these reports and asks for a poll, so the
//!   widget follows the cable within a second instead of a minute. Whether
//!   other models send them is not known; without them the regular poll
//!   still works.
//! - On the charger the reported voltage is that under charge current, far
//!   above the resting value: 3658 mV (21 %) became 3978 mV a minute after
//!   plugging in. The discharge polynomial reads that as 87 %, so charging
//!   uses its own table, see [`CHARGE_CURVE`].
//! - Only charge states `01` (discharging) and `03` (charging) were seen.
//!   What a full headset on the charger reports is not known.

use std::collections::HashMap;
use std::ffi::CString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use hidapi::{HidApi, HidDevice};

use crate::device::{
    BatterySource, BatteryState, DeviceKind, DeviceStatus, Waker, percent_from_voltage,
};

const VENDOR_LOGITECH: u16 = 0x046d;

const HIDPP_LONG: u8 = 0x11;
const HIDPP_LONG_LEN: usize = 20;
const HIDPP_RECEIVER: u8 = 0xff;
/// Sub-ID of an HID++ 2.0 error reply.
const HIDPP_ERROR: u8 = 0xff;
/// Charge state byte of a reply while the headset is on the charger.
const STATE_CHARGING: u8 = 0x03;

const REPLY_TIMEOUT: Duration = Duration::from_millis(1000);
/// A cable change comes as a burst of reports with the same charge state;
/// one poll per burst is enough.
const EVENT_QUIET: Duration = Duration::from_secs(5);

struct Model {
    product_id: u16,
    name: &'static str,
    usage_page: u16,
    usage: u16,
    /// Feature index + function of the "battery voltage" request.
    battery_cmd: [u8; 2],
    /// Discharge polynomial, see [`percent_from_voltage`].
    curve: &'static [f64],
    /// Readings below this are a headset that is off, not an empty battery.
    min_mv: u16,
}

/// Fitted to the G PRO series by HeadsetControl. Checked against G HUB on a
/// PRO X Wireless: 3692 mV gives 27 %, the same number G HUB showed.
const GPRO_CURVE: &[f64] = &[
    -1.779_008_582_425_361_3e6,
    2.369_215_330_734_489e3,
    -1.258_090_504_150_684,
    3.329_520_118_738_839_5e-4,
    -4.391_398_173_359_75e-8,
    2.309_312_104_553_990_7e-12,
];
const GPRO_MIN_MV: u16 = 3300;

/// Voltage on the charger (mV) to percent. An estimate from two charges of
/// one PRO X Wireless, scaled to what G HUB shows:
///
/// - Up to 4183 mV: a charge logged from 21 % to 71 % in 70 minutes (71 % is
///   what the discharge polynomial said right after unplugging). The
///   voltages are the logged ones; the percentages assume the level rose
///   evenly in between.
/// - The last two points are G HUB's own numbers, read from its settings
///   database during a second charge (4233 mV: 81.8 %, 4237 mV: 82.4 %).
///   Unplugged at 4240 mV, the headset read 3932 mV (79 %) three minutes
///   later.
///
/// Outside the table the nearest segment is extended; nothing was measured
/// below 21 % or above 82 %.
const CHARGE_CURVE: &[(u16, u8)] = &[
    (3978, 21),
    (4013, 25),
    (4049, 30),
    (4074, 36),
    (4102, 47),
    (4134, 59),
    (4156, 65),
    (4183, 71),
    (4237, 82),
];
/// The headset still says "charging", so do not claim it is full.
const CHARGE_MAX_PERCENT: u8 = 99;
/// After unplugging, the voltage needs a while to fall back to its resting
/// value (4219 mV, which reads as 100 %, in the first second). For this long
/// the level is not allowed above the last estimate made on the charger.
const SETTLE: Duration = Duration::from_secs(20 * 60);

const fn gpro(product_id: u16, name: &'static str) -> Model {
    Model {
        product_id,
        name,
        usage_page: 0xff43,
        usage: 0x0202,
        battery_cmd: [0x06, 0x0d],
        curve: GPRO_CURVE,
        min_mv: GPRO_MIN_MV,
    }
}

const MODELS: &[Model] = &[
    gpro(0x0aa7, "Logitech G PRO Wireless Headset"),
    gpro(0x0aaa, "Logitech PRO X Wireless"),
    gpro(0x0aba, "Logitech PRO X Wireless"),
    gpro(0x0afb, "Logitech PRO X 2"),
    gpro(0x0afc, "Logitech PRO X 2"),
];

/// What earlier replies of one headset said, to smooth over the moments
/// when the voltage alone is misleading.
#[derive(Default)]
struct Memory {
    /// Last level read off the charger.
    discharging: Option<u8>,
    /// Last estimate made on the charger, and when.
    charging: Option<(u8, Instant)>,
}

/// One open receiver whose thread waits for reports the headset sends by itself.
struct Listener {
    path: CString,
    alive: Arc<AtomicBool>,
}

pub struct LogitechHeadsets {
    wake: Waker,
    listeners: Vec<Listener>,
    /// Keyed by product ID.
    memory: HashMap<u16, Memory>,
}

impl LogitechHeadsets {
    pub fn new(wake: Waker) -> Self {
        Self {
            wake,
            listeners: Vec::new(),
            memory: HashMap::new(),
        }
    }

    /// Starts a listener for the receiver at `path` unless one is running.
    /// Its handle stays open, unlike the one used for polling: a report sent
    /// while nothing is open would be lost.
    fn ensure_listener(&mut self, api: &HidApi, path: &std::ffi::CStr, model: &Model) {
        self.listeners.retain(|l| l.alive.load(Ordering::Relaxed));
        if self.listeners.iter().any(|l| l.path.as_c_str() == path) {
            return;
        }
        let Ok(dev) = api.open_path(path) else {
            return;
        };
        let alive = Arc::new(AtomicBool::new(true));
        let (flag, wake, feature) = (alive.clone(), self.wake.clone(), model.battery_cmd[0]);
        thread::spawn(move || {
            listen(&dev, feature, &wake);
            flag.store(false, Ordering::Relaxed);
        });
        self.listeners.push(Listener {
            path: path.to_owned(),
            alive,
        });
    }
}

impl BatterySource for LogitechHeadsets {
    fn poll(&mut self, api: &HidApi) -> Vec<DeviceStatus> {
        let mut out = Vec::new();
        for info in api.device_list() {
            if info.vendor_id() != VENDOR_LOGITECH {
                continue;
            }
            let Some(model) = MODELS.iter().find(|m| {
                m.product_id == info.product_id()
                    && m.usage_page == info.usage_page()
                    && m.usage == info.usage()
            }) else {
                continue;
            };
            self.ensure_listener(api, info.path(), model);
            let memory = self.memory.entry(model.product_id).or_default();
            let state = info
                .open_device(api)
                .ok()
                .and_then(|dev| read_battery(&dev, model))
                .and_then(|(mv, state)| state_from_reply(model, mv, state, memory, Instant::now()))
                .unwrap_or(BatteryState::Offline);
            out.push(DeviceStatus {
                name: model.name.to_string(),
                kind: DeviceKind::Headset,
                state,
            });
        }
        out
    }
}

/// Blocks on the receiver and calls `wake` when the headset announces a
/// battery change. Sends nothing. Returns when the receiver is unplugged.
fn listen(dev: &HidDevice, feature: u8, wake: &Waker) {
    let mut buf = [0u8; HIDPP_LONG_LEN];
    // Charge state and time of the last event that led to a poll.
    let mut last_wake: Option<(u8, Instant)> = None;
    loop {
        let Ok(n) = dev.read_timeout(&mut buf, -1) else {
            return;
        };
        if !is_battery_event(&buf[..n], feature) {
            continue;
        }
        // A repeat of the same state is skipped; a cable that goes out and
        // back in within the quiet time still gets through.
        if last_wake.is_some_and(|(state, at)| state == buf[6] && at.elapsed() < EVENT_QUIET) {
            continue;
        }
        last_wake = Some((buf[6], Instant::now()));
        crate::log!("logitech: battery event {:02x?}", &buf[..7]);
        wake();
    }
}

/// `true` for a report the headset sent by itself about its battery:
/// `11 ff <feature> 00 …`. Replies to requests carry the asker's software
/// ID in the low half of the fourth byte (`0d` is ours, `0b` was G HUB's).
fn is_battery_event(report: &[u8], feature: u8) -> bool {
    report.len() >= 7 && report[..4] == [HIDPP_LONG, HIDPP_RECEIVER, feature, 0x00]
}

/// Asks for the battery voltage. Returns millivolts and the charge state
/// byte, or `None` when the headset does not answer (off / out of range).
fn read_battery(dev: &HidDevice, model: &Model) -> Option<(u16, u8)> {
    let mut request = [0u8; HIDPP_LONG_LEN];
    request[0] = HIDPP_LONG;
    request[1] = HIDPP_RECEIVER;
    request[2..4].copy_from_slice(&model.battery_cmd);
    dev.write(&request).ok()?;

    // Other software (G HUB) talks to the same receiver, so unrelated reports
    // can arrive in between: keep reading until ours shows up or time runs out.
    let deadline = Instant::now() + REPLY_TIMEOUT;
    let mut buf = [0u8; HIDPP_LONG_LEN];
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let n = dev.read_timeout(&mut buf, left.as_millis() as i32).ok()?;
        if n < 7 || buf[0] != HIDPP_LONG || buf[1] != HIDPP_RECEIVER {
            continue;
        }
        // Error reply: `11 ff ff <feature> <function> <code>`. Other software
        // can get errors for the same feature, so match the function too.
        if buf[2] == HIDPP_ERROR && buf[3..5] == model.battery_cmd {
            return None;
        }
        if buf[2..4] != model.battery_cmd {
            continue;
        }
        let mv = u16::from_be_bytes([buf[4], buf[5]]);
        crate::log!(
            "logitech: {} reports {mv} mV, state {:#04x}",
            model.name,
            buf[6]
        );
        return Some((mv, buf[6]));
    }
}

/// Percent for a voltage read on the charger, from [`CHARGE_CURVE`].
fn percent_on_charger(mv: u16) -> u8 {
    let curve = CHARGE_CURVE;
    // The segment that holds `mv`; the outer ones also serve what lies beyond.
    let i = curve[1..curve.len() - 1]
        .iter()
        .take_while(|p| p.0 <= mv)
        .count();
    let ((v0, p0), (v1, p1)) = (curve[i], curve[i + 1]);
    let slope = (p1 as f64 - p0 as f64) / (v1 as f64 - v0 as f64);
    let percent = p0 as f64 + (mv as f64 - v0 as f64) * slope;
    percent.round().clamp(0.0, CHARGE_MAX_PERCENT as f64) as u8
}

/// Turns the voltage and charge state of a reply into a battery state.
fn state_from_reply(
    model: &Model,
    mv: u16,
    state: u8,
    memory: &mut Memory,
    now: Instant,
) -> Option<BatteryState> {
    if mv < model.min_mv {
        return None;
    }
    if state != STATE_CHARGING {
        let mut percent = percent_from_voltage(model.curve, mv);
        match memory.charging {
            Some((cap, since)) if now.duration_since(since) < SETTLE => percent = percent.min(cap),
            _ => memory.charging = None,
        }
        memory.discharging = Some(percent);
        return Some(BatteryState::Level {
            percent,
            charging: false,
        });
    }

    // For the first minute on the charger the voltage is still climbing
    // (3910 mV at the moment of plugging in, 4140 mV a minute later), and a
    // battery on the charger does not lose charge: never go below the level
    // it had before.
    let floor = memory.discharging.unwrap_or(0);
    let percent = percent_on_charger(mv).max(floor).min(CHARGE_MAX_PERCENT);
    memory.charging = Some((percent, now));
    Some(BatteryState::Level {
        percent,
        charging: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(percent: u8, charging: bool) -> Option<BatteryState> {
        Some(BatteryState::Level { percent, charging })
    }

    #[test]
    fn gpro_curve_matches_known_points() {
        assert_eq!(percent_from_voltage(GPRO_CURVE, 3692), 27);
        assert_eq!(percent_from_voltage(GPRO_CURVE, 3900), 73);
        assert_eq!(percent_from_voltage(GPRO_CURVE, 4200), 100);
        assert_eq!(percent_from_voltage(GPRO_CURVE, 3300), 0);
    }

    #[test]
    fn charge_curve_interpolates_and_stays_in_range() {
        assert_eq!(percent_on_charger(3978), 21);
        assert_eq!(percent_on_charger(4118), 53);
        assert_eq!(percent_on_charger(4233), 81);
        assert_eq!(percent_on_charger(4237), 82);
        // Beyond the table the outer segments are extended.
        assert_eq!(percent_on_charger(3943), 17);
        assert_eq!(percent_on_charger(4262), 87);
        assert_eq!(percent_on_charger(3400), 0);
        assert_eq!(percent_on_charger(4500), 99);
    }

    #[test]
    fn follows_a_logged_charge() {
        let model = &MODELS[0];
        let mut memory = Memory::default();
        let start = Instant::now();
        let mut at = |secs: u64, mv, state| {
            let now = start + Duration::from_secs(secs);
            state_from_reply(model, mv, state, &mut memory, now)
        };
        assert_eq!(at(0, 3658, 0x01), level(21, false));
        // Just plugged in: the voltage has not climbed yet, keep the old level.
        assert_eq!(at(60, 3910, 0x03), level(21, true));
        assert_eq!(at(120, 3978, 0x03), level(21, true));
        assert_eq!(at(4200, 4183, 0x03), level(71, true));
        assert_eq!(at(5000, 4240, 0x03), level(83, true));
        // Unplugged: the polynomial says 100 % until the voltage drops.
        assert_eq!(at(5007, 4240, 0x01), level(83, false));
        assert_eq!(at(5200, 3932, 0x01), level(79, false));
        // Cable back in while the voltage is low again: keep the level.
        assert_eq!(at(5210, 3941, 0x03), level(79, true));
    }

    #[test]
    fn a_voltage_below_the_minimum_is_a_headset_that_is_off() {
        let mut memory = Memory::default();
        let now = Instant::now();
        assert_eq!(
            state_from_reply(&MODELS[0], 0, 0x01, &mut memory, now),
            None
        );
        assert_eq!(
            state_from_reply(&MODELS[0], 0, 0x03, &mut memory, now),
            None
        );
    }

    #[test]
    fn tells_events_from_replies() {
        let event = [0x11, 0xff, 0x06, 0x00, 0x0f, 0x46, 0x03, 0, 0];
        let ours = [0x11, 0xff, 0x06, 0x0d, 0x10, 0x69, 0x03, 0, 0];
        let ghub = [0x11, 0xff, 0x06, 0x0b, 0x10, 0x68, 0x03, 0, 0];
        assert!(is_battery_event(&event, 0x06));
        assert!(!is_battery_event(&ours, 0x06));
        assert!(!is_battery_event(&ghub, 0x06));
        assert!(!is_battery_event(&event[..4], 0x06));
        assert!(!is_battery_event(&event, 0x07));
    }
}
