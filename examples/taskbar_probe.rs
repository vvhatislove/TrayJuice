//! Diagnostic: checks whether a window embedded into the taskbar is visible
//! on this Windows build. Shows a magenta block left of the notification area
//! for 20 seconds, then removes it and exits.
//!
//! `cargo run --example taskbar_probe -- [popup|child|topmost]`

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const W: i32 = 120;
const H: i32 = 32;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match m {
            WM_TIMER => {
                DestroyWindow(h);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(h, m, wp, lp),
        }
    }
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "popup".into());
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let taskbar = FindWindowW(wide("Shell_TrayWnd").as_ptr(), null());
        let notify = FindWindowExW(taskbar, null_mut(), wide("TrayNotifyWnd").as_ptr(), null());
        let (mut tb, mut nt): (RECT, RECT) = (std::mem::zeroed(), std::mem::zeroed());
        GetWindowRect(taskbar, &mut tb);
        GetWindowRect(notify, &mut nt);
        println!(
            "taskbar {taskbar:?} {:?}",
            (tb.left, tb.top, tb.right, tb.bottom)
        );
        println!(
            "notify  {notify:?} {:?}",
            (nt.left, nt.top, nt.right, nt.bottom)
        );

        let sx = nt.left - W - 8;
        let sy = tb.top + (tb.bottom - tb.top - H) / 2;
        let (rx, ry) = (sx - tb.left, sy - tb.top);

        let hinst = GetModuleHandleW(null());
        let class = wide("TrayJuiceProbe");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);

        let base_ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let hwnd = match mode.as_str() {
            "child" => CreateWindowExW(
                WS_EX_LAYERED,
                class.as_ptr(),
                null(),
                WS_CHILD | WS_VISIBLE,
                rx,
                ry,
                W,
                H,
                taskbar,
                null_mut(),
                hinst,
                null(),
            ),
            "topmost" => CreateWindowExW(
                base_ex | WS_EX_TOPMOST,
                class.as_ptr(),
                null(),
                WS_POPUP,
                sx,
                sy,
                W,
                H,
                null_mut(),
                null_mut(),
                hinst,
                null(),
            ),
            _ => {
                let h = CreateWindowExW(
                    base_ex,
                    class.as_ptr(),
                    null(),
                    WS_POPUP,
                    sx,
                    sy,
                    W,
                    H,
                    null_mut(),
                    null_mut(),
                    hinst,
                    null(),
                );
                let prev = SetParent(h, taskbar);
                println!("SetParent prev={prev:?} err={}", GetLastError());
                SetWindowPos(h, HWND_TOP, rx, ry, W, H, SWP_NOACTIVATE);
                h
            }
        };
        println!("mode={mode} hwnd={hwnd:?} err={}", GetLastError());
        if hwnd.is_null() {
            return;
        }

        // Opaque magenta, premultiplied BGRA.
        let screen = GetDC(null_mut());
        let mem = CreateCompatibleDC(screen);
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = W;
        bmi.bmiHeader.biHeight = -H;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        let mut bits: *mut core::ffi::c_void = null_mut();
        let dib = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (W * H) as usize);
        px.fill(0xffff00ff);
        let old = SelectObject(mem, dib);
        let size = SIZE { cx: W, cy: H };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let ok = UpdateLayeredWindow(hwnd, screen, null(), &size, mem, &src, 0, &blend, ULW_ALPHA);
        println!("UpdateLayeredWindow ok={ok} err={}", GetLastError());
        SelectObject(mem, old);
        DeleteObject(dib);
        DeleteDC(mem);
        ReleaseDC(null_mut(), screen);

        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(hwnd, 1, 20_000, None);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
    }
}
