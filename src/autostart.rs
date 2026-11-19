//! "Start with Windows": a value under the current user's `Run` key that
//! holds the quoted path of this executable. No administrator rights are
//! needed, and nothing else is written anywhere.

use std::path::Path;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
};

use crate::settings::{read_string, wide};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "TrayJuice";

/// What Windows runs at sign-in: the path in quotes, so spaces are safe.
fn command_for(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// The command stored under the `Run` key, if any.
fn stored_command() -> Option<String> {
    read_string(RUN_KEY, VALUE)
}

/// `true` when Windows is set to start this very executable at sign-in. A
/// leftover entry that points at another copy counts as off.
pub fn is_enabled() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    stored_command().is_some_and(|stored| stored.eq_ignore_ascii_case(&command_for(&exe)))
}

/// Adds or removes the entry. Returns `false` when the registry refused.
pub fn set(enabled: bool) -> bool {
    let (key, value) = (wide(RUN_KEY), wide(VALUE));
    if !enabled {
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
        // Already gone is as good as removed.
        return status == ERROR_SUCCESS || stored_command().is_none();
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let command = wide(&command_for(&exe));
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            REG_SZ,
            command.as_ptr().cast(),
            (command.len() * 2) as u32,
        )
    };
    status == ERROR_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_the_path() {
        assert_eq!(
            command_for(Path::new(r"C:\Program Files\TrayJuice\trayjuice.exe")),
            r#""C:\Program Files\TrayJuice\trayjuice.exe""#
        );
    }
}
