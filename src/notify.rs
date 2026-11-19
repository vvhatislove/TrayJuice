//! Shows a notification on the app's tray icon (a balloon, which Windows 10
//! and 11 present as a toast). No second icon is added.
//!
//! The icon belongs to the `tray-icon` crate, which gives out its window
//! but not the number it registered the icon under. In version 0.24.2 that
//! number comes from a counter that starts at 1, so the first few numbers
//! are tried, and the one Windows accepts is kept. If a later version
//! numbers its icons differently, nothing is shown and `show` returns
//! `false`; the levels are still in the tooltip.

use std::ffi::c_void;

use windows_sys::Win32::UI::Shell::{
    NIF_INFO, NIIF_WARNING, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};

/// How many icon numbers are tried.
const IDS_TRIED: u32 = 8;

pub struct Notifier {
    /// Window of the tray icon.
    hwnd: *mut c_void,
    /// The icon's number, once found.
    id: Option<u32>,
}

impl Notifier {
    /// `hwnd` is `TrayIcon::window_handle()`.
    pub fn new(hwnd: *mut c_void) -> Self {
        Self { hwnd, id: None }
    }

    /// Shows a notification. Call on the thread that owns the tray icon.
    /// Returns `false` when Windows did not take it.
    pub fn show(&mut self, title: &str, text: &str) -> bool {
        let ids = match self.id {
            Some(id) => id..=id,
            None => 1..=IDS_TRIED,
        };
        for id in ids {
            let mut data = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: id,
                uFlags: NIF_INFO,
                dwInfoFlags: NIIF_WARNING,
                ..unsafe { std::mem::zeroed() }
            };
            copy_text(&mut data.szInfoTitle, title);
            copy_text(&mut data.szInfo, text);
            if unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) } != 0 {
                self.id = Some(id);
                return true;
            }
        }
        false
    }
}

/// Copies `text` into a fixed UTF-16 buffer, cut to fit with the
/// terminating zero. A surrogate pair is never cut in half.
fn copy_text(buf: &mut [u16], text: &str) {
    let mut len = 0;
    let mut units = [0u16; 2];
    for c in text.chars() {
        let encoded = c.encode_utf16(&mut units);
        if len + encoded.len() >= buf.len() {
            break;
        }
        buf[len..len + encoded.len()].copy_from_slice(encoded);
        len += encoded.len();
    }
    buf[len] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_text_to_the_buffer() {
        let mut buf = [0xffffu16; 6];
        copy_text(&mut buf, "abc");
        assert_eq!(buf[..4], [97, 98, 99, 0]);
        copy_text(&mut buf, "abcdefgh");
        assert_eq!(buf, [97, 98, 99, 100, 101, 0]);
        // The pair of a character outside the basic plane stays whole.
        copy_text(&mut buf, "abcd\u{1f50b}");
        assert_eq!(buf[..5], [97, 98, 99, 100, 0]);
        copy_text(&mut buf, "");
        assert_eq!(buf[0], 0);
    }
}
