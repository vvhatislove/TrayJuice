//! Xbox controllers (and other XInput gamepads) through the XInput API.
//!
//! Windows only tells four battery levels for these: empty, low, medium and
//! full. They are shown as rough percentages, and the level word is part of
//! the device name so the tooltip does not pretend to be exact.
//!
//! API reference: `XInputGetBatteryInformation`,
//! <https://learn.microsoft.com/windows/win32/api/xinput/nf-xinput-xinputgetbatteryinformation>

use hidapi::HidApi;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::UI::Input::XboxController::*;

use crate::device::{BatterySource, BatteryState, DeviceKind, DeviceStatus};

/// Maps an XInput battery level to `(word, rough percent)`.
fn describe(level: u8) -> Option<(&'static str, u8)> {
    match level {
        BATTERY_LEVEL_FULL => Some(("full", 100)),
        BATTERY_LEVEL_MEDIUM => Some(("medium", 60)),
        BATTERY_LEVEL_LOW => Some(("low", 25)),
        BATTERY_LEVEL_EMPTY => Some(("empty", 5)),
        _ => None,
    }
}

/// `true` for battery types that carry a level. A controller on a cable
/// reports `BATTERY_TYPE_WIRED` and no usable level, so it is not shown.
fn has_battery(battery_type: u8) -> bool {
    matches!(battery_type, BATTERY_TYPE_ALKALINE | BATTERY_TYPE_NIMH)
}

pub struct XboxControllers;

impl BatterySource for XboxControllers {
    fn poll(&mut self, _api: &HidApi) -> Vec<DeviceStatus> {
        let mut out = Vec::new();
        for slot in 0..XUSER_MAX_COUNT {
            let mut info = XINPUT_BATTERY_INFORMATION {
                BatteryType: 0,
                BatteryLevel: 0,
            };
            let status =
                unsafe { XInputGetBatteryInformation(slot, BATTERY_DEVTYPE_GAMEPAD, &mut info) };
            crate::log!(
                "xbox: slot {slot} status {status} type {} level {}",
                info.BatteryType,
                info.BatteryLevel
            );
            if status != ERROR_SUCCESS || !has_battery(info.BatteryType) {
                continue;
            }
            let Some((word, percent)) = describe(info.BatteryLevel) else {
                continue;
            };
            out.push(DeviceStatus {
                name: format!("Xbox controller {} (battery {word})", slot + 1),
                kind: DeviceKind::Gamepad,
                state: BatteryState::Level {
                    percent,
                    charging: false,
                },
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_levels_and_types() {
        assert_eq!(describe(BATTERY_LEVEL_FULL), Some(("full", 100)));
        assert_eq!(describe(BATTERY_LEVEL_EMPTY), Some(("empty", 5)));
        assert_eq!(describe(9), None);
        assert!(has_battery(BATTERY_TYPE_NIMH));
        assert!(!has_battery(BATTERY_TYPE_WIRED));
        assert!(!has_battery(BATTERY_TYPE_DISCONNECTED));
    }
}
