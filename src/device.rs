//! Vendor-independent battery model. Every backend (Logitech HID++, and later
//! Bluetooth, other dongles…) reports its devices as [`DeviceStatus`].

use std::sync::Arc;

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

/// Asks for an extra poll right away. A backend calls it from its own
/// thread when a device announces a change by itself.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// All backends compiled into this build.
pub fn all_sources(wake: Waker) -> Vec<Box<dyn BatterySource>> {
    vec![
        Box::new(crate::logitech::LogitechHeadsets::new(wake)),
        Box::new(crate::delux::DeluxMice::default()),
        Box::new(crate::xbox::XboxControllers),
    ]
}

/// Evaluates a discharge polynomial: `terms[i]` multiplies `mv^i`.
/// The result is rounded and clamped to 0..=100.
pub fn percent_from_voltage(terms: &[f64], mv: u16) -> u8 {
    let v = mv as f64;
    // Horner's scheme, highest power first.
    let percent = terms.iter().rev().fold(0.0, |acc, term| acc * v + term);
    percent.round().clamp(0.0, 100.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_and_clamps() {
        // percent = mv / 10 - 300
        let line = [-300.0, 0.1];
        assert_eq!(percent_from_voltage(&line, 3500), 50);
        assert_eq!(percent_from_voltage(&line, 3754), 75);
        assert_eq!(percent_from_voltage(&line, 2000), 0);
        assert_eq!(percent_from_voltage(&line, 4500), 100);
    }
}
