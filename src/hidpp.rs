//! Logitech mice and keyboards that speak HID++ 2.0 behind a Unifying, Bolt
//! or Lightspeed receiver.
//!
//! Nothing in this module has been run on hardware: no such receiver was at
//! hand. Every protocol fact below is taken from a source, and what is only
//! assumed is marked as such.
//!
//! From Solaar, <https://github.com/pwr-Solaar/Solaar> (`lib/logitech_receiver`):
//!
//! - Receiver product IDs (`base_usb.py`).
//! - A request is `<report> <device> <feature index> <function | software ID>`
//!   followed by parameters; the reply repeats those four bytes. Devices on
//!   a receiver are numbered from 1 (`base.py`).
//! - Feature index 0 is the root. Its function 0 takes a feature ID and
//!   returns that feature's index, 0 when the device does not have it
//!   (`hidpp20.py`, `FeaturesArray`).
//! - `0x1004` unified battery, function `0x10`: percent, a level bit mask
//!   (8 full, 4 good, 2 low, 1 critical) used when the percent is 0, and a
//!   status byte. `0x1000` battery status, function `0x00`: percent (0 for
//!   unknown), the next level, and the same status byte: 0 discharging, 1 to
//!   4 on the charger, 5 and 6 battery faults (`hidpp20.py`, `common.py`).
//! - `0x0005` device name: function `0x00` gives the length, `0x10` a piece
//!   of the name from an offset, `0x20` the kind of device (`hidpp20.py`,
//!   `hidpp20_constants.py`).
//! - Errors. A short report `10 <device> 8f <the two request bytes> <code>`
//!   comes for a device that cannot be asked: code `01` from a device that
//!   only speaks HID++ 1.0, `08` when nothing is paired at that number,
//!   `09` and `04` when the device cannot be reached. A HID++ 2.0 device
//!   refuses with `<device> ff <the two request bytes> <code>` (`base.py`,
//!   `hidpp10_constants.py`).
//!
//! From the Linux kernel, `drivers/hid/hid-logitech-dj.c` (`hidpp_descriptor`):
//! short reports (ID `0x10`, 7 bytes) are usage 1 and long reports (ID
//! `0x11`, 20 bytes) usage 2 of usage page `0xff00`. Windows makes a separate
//! device of each such collection, so a request written as a long report can
//! be answered on the other one by a short error report. Both are read.
//!
//! Assumed, not taken from a source:
//!
//! - That the receiver answers by itself, without waking anything, for a
//!   number that is empty or unreachable.
//! - That half a second is enough for a device to answer.
//! - That a receiver takes HID++ 2.0 requests as long reports. Solaar does
//!   send them that way, but not on Windows.
//!
//! Only features that give a percentage are read. A device that reports a
//! voltage (`0x1001`), or one that only speaks HID++ 1.0, is left out.
//! Nothing is asked of the receiver itself and nothing is written to its
//! registers: Solaar's way of listing paired devices sets one.

use std::time::{Duration, Instant};

use hidapi::{DeviceInfo, HidApi, HidDevice};

use crate::device::{BatterySource, BatteryState, DeviceKind, DeviceStatus};

const VENDOR_LOGITECH: u16 = 0x046d;
/// Unifying `c52b` and `c532`, Bolt `c548`, and the Lightspeed receivers.
const RECEIVERS: &[u16] = &[
    0xc52b, 0xc532, 0xc548, 0xc539, 0xc53a, 0xc53d, 0xc53f, 0xc541, 0xc545, 0xc547, 0xc54d,
];
const USAGE_PAGE: u16 = 0xff00;
const USAGE_SHORT: u16 = 0x0001;
const USAGE_LONG: u16 = 0x0002;

const SHORT: u8 = 0x10;
const LONG: u8 = 0x11;
const LONG_LEN: usize = 20;
/// Low half of the function byte. It tells our replies from those to other
/// software on the same receiver (Solaar and G HUB use `0b`) and from
/// reports a device sends by itself, which carry 0.
const SW_ID: u8 = 0x0d;

