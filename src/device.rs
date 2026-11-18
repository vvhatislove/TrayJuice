//! Vendor-independent battery model. Every backend (Logitech HID++, and later
//! Bluetooth, other dongles…) reports its devices as [`DeviceStatus`].

use hidapi::HidApi;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatteryState {
    /// Device is reachable and reported a level.
    Level { percent: u8, charging: bool },
    /// Receiver is plugged in, but the device itself is off or out of range.
    Offline,
}

/// What the device is; picks the glyph shown next to the percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Headset,
    Mouse,
    Keyboard,
    Gamepad,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceStatus {
    pub name: String,
    pub kind: DeviceKind,
    pub state: BatteryState,
}

impl DeviceStatus {
    /// One line for tooltips, e.g. `Logitech PRO X Wireless: 30%`.
    pub fn summary(&self) -> String {
        match self.state {
            BatteryState::Level {
                percent,
                charging: true,
            } => format!("{}: {percent}% (charging)", self.name),
            BatteryState::Level { percent, .. } => format!("{}: {percent}%", self.name),
            BatteryState::Offline => format!("{}: off", self.name),
        }
    }
}

/// A family of devices that knows how to find its members and read their battery.
pub trait BatterySource: Send {
    /// Poll every supported device currently plugged in.
    /// Devices whose receiver is absent are simply not returned.
    fn poll(&mut self, api: &HidApi) -> Vec<DeviceStatus>;
}

/// All backends compiled into this build.
pub fn all_sources() -> Vec<Box<dyn BatterySource>> {
    vec![
        Box::new(crate::logitech::LogitechHeadsets),
        Box::new(crate::delux::DeluxMice::default()),
    ]
}

/// Piecewise-linear interpolation over a discharge curve.
/// `curve` is `(millivolts, percent)` sorted by descending voltage.
pub fn percent_from_voltage(curve: &[(u16, u8)], mv: u16) -> u8 {
    let (top_mv, top_pct) = curve[0];
    if mv >= top_mv {
        return top_pct;
    }
    for pair in curve.windows(2) {
        let (hi_mv, hi_pct) = pair[0];
        let (lo_mv, lo_pct) = pair[1];
        if mv >= lo_mv {
            let span = (hi_mv - lo_mv) as f32;
            let t = (mv - lo_mv) as f32 / span;
            return (lo_pct as f32 + t * (hi_pct - lo_pct) as f32).round() as u8;
        }
    }
    curve[curve.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURVE: &[(u16, u8)] = &[(4150, 100), (3830, 50), (3320, 0)];

    #[test]
    fn clamps_and_interpolates() {
        assert_eq!(percent_from_voltage(CURVE, 4300), 100);
        assert_eq!(percent_from_voltage(CURVE, 4150), 100);
        assert_eq!(percent_from_voltage(CURVE, 3990), 75);
        assert_eq!(percent_from_voltage(CURVE, 3830), 50);
        assert_eq!(percent_from_voltage(CURVE, 3000), 0);
    }
}
