//! Logitech wireless headsets that speak HID++ over their USB receiver.
//!
//! Protocol details (request bytes, response layout, discharge polynomial)
//! follow the HeadsetControl project: <https://github.com/Sapd/HeadsetControl>
//! (`src/devices/logitech_gpro.c` in release 3.0.0 for the polynomial).
//!
//! The battery tables of the PRO X Wireless `046d:0aba` are reduced from the
//! power model G HUB installs for that headset
//! (`pro_headset\pro_x_wireless_headset_battery.xml` under
//! `%ProgramData%\LGHUB\depots`: a discharge curve and a two-stage charging
//! curve). With them the level agrees with G HUB within a point, on and off
//! the charger. The polynomial is up to 12 points lower than that model
//! between 3750 and 3900 mV.
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
//!   above the resting value: 3658 mV became 3978 mV a minute after plugging
//!   in. A discharge curve reads that far too high, so charging needs its
//!   own curve; a model without one shows no number while it charges.
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
    BatterySource, BatteryState, DeviceKind, DeviceStatus, Waker, percent_from_table,
    percent_from_voltage,
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
/// A poll that comes sooner than this after the last request, and not
/// because the headset announced a change, gets the last answer again. Other
/// backends ask for polls too (a mouse on its charger did so every 2
/// seconds), and each request wakes the headset's radio.
const MIN_REQUEST_GAP: Duration = Duration::from_secs(30);
/// A cable change comes as a burst of reports with the same charge state;
/// one poll per burst is enough.
const EVENT_QUIET: Duration = Duration::from_secs(5);