const ERROR_10: u8 = 0x8f;
const ERROR_20: u8 = 0xff;
const ERR_ONLY_HIDPP_10: u8 = 0x01;
const ERR_NOT_PAIRED: u8 = 0x08;

const FEATURE_NAME: u16 = 0x0005;
const FEATURE_BATTERY_STATUS: u16 = 0x1000;
const FEATURE_UNIFIED_BATTERY: u16 = 0x1004;

/// A Bolt receiver pairs up to six devices (Solaar). The same numbers are
/// asked on the other receivers, where an empty one gives error `08`.
const SLOTS: u8 = 6;
const REPLY_TIMEOUT: Duration = Duration::from_millis(500);
/// How long one read waits before the other collection gets its turn.
const READ_SLICE_MS: i32 = 5;
/// A poll that comes sooner than this after the last one gets the same
/// answer again: other backends ask for extra polls, and a request wakes
/// the device's radio.
const MIN_REQUEST_GAP: Duration = Duration::from_secs(30);
/// The gap while no device has answered yet, so that one shows up soon
/// after its receiver is plugged in or it is switched on. Until then only
/// numbers with nothing reachable behind them are asked.
const LOOK_AGAIN: Duration = Duration::from_secs(5);
/// Longest device name that is read, in bytes.
const MAX_NAME: usize = 48;

/// What came back for a request.
#[derive(Debug, PartialEq, Eq)]
enum Reply {
    /// The bytes after the four header bytes.
    Data([u8; 16]),
    /// The request was refused before it reached a HID++ 2.0 device; the
    /// code says why.
    Refused(u8),
    /// The device itself answered with an error.
    Failed,
}

/// Where the level of a device is read, with the feature's index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Battery {
    Unified(u8),
    Status(u8),
}

/// The last thing shown for a device, to ride out one unanswered request.
#[derive(Default)]
struct Shown {
    state: Option<BatteryState>,
    missed: bool,
}

enum Reading {
    State(BatteryState),
    /// The receiver said the device cannot be reached.
    Unreachable,
    /// No usable answer.
    Silence,
}

struct Known {
    name: String,
    kind: DeviceKind,
    battery: Battery,
    shown: Shown,
}

enum Slot {
    Known(Known),
    /// Speaks HID++ 1.0 only, or has no feature that gives a percentage.
    /// Not asked again while the receiver stays plugged in.
    Unsupported,
}

/// What asking an unknown number turned up.
enum Found {
    Device(Known),
    Unsupported,
    /// Empty, unreachable or silent: asked again at the next poll.
    Nothing,
}

/// The two collections of one receiver.
struct Io {
    long: HidDevice,
    short: HidDevice,
}

#[derive(Default)]
pub struct LogitechReceivers {
    /// When the receivers were last asked, which ones, and what came of it.
    last: Option<(Instant, Vec<String>, Vec<DeviceStatus>)>,
    /// Keyed by receiver (see [`interface_key`]) and device number.
    slots: Vec<((String, u8), Slot)>,
}

impl LogitechReceivers {
    pub fn new() -> Self {
        Self::default()
    }

    fn poll_receiver(&mut self, key: &str, io: &Io, out: &mut Vec<DeviceStatus>) {
        for number in 1..=SLOTS {
            let at = self
                .slots
                .iter()
                .position(|((k, n), _)| k == key && *n == number);
            let Some(at) = at else {
                match discover(io, number) {
                    Found::Device(mut known) => {
                        let state = known.shown.after(read_battery(io, number, known.battery).1);
                        crate::log!("hidpp: found {} at {number}: {state:?}", known.name);
                        out.push(status(&known, state));
                        self.slots
                            .push(((key.to_owned(), number), Slot::Known(known)));
                    }
                    Found::Unsupported => {
                        crate::log!("hidpp: nothing to read from device {number}");
                        self.slots
                            .push(((key.to_owned(), number), Slot::Unsupported));
                    }
                    Found::Nothing => {}
                }
                continue;
            };
            let Slot::Known(known) = &mut self.slots[at].1 else {
                continue;
            };
            let (gone, reading) = read_battery(io, number, known.battery);
            if gone {
                // Unpaired since the last poll.
                self.slots.remove(at);
                continue;
            }
            let state = known.shown.after(reading);
            crate::log!("hidpp: {} at {number}: {state:?}", known.name);
            out.push(status(known, state));
        }
    }
}

