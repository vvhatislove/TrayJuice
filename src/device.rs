//! Vendor-independent battery model. Every backend (Logitech HID++, and later
//! Bluetooth, other dongles…) reports its devices as [`DeviceStatus`].

use std::sync::Arc;

use hidapi::HidApi;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatteryState {
    /// Device is reachable and reported a level.
    Level { percent: u8, charging: bool },
    /// Powered through a cable, and no level is available meanwhile.
    Charging,
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
            BatteryState::Charging => format!("{}: charging", self.name),
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
        Box::new(crate::logitech::LogitechHeadsets::new(wake.clone())),
        Box::new(crate::delux::DeluxMice::new(wake)),
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

/// Reads a percentage off a table of `(millivolts, percent)` points sorted
/// by voltage, with straight lines between them. Outside the table the
/// nearest end applies. The result is rounded and clamped to 0..=100.
pub fn percent_from_table(points: &[(u16, f32)], mv: u16) -> u8 {
    let Some(&(_, mut percent)) = points.first() else {
        return 0;
    };
    for pair in points.windows(2) {
        let ((v0, p0), (v1, p1)) = (pair[0], pair[1]);
        if mv <= v0 {
            break;
        }
        percent = if mv >= v1 {
            p1
        } else {
            p0 + (p1 - p0) * (mv - v0) as f32 / (v1 - v0) as f32
        };
    }
    percent.round().clamp(0.0, 100.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tables_in_both_directions() {
        let rising = [(3000, 0.0), (3500, 50.0), (4000, 100.0)];
        assert_eq!(percent_from_table(&rising, 2000), 0);
        assert_eq!(percent_from_table(&rising, 3250), 25);
        assert_eq!(percent_from_table(&rising, 3500), 50);
        assert_eq!(percent_from_table(&rising, 3900), 90);
        assert_eq!(percent_from_table(&rising, 4500), 100);
        // A level that falls as the voltage rises works the same way.
        let falling = [(4200, 100.0), (4280, 80.0)];
        assert_eq!(percent_from_table(&falling, 4100), 100);
        assert_eq!(percent_from_table(&falling, 4240), 90);
        assert_eq!(percent_from_table(&falling, 4300), 80);
        assert_eq!(percent_from_table(&[], 4000), 0);
    }

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
