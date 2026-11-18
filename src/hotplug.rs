//! Asks for a poll when a device appears or disappears, so a controller
//! that was just switched on shows up without waiting for the next regular
//! poll.
//!
//! Uses `CM_Register_Notification` for device interface arrival and removal
//! of every interface class. Windows calls back on one of its own threads;
//! nothing here runs periodically.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_NOTIFY_ACTION, CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
    CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL, CM_NOTIFY_EVENT_DATA, CM_NOTIFY_FILTER,
    CM_NOTIFY_FILTER_FLAG_ALL_INTERFACE_CLASSES, CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
    CM_Register_Notification, CR_SUCCESS, HCMNOTIFICATION,
};

use crate::device::Waker;

/// A device is not ready the instant its interface arrives: an Xbox
/// controller had no battery reading 2 seconds after it, and had one 11
/// seconds after it. So more polls follow, these many milliseconds apart.
const LOOK_AGAIN_MS: [u64; 5] = [1000, 1000, 2000, 4000, 4000];

struct Watch {
    wake: Waker,
    /// One device brings several interfaces; they share one round of polls.
    busy: AtomicBool,
}

/// Starts watching for the lifetime of the process. Returns `false` when
/// Windows refused the registration; the regular poll still works then.
pub fn watch(wake: Waker) -> bool {
    // Never freed: the registration lives as long as the process.
    let watch: &'static Watch = Box::leak(Box::new(Watch {
        wake,
        busy: AtomicBool::new(false),
    }));
    let filter = CM_NOTIFY_FILTER {
        cbSize: size_of::<CM_NOTIFY_FILTER>() as u32,
        Flags: CM_NOTIFY_FILTER_FLAG_ALL_INTERFACE_CLASSES,
        FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
        ..Default::default()
    };
    let mut handle: HCMNOTIFICATION = std::ptr::null_mut();
    let result = unsafe {
        CM_Register_Notification(
            &filter,
            watch as *const Watch as *const core::ffi::c_void,
            Some(on_event),
            &mut handle,
        )
    };
    result == CR_SUCCESS
}

unsafe extern "system" fn on_event(
    _: HCMNOTIFICATION,
    context: *const core::ffi::c_void,
    action: CM_NOTIFY_ACTION,
    _: *const CM_NOTIFY_EVENT_DATA,
    _: u32,
) -> u32 {
    if is_plug_event(action) {
        // SAFETY: `context` is the leaked `Watch` passed at registration.
        let watch: &'static Watch = unsafe { &*(context as *const Watch) };
        if !watch.busy.swap(true, Ordering::Relaxed) {
            // The callback must return quickly, so the waiting happens elsewhere.
            thread::spawn(move || {
                (watch.wake)();
                for pause in LOOK_AGAIN_MS {
                    thread::sleep(Duration::from_millis(pause));
                    (watch.wake)();
                }
                watch.busy.store(false, Ordering::Relaxed);
            });
        }
    }
    0 // ERROR_SUCCESS
}

fn is_plug_event(action: CM_NOTIFY_ACTION) -> bool {
    action == CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL
        || action == CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_arrival_and_removal_count() {
        assert!(is_plug_event(CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL));
        assert!(is_plug_event(CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL));
        // 2 is "query remove", 7 is a custom event of a device handle.
        assert!(!is_plug_event(2));
        assert!(!is_plug_event(7));
    }
}