impl BatterySource for LogitechReceivers {
    fn poll(&mut self, api: &HidApi) -> Vec<DeviceStatus> {
        // The short and the long collection of each receiver, in the order
        // Windows lists them, so the devices keep their order on screen.
        let mut receivers: Vec<(&str, Option<&DeviceInfo>, Option<&DeviceInfo>)> = Vec::new();
        for info in api.device_list() {
            if info.vendor_id() != VENDOR_LOGITECH
                || !RECEIVERS.contains(&info.product_id())
                || info.usage_page() != USAGE_PAGE
            {
                continue;
            }
            let Some(key) = info.path().to_str().ok().and_then(interface_key) else {
                continue;
            };
            let at = receivers
                .iter()
                .position(|r| r.0 == key)
                .unwrap_or_else(|| {
                    receivers.push((key, None, None));
                    receivers.len() - 1
                });
            match info.usage() {
                USAGE_SHORT => receivers[at].1 = Some(info),
                USAGE_LONG => receivers[at].2 = Some(info),
                _ => {}
            }
        }

        // What was learnt about a receiver is dropped when it is unplugged.
        self.slots
            .retain(|((key, _), _)| receivers.iter().any(|r| r.0 == key));

        // A receiver that was just plugged in is asked at once.
        let keys: Vec<String> = receivers.iter().map(|r| r.0.to_owned()).collect();
        let known = self.slots.iter().any(|(_, s)| matches!(s, Slot::Known(_)));
        if let Some((at, asked, statuses)) = &self.last
            && *asked == keys
            && at.elapsed() < request_gap(known)
        {
            return statuses.clone();
        }

        let mut out = Vec::new();
        for (key, short, long) in receivers {
            let (Some(short), Some(long)) = (short, long) else {
                continue;
            };
            let (Ok(short), Ok(long)) = (short.open_device(api), long.open_device(api)) else {
                continue;
            };
            self.poll_receiver(key, &Io { long, short }, &mut out);
        }
        self.last = Some((Instant::now(), keys, out.clone()));
        out
    }
}

/// How long the last answer stands. `known` is set once a device has been
/// found.
fn request_gap(known: bool) -> Duration {
    if known { MIN_REQUEST_GAP } else { LOOK_AGAIN }
}

fn status(known: &Known, state: BatteryState) -> DeviceStatus {
    DeviceStatus {
        name: known.name.clone(),
        kind: known.kind,
        state,
    }
}

/// Names the receiver an interface path belongs to. Windows gives every
/// collection of one interface the same instance ID apart from its last
/// part: `…&col01#7&1a2b3c4d&0&0000#{…}` and `…&col02#7&1a2b3c4d&0&0001#{…}`
/// (seen on the receivers of this project's other backends).
fn interface_key(path: &str) -> Option<&str> {
    let instance = path.split('#').nth(2)?;
    instance.rsplit_once('&').map(|(parent, _)| parent)
}

impl Shown {
    /// What to show after a request. One request without an answer keeps
    /// the old state; the second in a row means the device is off.
    fn after(&mut self, reading: Reading) -> BatteryState {
        match (reading, self.state.clone()) {
            (Reading::State(state), _) => {
                self.missed = false;
                self.state = Some(state.clone());
                state
            }
            (Reading::Silence, Some(old)) if !self.missed => {
                self.missed = true;
                old
            }
            _ => {
                self.state = None;
                BatteryState::Offline
            }
        }
    }
}

/// Sends one request and waits for its reply. `None` when nothing that
/// matches arrives in time.
fn request(io: &Io, number: u8, index: u8, function: u8, params: &[u8]) -> Option<Reply> {
    let header = [index, function | SW_ID];
    let mut out = [0u8; LONG_LEN];
    out[0] = LONG;
    out[1] = number;
    out[2..4].copy_from_slice(&header);
    out[4..4 + params.len()].copy_from_slice(params);
    io.long.write(&out).ok()?;

    // Other software shares the receiver and the devices send reports of
    // their own: read until ours shows up or time runs out.
    let deadline = Instant::now() + REPLY_TIMEOUT;
    let mut buf = [0u8; LONG_LEN];
    while Instant::now() < deadline {
        for dev in [&io.long, &io.short] {
            let n = dev.read_timeout(&mut buf, READ_SLICE_MS).ok()?;
            if let Some(reply) = classify(&buf[..n], number, header) {
                return Some(reply);
            }
        }
    }
    None
}

