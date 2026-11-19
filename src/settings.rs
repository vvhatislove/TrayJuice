//! Choices made in the tray menu, kept as DWORD values under
//! `HKCU\Software\TrayJuice`. A value that is missing means "on", so a
//! first start writes nothing.
//!
//! Reading happens once at start, before any window exists. Writing is
//! done by the caller on its own thread, never on the main one.

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD, RegGetValueW, RegSetKeyValueW,
};

const KEY: &str = r"Software\TrayJuice";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// The "Show in taskbar" menu item.
    ShowInTaskbar,
    /// The "Low battery alerts" menu item.
    LowBatteryAlerts,
}

impl Setting {
    fn value_name(self) -> &'static str {
        match self {
            Setting::ShowInTaskbar => "ShowInTaskbar",
            Setting::LowBatteryAlerts => "LowBatteryAlerts",
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// What a stored value means: only an explicit 0 turns a setting off.
fn is_on(stored: Option<u32>) -> bool {
    stored != Some(0)
}

fn stored(setting: Setting) -> Option<u32> {
    let (key, value) = (wide(KEY), wide(setting.value_name()));
    let mut data = 0u32;
    let mut bytes = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut data).cast(),
            &mut bytes,
        )
    };
    (status == ERROR_SUCCESS).then_some(data)
}

/// Whether the setting is on. On when it was never changed.
pub fn get(setting: Setting) -> bool {
    is_on(stored(setting))
}

/// Stores the setting, creating the key when needed. Returns `false` when
/// the registry refused.
pub fn set(setting: Setting, on: bool) -> bool {
    let (key, value) = (wide(KEY), wide(setting.value_name()));
    let data = u32::from(on);
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            REG_DWORD,
            (&raw const data).cast(),
            size_of::<u32>() as u32,
        )
    };
    status == ERROR_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_value_means_on() {
        assert!(is_on(None));
        assert!(is_on(Some(1)));
        assert!(!is_on(Some(0)));
    }

    #[test]
    fn settings_have_their_own_values() {
        assert_ne!(
            Setting::ShowInTaskbar.value_name(),
            Setting::LowBatteryAlerts.value_name()
        );
    }
}
