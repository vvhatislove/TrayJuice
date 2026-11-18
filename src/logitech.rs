//! Logitech wireless headsets that speak HID++ over their USB receiver.
//!
//! Protocol details (request bytes, response layout, discharge curve) follow
//! the HeadsetControl project: <https://github.com/Sapd/HeadsetControl>.

use std::time::{Duration, Instant};

use hidapi::{HidApi, HidDevice};

use crate::device::{percent_from_voltage, BatterySource, BatteryState, DeviceStatus};

const VENDOR_LOGITECH: u16 = 0x046d;

const HIDPP_LONG: u8 = 0x11;
const HIDPP_LONG_LEN: usize = 20;
const HIDPP_RECEIVER: u8 = 0xff;
/// Sub-ID of an HID++ 2.0 error reply.
const HIDPP_ERROR: u8 = 0xff;

const REPLY_TIMEOUT: Duration = Duration::from_millis(1000);

struct Model {
    product_id: u16,
    name: &'static str,
    usage_page: u16,
    usage: u16,
    /// Feature index + function of the "battery voltage" request.
    battery_cmd: [u8; 2],
    /// `(millivolts, percent)`, descending.
    curve: &'static [(u16, u8)],
}

const GPRO_CURVE: &[(u16, u8)] = &[
    (4150, 100),
    (3830, 50),
    (3780, 30),
    (3740, 20),
    (3670, 5),
    (3320, 0),
];

const fn gpro(product_id: u16, name: &'static str) -> Model {
    Model {
        product_id,
        name,
        usage_page: 0xff43,
        usage: 0x0202,
        battery_cmd: [0x06, 0x0d],
        curve: GPRO_CURVE,
    }
}

const MODELS: &[Model] = &[
    gpro(0x0aa7, "Logitech G PRO Wireless Headset"),
    gpro(0x0aaa, "Logitech PRO X Wireless"),
    gpro(0x0aba, "Logitech PRO X Wireless"),
    gpro(0x0afb, "Logitech PRO X 2"),
    gpro(0x0afc, "Logitech PRO X 2"),
];

pub struct LogitechHeadsets;

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
            let state = info
                .open_device(api)
                .ok()
                .and_then(|dev| read_battery(&dev, model))
                .unwrap_or(BatteryState::Offline);
            out.push(DeviceStatus {
                name: model.name.to_string(),
                state,
            });
        }
        out
    }
}

/// Returns `None` when the headset does not answer (off / out of range).
fn read_battery(dev: &HidDevice, model: &Model) -> Option<BatteryState> {
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
        if buf[2] == HIDPP_ERROR && buf[3] == model.battery_cmd[0] {
            return None;
        }
        if buf[2..4] != model.battery_cmd {
            continue;
        }
        let mv = u16::from_be_bytes([buf[4], buf[5]]);
        let lowest = model.curve[model.curve.len() - 1].0;
        if mv < lowest {
            return None;
        }
        return Some(BatteryState::Level {
            percent: percent_from_voltage(model.curve, mv),
            charging: buf[6] == 0x03,
        });
    }
}