/// Tells whether `report` answers the request with this device number and
/// header (feature index, function and software ID), and how.
fn classify(report: &[u8], number: u8, header: [u8; 2]) -> Option<Reply> {
    if report.len() < 7 || report[1] != number || !matches!(report[0], SHORT | LONG) {
        return None;
    }
    if report[3..5] == header {
        match report[2] {
            ERROR_10 if report[0] == SHORT => return Some(Reply::Refused(report[5])),
            ERROR_20 => return Some(Reply::Failed),
            _ => {}
        }
    }
    if report[2..4] != header {
        return None;
    }
    let mut data = [0u8; 16];
    let n = (report.len() - 4).min(data.len());
    data[..n].copy_from_slice(&report[4..4 + n]);
    Some(Reply::Data(data))
}

/// Asks the root feature for the index of `feature`. `Ok(None)` when the
/// device does not have it, `Err` when the device cannot be asked.
fn find_feature(io: &Io, number: u8, feature: u16) -> Result<Option<u8>, Found> {
    match request(io, number, 0, 0x00, &feature.to_be_bytes()) {
        Some(Reply::Data(data)) => Ok((data[0] != 0).then_some(data[0])),
        Some(Reply::Refused(ERR_ONLY_HIDPP_10)) => Err(Found::Unsupported),
        _ => Err(Found::Nothing),
    }
}

/// Finds out what is paired at `number` and where its level is read.
fn discover(io: &Io, number: u8) -> Found {
    let battery = match find_feature(io, number, FEATURE_UNIFIED_BATTERY) {
        Ok(Some(index)) => Battery::Unified(index),
        Ok(None) => match find_feature(io, number, FEATURE_BATTERY_STATUS) {
            Ok(Some(index)) => Battery::Status(index),
            Ok(None) => return Found::Unsupported,
            Err(found) => return found,
        },
        Err(found) => return found,
    };
    let Some((name, kind)) = identity(io, number) else {
        return Found::Nothing;
    };
    Found::Device(Known {
        name,
        kind,
        battery,
        shown: Shown::default(),
    })
}

/// Reads the name and the kind of a device. `None` when it stops answering
/// half way.
fn identity(io: &Io, number: u8) -> Option<(String, DeviceKind)> {
    let Some(index) = find_feature(io, number, FEATURE_NAME).ok()? else {
        return Some((display_name(&[]), DeviceKind::Other));
    };
    let data = |function, params: &[u8]| match request(io, number, index, function, params) {
        Some(Reply::Data(data)) => Some(data),
        _ => None,
    };
    let len = (data(0x00, &[])?[0] as usize).min(MAX_NAME);
    let mut name = Vec::with_capacity(len);
    while name.len() < len {
        let piece = data(0x10, &[name.len() as u8])?;
        let take = (len - name.len()).min(piece.len());
        name.extend_from_slice(&piece[..take]);
    }
    let kind = kind_from(data(0x20, &[])?[0]);
    Some((display_name(&name), kind))
}

/// Asks for the level. The flag is set when nothing is paired at that
/// number any more.
fn read_battery(io: &Io, number: u8, battery: Battery) -> (bool, Reading) {
    let reply = match battery {
        Battery::Unified(index) => request(io, number, index, 0x10, &[]),
        Battery::Status(index) => request(io, number, index, 0x00, &[]),
    };
    let reading = match (reply, battery) {
        (Some(Reply::Data(data)), Battery::Unified(_)) => Reading::State(unified_state(&data)),
        (Some(Reply::Data(data)), Battery::Status(_)) => match status_state(&data) {
            Some(state) => Reading::State(state),
            None => Reading::Unreachable,
        },
        (Some(Reply::Refused(ERR_NOT_PAIRED)), _) => return (true, Reading::Unreachable),
        (Some(Reply::Refused(_)), _) => Reading::Unreachable,
        (Some(Reply::Failed) | None, _) => Reading::Silence,
    };
    (false, reading)
}

