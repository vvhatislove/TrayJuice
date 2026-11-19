//! Lets the installer close a running copy without the user's help.
//!
//! The app creates the event `Local\TrayJuice.Quit` and a thread waits on
//! it. `installer/trayjuice.iss` sets the event right before it replaces
//! the files (and before an uninstall); the app then leaves the way "Quit
//! TrayJuice" does. The thread blocks in the wait and does no periodic work.

use std::thread;

use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};

/// Name of the event. The installer opens it as `TrayJuice.Quit`, which is
/// the same object within one user session.
const EVENT: &str = r"Local\TrayJuice.Quit";

/// Calls `on_quit` once when the installer asks this copy to leave.
/// Returns `false` when the event could not be created; the installer then
/// asks the user to quit the app by hand.
pub fn listen(on_quit: impl FnOnce() + Send + 'static) -> bool {
    listen_on(EVENT, on_quit)
}

fn listen_on(name: &str, on_quit: impl FnOnce() + Send + 'static) -> bool {
    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // Auto-reset, not signalled. Never closed: it lives as long as the app.
    let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
    if event.is_null() {
        return false;
    }
    // A raw handle is not `Send`; its value as a number is.
    let event = event as usize;
    thread::spawn(move || {
        if unsafe { WaitForSingleObject(event as _, INFINITE) } == WAIT_OBJECT_0 {
            on_quit();
        }
    });
    true
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};

    use super::*;

    #[test]
    fn the_event_is_what_the_installer_opens() {
        // The installer leaves the prefix out.
        assert_eq!(EVENT.strip_prefix(r"Local\"), Some("TrayJuice.Quit"));
    }

    #[test]
    fn setting_the_event_by_its_short_name_calls_back() {
        // A name of its own, so the test never closes a running TrayJuice.
        let short = format!("TrayJuice.QuitTest.{}", std::process::id());
        let (tx, rx) = mpsc::channel();
        assert!(listen_on(&format!(r"Local\{short}"), move || {
            let _ = tx.send(());
        }));
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());

        // What the installer does: open without the prefix, set, close.
        let name: Vec<u16> = short.encode_utf16().chain([0]).collect();
        let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
        assert!(!event.is_null());
        unsafe {
            assert_ne!(SetEvent(event), 0);
            CloseHandle(event);
        }
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }
}
