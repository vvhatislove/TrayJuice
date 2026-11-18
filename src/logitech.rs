//! Logitech wireless headsets that speak HID++ over their USB receiver.
//!
//! Protocol details (request bytes, response layout, discharge polynomial)
//! follow the HeadsetControl project: <https://github.com/Sapd/HeadsetControl>
//! (`src/devices/logitech_gpro.c` in release 3.0.0 for the polynomial).

use std::time::{Duration, Instant};

use hidapi::{HidApi, HidDevice};

use crate::device::{BatterySource, BatteryState, DeviceKind, DeviceStatus, percent_from_voltage};

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
                kind: DeviceKind::Headset,
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
        crate::log!(
            "logitech: {} reports {mv} mV, state {:#04x}",
            model.name,
            buf[6]
        );
        if mv < model.min_mv {
            return None;
        }
        return Some(BatteryState::Level {
            percent: percent_from_voltage(model.curve, mv),
            charging: buf[6] == 0x03,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpro_curve_matches_known_points() {
        assert_eq!(percent_from_voltage(GPRO_CURVE, 3692), 27);
        assert_eq!(percent_from_voltage(GPRO_CURVE, 3900), 73);
        assert_eq!(percent_from_voltage(GPRO_CURVE, 4200), 100);
        assert_eq!(percent_from_voltage(GPRO_CURVE, 3300), 0);
    }
}