/// Status byte of both battery features: 1 recharging, 2 almost full,
/// 3 full, 4 slow recharge. Solaar counts all four as on the charger.
fn on_charger(status: u8) -> bool {
    (1..=4).contains(&status)
}

/// Reply of the unified battery feature: percent, level bits, status.
fn unified_state(data: &[u8; 16]) -> BatteryState {
    let percent = match (data[0], data[1]) {
        // No percentage: the rough level, as numbers Solaar uses for them.
        (0, 8) => 90,
        (0, 4) => 50,
        (0, 2) => 20,
        (0, 1) => 5,
        (percent, _) => percent.min(100),
    };
    BatteryState::Level {
        percent,
        charging: on_charger(data[2]),
    }
}

/// Reply of the battery status feature: percent, next level, status. A
/// percent of 0 means the device does not know its level.
fn status_state(data: &[u8; 16]) -> Option<BatteryState> {
    let charging = on_charger(data[2]);
    match data[0] {
        0 if charging => Some(BatteryState::Charging),
        0 => None,
        percent => Some(BatteryState::Level {
            percent: percent.min(100),
            charging,
        }),
    }
}

/// Kind byte of the device name feature: 0 keyboard, 2 numpad, 3 mouse,
/// 5 trackball. The rest (remote, touchpad, presenter) has no glyph.
fn kind_from(kind: u8) -> DeviceKind {
    match kind {
        0x00 | 0x02 => DeviceKind::Keyboard,
        0x03 | 0x05 => DeviceKind::Mouse,
        _ => DeviceKind::Other,
    }
}