/// How a voltage read off the charger becomes a level.
enum Discharge {
    /// See [`percent_from_voltage`].
    Polynomial(&'static [f64]),
    /// See [`percent_from_table`].
    Table(&'static [(u16, f32)]),
}

/// How a voltage read on the charger becomes a level. Charging has two
/// stages: the voltage rises to the last point of `rising`, then falls
/// slowly along `topping` while the battery is topped up.
struct Charge {
    rising: &'static [(u16, f32)],
    topping: &'static [(u16, f32)],
}

struct Model {
    product_id: u16,
    name: &'static str,
    usage_page: u16,
    usage: u16,
    /// Feature index + function of the "battery voltage" request.
    battery_cmd: [u8; 2],
    discharge: Discharge,
    /// `None` when nothing is known about this model on the charger.
    charge: Option<Charge>,
    /// Readings below this are a headset that is off, not an empty battery.
    min_mv: u16,
}

/// Fitted to the G PRO series by HeadsetControl. Used for the models whose
/// own curve is not known.
const GPRO_CURVE: &[f64] = &[
    -1.779_008_582_425_361_3e6,
    2.369_215_330_734_489e3,
    -1.258_090_504_150_684,
    3.329_520_118_738_839_5e-4,
    -4.391_398_173_359_75e-8,
    2.309_312_104_553_990_7e-12,
];
const GPRO_MIN_MV: u16 = 3300;

/// PRO X Wireless `0aba`, off the charger: millivolts to percent. Reduced
/// from G HUB's discharge curve to the points that keep it within 0.6 of a
/// point.
const PRO_X_DISCHARGE: &[(u16, f32)] = &[
    (3293, 0.0),
    (3533, 9.3),
    (3644, 15.0),
    (3653, 16.7),
    (3664, 20.8),
    (3681, 23.7),
    (3708, 30.4),
    (3734, 39.1),
    (3757, 49.4),
    (3773, 55.4),
    (3796, 61.9),
    (3820, 67.1),
    (3975, 88.2),
    (4102, 100.0),
];
/// PRO X Wireless `0aba` on the charger while the voltage rises, reduced the
/// same way from G HUB's charging curve.
const PRO_X_CHARGE_RISING: &[(u16, f32)] = &[
    (3444, 0.2),
    (3749, 2.1),
    (3874, 4.4),
    (3921, 6.4),
    (4008, 21.6),
    (4020, 25.5),
    (4044, 38.2),
    (4061, 44.9),
    (4087, 52.8),
    (4123, 61.7),
    (4154, 68.2),
    (4202, 76.7),
    (4238, 82.4),
    (4275, 86.9),
];
/// The second stage: the voltage falls from 4273 mV while the level goes
/// from 87 % to full.
const PRO_X_CHARGE_TOPPING: &[(u16, f32)] =
    &[(4206, 99.9), (4241, 93.0), (4242, 92.1), (4273, 87.2)];
/// After unplugging, the voltage needs a moment to fall back (4219 mV in
/// the first second, 3937 mV a minute later). For this long the level is not
/// allowed above the last estimate made on the charger.
const SETTLE: Duration = Duration::from_secs(120);

const fn gpro(product_id: u16, name: &'static str) -> Model {
    Model {
        product_id,
        name,
        usage_page: 0xff43,
        usage: 0x0202,
        battery_cmd: [0x06, 0x0d],
        discharge: Discharge::Polynomial(GPRO_CURVE),
        charge: None,
        min_mv: GPRO_MIN_MV,
    }
}

const MODELS: &[Model] = &[
    gpro(0x0aa7, "Logitech G PRO Wireless Headset"),
    gpro(0x0aaa, "Logitech PRO X Wireless"),
    Model {
        discharge: Discharge::Table(PRO_X_DISCHARGE),
        charge: Some(Charge {
            rising: PRO_X_CHARGE_RISING,
            topping: PRO_X_CHARGE_TOPPING,
        }),
        ..gpro(0x0aba, "Logitech PRO X Wireless")
    },
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
    /// The voltage has reached the top of the rising stage during this
    /// charge, so it is read along the topping stage from now on.
    topping: bool,
}

/// One open receiver whose thread waits for reports the headset sends by itself.
struct Listener {
    path: CString,
    alive: Arc<AtomicBool>,
}

pub struct LogitechHeadsets {
    wake: Waker,
    listeners: Vec<Listener>,
    /// Set by a listener when the headset announced a battery change.
    announced: Arc<AtomicBool>,
    /// When the headsets were last asked, and what they answered.
    last: Option<(Instant, Vec<DeviceStatus>)>,
    /// Keyed by product ID.
    memory: HashMap<u16, Memory>,
}

impl LogitechHeadsets {
    pub fn new(wake: Waker) -> Self {
        Self {
            wake,
            listeners: Vec::new(),
            announced: Arc::new(AtomicBool::new(false)),
            last: None,
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
        let announced = self.announced.clone();
        thread::spawn(move || {
            listen(&dev, feature, &announced, &wake);
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
        let announced = self.announced.swap(false, Ordering::Relaxed);
        if let Some((at, statuses)) = &self.last
            && !too_old(at.elapsed(), announced)
        {
            return statuses.clone();
        }
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
        self.last = Some((Instant::now(), out.clone()));
        out
    }
}

/// Whether an answer of this age has to be replaced by asking again.
fn too_old(age: Duration, announced: bool) -> bool {
    announced || age >= MIN_REQUEST_GAP
}

/// Blocks on the receiver and calls `wake` when the headset announces a
/// battery change. Sends nothing. Returns when the receiver is unplugged.
fn listen(dev: &HidDevice, feature: u8, announced: &AtomicBool, wake: &Waker) {
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
        announced.store(true, Ordering::Relaxed);
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
        let mut percent = match model.discharge {
            Discharge::Polynomial(terms) => percent_from_voltage(terms, mv),
            Discharge::Table(points) => percent_from_table(points, mv),
        };
        match memory.charging {
            Some((cap, since)) if now.duration_since(since) < SETTLE => percent = percent.min(cap),
            _ => memory.charging = None,
        }
        memory.discharging = Some(percent);
        memory.topping = false;
        return Some(BatteryState::Level {
            percent,
            charging: false,
        });
    }

    let Some(charge) = &model.charge else {
        return Some(BatteryState::Charging);
    };
    // Which stage a voltage belongs to cannot be told from one reading: it
    // is known only after the top of the rising stage was seen. A charge
    // that was already topping up when the app started reads too low.
    let (top_mv, _) = charge.rising[charge.rising.len() - 1];
    memory.topping |= mv >= top_mv;
    let estimate = if memory.topping {
        percent_from_table(charge.topping, mv)
    } else {
        percent_from_table(charge.rising, mv)
    };
    // For the first minute on the charger the voltage is still climbing
    // (3910 mV at the moment of plugging in, 4140 mV a minute later), and a
    // battery on the charger does not lose charge: never go below the level
    // it had before.
    let percent = estimate.max(memory.discharging.unwrap_or(0));
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

    /// The PRO X Wireless with its own tables.
    fn pro_x() -> &'static Model {
        MODELS.iter().find(|m| m.product_id == 0x0aba).unwrap()
    }

    #[test]
    fn matches_what_g_hub_showed() {
        // Pairs of voltage and level read from G HUB's settings database.
        let now = Instant::now();
        let mut memory = Memory::default();
        let mut at = |mv| state_from_reply(pro_x(), mv, 0x01, &mut memory, now);
        assert_eq!(at(3930), level(82, false));
        assert_eq!(at(3692), level(26, false));
        let mut memory = Memory::default();
        let mut at = |mv| state_from_reply(pro_x(), mv, 0x03, &mut memory, now);
        assert_eq!(at(4233), level(82, true));
        assert_eq!(at(4249), level(84, true));
        assert_eq!(at(4260), level(85, true));
    }

    #[test]
    fn follows_a_charge_through_both_stages() {
        let mut memory = Memory::default();
        let start = Instant::now();
        let mut at = |secs: u64, mv, state| {
            let now = start + Duration::from_secs(secs);
            state_from_reply(pro_x(), mv, state, &mut memory, now)
        };
        assert_eq!(at(0, 3658, 0x01), level(19, false));
        // Just plugged in: the voltage has not climbed yet, keep the old level.
        assert_eq!(at(60, 3910, 0x03), level(19, true));
        assert_eq!(at(1200, 4087, 0x03), level(53, true));
        assert_eq!(at(4200, 4238, 0x03), level(82, true));
        // The top of the rising stage; from here the voltage falls.
        assert_eq!(at(6000, 4275, 0x03), level(87, true));
        assert_eq!(at(7000, 4241, 0x03), level(93, true));
        assert_eq!(at(9000, 4206, 0x03), level(100, true));
        // Unplugged: the voltage is still high for a moment.
        assert_eq!(at(9005, 4200, 0x01), level(100, false));
        // The next charge starts in the rising stage again.
        assert_eq!(at(20000, 3930, 0x01), level(82, false));
        assert_eq!(at(20100, 4238, 0x03), level(82, true));
    }

    #[test]
    fn unplugging_does_not_raise_the_level() {
        let mut memory = Memory::default();
        let start = Instant::now();
        let mut at = |secs: u64, mv, state| {
            let now = start + Duration::from_secs(secs);
            state_from_reply(pro_x(), mv, state, &mut memory, now)
        };
        assert_eq!(at(0, 4202, 0x03), level(77, true));
        // A second after the cable is pulled the voltage has barely moved.
        assert_eq!(at(1, 4200, 0x01), level(77, false));
        // Later the reading stands on its own.
        assert_eq!(at(600, 3975, 0x01), level(88, false));
    }

    #[test]
    fn a_model_without_a_charging_curve_shows_no_number() {
        let model = &MODELS[0];
        let mut memory = Memory::default();
        let now = Instant::now();
        assert_eq!(
            state_from_reply(model, 3692, 0x01, &mut memory, now),
            level(27, false)
        );
        assert_eq!(
            state_from_reply(model, 3978, 0x03, &mut memory, now),
            Some(BatteryState::Charging)
        );
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
    fn asks_again_only_when_due_or_announced() {
        assert!(!too_old(Duration::from_secs(2), false));
        assert!(too_old(Duration::from_secs(2), true));
        assert!(too_old(Duration::from_secs(30), false));
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
