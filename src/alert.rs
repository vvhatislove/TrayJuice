//! Decides when a low battery is worth a notification. Pure logic: the
//! notification itself is shown by `src/notify.rs`.
//!
//! A device is announced when it drops to 20 % and again at 10 %, once
//! each. It can be announced again after it was charged, or after its level
//! rose to 25 %: levels worked out from a voltage wander by a point or two,
//! and one that hovers around 20 % must not alert over and over.

use std::collections::HashMap;

use crate::device::{BatteryState, DeviceStatus};

/// Levels at or below which a device is announced, highest first.
const THRESHOLDS: [u8; 2] = [20, 10];
/// A level this high forgets that the device was announced.
const CLEAR_FROM: u8 = 25;

pub const TITLE: &str = "Low battery";

#[derive(Default)]
pub struct LowBattery {
    /// The threshold each device was last announced at, by device name.
    announced: HashMap<String, u8>,
}

impl LowBattery {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the result of a poll and returns the text to show, one line
    /// per device that has just become low, or `None`.
    pub fn check(&mut self, statuses: &[DeviceStatus]) -> Option<String> {
        let mut lines = Vec::new();
        for status in statuses {
            let name = plain_name(&status.name);
            let percent = match status.state {
                BatteryState::Level {
                    percent,
                    charging: false,
                } => percent,
                // On the charger: the next discharge starts afresh.
                BatteryState::Level { charging: true, .. } | BatteryState::Charging => {
                    self.announced.remove(name);
                    continue;
                }
                // Off or asleep says nothing about the level: a mouse that
                // wakes up at 15 % was announced already.
                BatteryState::Offline => continue,
            };
            let Some(reached) = THRESHOLDS.into_iter().rfind(|t| percent <= *t) else {
                if percent >= CLEAR_FROM {
                    self.announced.remove(name);
                }
                continue;
            };
            if self.announced.get(name).is_some_and(|at| *at <= reached) {
                continue;
            }
            self.announced.insert(name.to_string(), reached);
            lines.push(format!("{name}: {percent}%"));
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}

/// The device name without the note some backends add in brackets
/// ("heard 3 min ago", "battery low"), which changes between polls.
fn plain_name(name: &str) -> &str {
    name.split(" (").next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceKind;

    fn device(name: &str, state: BatteryState) -> DeviceStatus {
        DeviceStatus {
            name: name.to_string(),
            kind: DeviceKind::Mouse,
            state,
        }
    }

    fn at(percent: u8) -> BatteryState {
        BatteryState::Level {
            percent,
            charging: false,
        }
    }

    fn charging(percent: u8) -> BatteryState {
        BatteryState::Level {
            percent,
            charging: true,
        }
    }

    #[test]
    fn announces_each_threshold_once() {
        let mut low = LowBattery::new();
        let mut check = |percent| low.check(&[device("Mouse", at(percent))]);
        assert_eq!(check(21), None);
        assert_eq!(check(20), Some("Mouse: 20%".to_string()));
        assert_eq!(check(19), None);
        assert_eq!(check(11), None);
        assert_eq!(check(10), Some("Mouse: 10%".to_string()));
        assert_eq!(check(3), None);
    }

    #[test]
    fn a_level_that_hovers_does_not_repeat() {
        let mut low = LowBattery::new();
        let mut check = |percent| low.check(&[device("Mouse", at(percent))]);
        assert!(check(20).is_some());
        assert_eq!(check(22), None);
        assert_eq!(check(20), None);
        assert_eq!(check(24), None);
        assert_eq!(check(19), None);
        // Well above: the next drop counts again.
        assert_eq!(check(25), None);
        assert!(check(20).is_some());
    }

    #[test]
    fn a_first_reading_far_down_gives_one_alert() {
        let mut low = LowBattery::new();
        assert_eq!(
            low.check(&[device("Mouse", at(5))]),
            Some("Mouse: 5%".to_string())
        );
        assert_eq!(low.check(&[device("Mouse", at(5))]), None);
    }

    #[test]
    fn charging_is_quiet_and_starts_afresh() {
        let mut low = LowBattery::new();
        assert_eq!(low.check(&[device("Mouse", charging(8))]), None);
        assert!(low.check(&[device("Mouse", at(18))]).is_some());
        assert_eq!(low.check(&[device("Mouse", charging(19))]), None);
        assert_eq!(low.check(&[device("Mouse", BatteryState::Charging)]), None);
        // Pulled off the charger while still low: said again.
        assert!(low.check(&[device("Mouse", at(19))]).is_some());
    }

    #[test]
    fn going_off_and_back_does_not_repeat() {
        let mut low = LowBattery::new();
        assert!(low.check(&[device("Mouse", at(15))]).is_some());
        assert_eq!(low.check(&[device("Mouse", BatteryState::Offline)]), None);
        assert_eq!(low.check(&[]), None);
        assert_eq!(low.check(&[device("Mouse", at(15))]), None);
    }

    #[test]
    fn lists_every_device_and_ignores_the_note_in_the_name() {
        let mut low = LowBattery::new();
        let statuses = [
            device("Headset", at(18)),
            device("Mouse (heard 2 min ago)", at(9)),
            device("Keyboard", at(80)),
        ];
        assert_eq!(
            low.check(&statuses),
            Some("Headset: 18%\nMouse: 9%".to_string())
        );
        let later = [device("Mouse (heard 3 min ago)", at(9))];
        assert_eq!(low.check(&later), None);
    }
}