/// The name a device gives for itself, made fit for a tooltip.
fn display_name(raw: &[u8]) -> String {
    let name = String::from_utf8_lossy(raw);
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    let name = name.trim();
    if name.is_empty() {
        "Logitech device".to_string()
    } else if name.starts_with("Logi") {
        name.to_string()
    } else {
        format!("Logitech {name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(bytes: &[u8]) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[..bytes.len()].copy_from_slice(bytes);
        out
    }

    fn level(percent: u8, charging: bool) -> BatteryState {
        BatteryState::Level { percent, charging }
    }

    #[test]
    fn matches_replies_by_device_and_header() {
        // Unified battery at feature index 6, function 1, our software ID.
        let header = [0x06, 0x1d];
        let mut reply = [0u8; 20];
        reply[..7].copy_from_slice(&[0x11, 0x02, 0x06, 0x1d, 55, 4, 0]);
        assert_eq!(
            classify(&reply, 2, header),
            Some(Reply::Data(data(&[55, 4, 0])))
        );
        // Another device, another program's reply, a report sent unasked.
        assert_eq!(classify(&reply, 1, header), None);
        reply[3] = 0x1b;
        assert_eq!(classify(&reply, 2, header), None);
        reply[3] = 0x10;
        assert_eq!(classify(&reply, 2, header), None);
        // A short reply carries three bytes.
        assert_eq!(
            classify(&[0x10, 0x02, 0x06, 0x1d, 55, 4, 1], 2, header),
            Some(Reply::Data(data(&[55, 4, 1])))
        );
        assert_eq!(classify(&[0x11, 0x02, 0x06], 2, header), None);
        assert_eq!(
            classify(&[0x20, 0x02, 0x06, 0x1d, 0, 0, 0], 2, header),
            None
        );
    }

    #[test]
    fn tells_the_two_kinds_of_error() {
        let header = [0x00, 0x0d];
        // Nothing paired at number 3.
        let refused = [0x10, 0x03, 0x8f, 0x00, 0x0d, 0x08, 0x00];
        assert_eq!(classify(&refused, 3, header), Some(Reply::Refused(0x08)));
        assert_eq!(classify(&refused, 3, [0x00, 0x0b]), None);
        // The device itself refuses.
        let mut failed = [0u8; 20];
        failed[..6].copy_from_slice(&[0x11, 0x03, 0xff, 0x00, 0x0d, 0x02]);
        assert_eq!(classify(&failed, 3, header), Some(Reply::Failed));
        // A root reply is not taken for an error.
        let mut root = [0u8; 20];
        root[..5].copy_from_slice(&[0x11, 0x03, 0x00, 0x0d, 0x06]);
        assert_eq!(classify(&root, 3, header), Some(Reply::Data(data(&[6]))));
    }

    #[test]
    fn reads_the_unified_battery() {
        assert_eq!(unified_state(&data(&[55, 4, 0])), level(55, false));
        assert_eq!(unified_state(&data(&[80, 8, 1])), level(80, true));
        assert_eq!(unified_state(&data(&[100, 8, 3])), level(100, true));
        // Only the rough level is known.
        assert_eq!(unified_state(&data(&[0, 8, 0])), level(90, false));
        assert_eq!(unified_state(&data(&[0, 2, 0])), level(20, false));
        assert_eq!(unified_state(&data(&[0, 1, 4])), level(5, true));
        assert_eq!(unified_state(&data(&[0, 0, 0])), level(0, false));
        assert_eq!(unified_state(&data(&[130, 8, 0])), level(100, false));
        // A battery fault is not charging.
        assert_eq!(unified_state(&data(&[40, 4, 6])), level(40, false));
    }

    #[test]
    fn reads_the_battery_status() {
        assert_eq!(status_state(&data(&[70, 50, 0])), Some(level(70, false)));
        assert_eq!(status_state(&data(&[70, 50, 2])), Some(level(70, true)));
        assert_eq!(
            status_state(&data(&[0, 0, 1])),
            Some(BatteryState::Charging)
        );
        assert_eq!(status_state(&data(&[0, 0, 0])), None);
    }

    #[test]
    fn one_unanswered_request_keeps_the_level() {
        let mut shown = Shown::default();
        // Never heard: off at once.
        assert_eq!(shown.after(Reading::Silence), BatteryState::Offline);
        let good = level(64, false);
        assert_eq!(shown.after(Reading::State(good.clone())), good);
        assert_eq!(shown.after(Reading::Silence), good);
        assert_eq!(shown.after(Reading::Silence), BatteryState::Offline);
        // The receiver saying so is believed the first time.
        shown.after(Reading::State(good.clone()));
        assert_eq!(shown.after(Reading::Unreachable), BatteryState::Offline);
        // An answer in between clears the miss.
        shown.after(Reading::State(good.clone()));
        shown.after(Reading::Silence);
        shown.after(Reading::State(good.clone()));
        assert_eq!(shown.after(Reading::Silence), good);
    }

    #[test]
    fn asks_sooner_while_nothing_was_found() {
        assert_eq!(request_gap(true), Duration::from_secs(30));
        assert!(request_gap(false) < request_gap(true));
    }

    #[test]
    fn names_and_kinds() {
        assert_eq!(display_name(b"MX Master 3"), "Logitech MX Master 3");
        assert_eq!(display_name(b"Logi POP Keys\0"), "Logi POP Keys");
        assert_eq!(display_name(b""), "Logitech device");
        assert_eq!(display_name(b" \0"), "Logitech device");
        assert_eq!(kind_from(0x00), DeviceKind::Keyboard);
        assert_eq!(kind_from(0x03), DeviceKind::Mouse);
        assert_eq!(kind_from(0x05), DeviceKind::Mouse);
        assert_eq!(kind_from(0x06), DeviceKind::Other);
    }

    #[test]
    fn pairs_the_collections_of_one_receiver() {
        let short = r"\\?\hid#vid_046d&pid_c52b&mi_02&col01#7&1a2b3c4d&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let long = r"\\?\hid#vid_046d&pid_c52b&mi_02&col02#7&1a2b3c4d&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let other = r"\\?\hid#vid_046d&pid_c52b&mi_02&col02#7&99887766&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(interface_key(short), Some("7&1a2b3c4d&0"));
        assert_eq!(interface_key(short), interface_key(long));
        assert_ne!(interface_key(long), interface_key(other));
        assert_eq!(interface_key("no separators"), None);
    }
}
