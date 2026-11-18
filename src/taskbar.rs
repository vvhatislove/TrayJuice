//! Taskbar widget: device glyph + percentage drawn directly on the taskbar,
//! left of the notification area.
//!
//! Windows has no public API for this. The window is created as a layered
//! popup and re-parented into `Shell_TrayWnd`, the same approach tools such as
//! TrafficMonitor use. It can stop working after a Windows update, so the rest
//! of the app must keep working when [`Widget::update`] returns `false`.
//!
//! All `unsafe` Win32 code of the widget lives in this module.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::device::{BatteryState, DeviceKind, DeviceStatus};

const CLASS_NAME: &str = "TrayJuiceWidget";
const REPOSITION_TIMER: usize = 1;
const REPOSITION_MS: u32 = 1000;

type Rgb = (u8, u8, u8);
const WHITE: Rgb = (0xff, 0xff, 0xff);
const BLACK: Rgb = (0x1b, 0x1b, 0x1b);
const RED_ON_DARK: Rgb = (0xff, 0x6b, 0x60);
const RED_ON_LIGHT: Rgb = (0xc4, 0x2b, 0x1c);
const GREEN_ON_DARK: Rgb = (0x6c, 0xcb, 0x5f);
const GREEN_ON_LIGHT: Rgb = (0x0f, 0x7b, 0x0f);

/// Premultiplied BGRA pixels, top-down, ready for `UpdateLayeredWindow`.
pub struct Bitmap {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>,
    /// Horizontal extent `(left, right)` of each device, for hover tooltips.
    pub regions: Vec<(i32, i32)>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Glyph in the "Segoe Fluent Icons" font shipped with Windows 11.
fn glyph(kind: DeviceKind) -> char {
    match kind {
        DeviceKind::Headset => '\u{e7f6}',
        DeviceKind::Mouse => '\u{e962}',
        DeviceKind::Keyboard => '\u{e92e}',
        DeviceKind::Gamepad => '\u{e7fc}',
        DeviceKind::Other => '\u{e83f}',
    }
}

fn label(state: &BatteryState) -> String {
    match *state {
        BatteryState::Level { percent, .. } => format!("{percent}%"),
        BatteryState::Offline => "\u{2013}".into(),
    }
}

/// Text colour and opacity (0-255) for a state.
fn style(state: &BatteryState, light: bool) -> (Rgb, u32) {
    let normal = if light { BLACK } else { WHITE };
    match *state {
        BatteryState::Offline => (normal, 110),
        BatteryState::Level { charging: true, .. } => {
            (if light { GREEN_ON_LIGHT } else { GREEN_ON_DARK }, 255)
        }
        BatteryState::Level { percent, .. } if percent <= 20 => {
            (if light { RED_ON_LIGHT } else { RED_ON_DARK }, 255)
        }
        BatteryState::Level { .. } => (normal, 255),
    }
}

struct Run {
    text: Vec<u16>,
    icon: bool,
    x: i32,
    y: i32,
    width: i32,
    color: Rgb,
    opacity: u32,
}

/// Draws all devices in one row. `height` is the taskbar height in pixels,
/// `light` selects colours for the light taskbar theme.
pub fn render(statuses: &[DeviceStatus], height: i32, dpi: u32, light: bool) -> Bitmap {
    let px = |v: i32| v * dpi as i32 / 96;
    let (pad, icon_gap, device_gap) = (px(6), px(5), px(12));

    unsafe {
        let screen = GetDC(null_mut());
        let dc = CreateCompatibleDC(screen);
        ReleaseDC(null_mut(), screen);

        let font = |face: &str, size: i32| {
            CreateFontW(
                -size,
                0,
                0,
                0,
                FW_NORMAL as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                ANTIALIASED_QUALITY as u32,
                0,
                wide(face).as_ptr(),
            )
        };
        let icon_font = font("Segoe Fluent Icons", px(16));
        let text_font = font("Segoe UI", px(13));
        let old_font = SelectObject(dc, icon_font);

        // Lay the runs out left to right.
        let mut runs = Vec::new();
        let mut regions = Vec::new();
        let mut x = pad;
        for (i, status) in statuses.iter().enumerate() {
            if i > 0 {
                x += device_gap;
            }
            let device_left = x - device_gap / 2;
            let (color, opacity) = style(&status.state, light);
            for (text, icon) in [
                (glyph(status.kind).to_string(), true),
                (label(&status.state), false),
            ] {
                let text: Vec<u16> = text.encode_utf16().collect();
                SelectObject(dc, if icon { icon_font } else { text_font });
                let mut size = SIZE { cx: 0, cy: 0 };
                GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32, &mut size);
                runs.push(Run {
                    text,
                    icon,
                    x,
                    y: (height - size.cy) / 2,
                    width: size.cx,
                    color,
                    opacity,
                });
                x += size.cx + if icon { icon_gap } else { 0 };
            }
            regions.push((device_left, x + device_gap / 2));
        }
        let width = if statuses.is_empty() { 0 } else { x + pad };
        if width == 0 || height <= 0 {
            SelectObject(dc, old_font);
            DeleteObject(icon_font);
            DeleteObject(text_font);
            DeleteDC(dc);
            return Bitmap {
                width: 0,
                height: 0,
                pixels: Vec::new(),
                regions: Vec::new(),
            };
        }

        // GDI text has no alpha channel: draw white on black with greyscale
        // antialiasing and use the brightness as coverage.
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = width;
        bmi.bmiHeader.biHeight = -height;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        let mut bits: *mut core::ffi::c_void = null_mut();
        let dib = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let old_bitmap = SelectObject(dc, dib);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, 0x00ff_ffff);
        for run in &runs {
            SelectObject(dc, if run.icon { icon_font } else { text_font });
            TextOutW(dc, run.x, run.y, run.text.as_ptr(), run.text.len() as i32);
        }
        GdiFlush();

        let mut pixels =
            std::slice::from_raw_parts(bits as *const u32, (width * height) as usize).to_vec();
        for run in &runs {
            let (r, g, b) = run.color;
            for row in 0..height {
                for col in run.x.max(0)..(run.x + run.width).min(width) {
                    let p = &mut pixels[(row * width + col) as usize];
                    let coverage = (*p & 0xff).max((*p >> 8) & 0xff).max((*p >> 16) & 0xff);
                    let a = coverage * run.opacity / 255;
                    let mul = |c: u8| c as u32 * a / 255;
                    *p = (a << 24) | (mul(r) << 16) | (mul(g) << 8) | mul(b);
                }
            }
        }

        // A layered window only receives the mouse where alpha is non-zero.
        // Keep the whole strip barely opaque so hovering anywhere over a
        // device shows its tooltip, not only on the strokes of the text.
        for p in &mut pixels {
            if *p >> 24 == 0 {
                *p = 0x0100_0000;
            }
        }

        SelectObject(dc, old_bitmap);
        SelectObject(dc, old_font);
        DeleteObject(dib);
        DeleteObject(icon_font);
        DeleteObject(text_font);
        DeleteDC(dc);
        Bitmap {
            width,
            height,
            pixels,
            regions,
        }
    }
}

/// `true` when the taskbar uses the light theme.
fn light_taskbar() -> bool {
    let mut value: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize").as_ptr(),
            wide("SystemUsesLightTheme").as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    status == ERROR_SUCCESS && value != 0
}

fn taskbar() -> HWND {
    unsafe { FindWindowW(wide("Shell_TrayWnd").as_ptr(), null()) }
}

fn rect_of(hwnd: HWND) -> RECT {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe { GetWindowRect(hwnd, &mut rect) };
    rect
}

/// Keeps the widget glued to the left edge of the notification area, whose
/// width changes whenever a tray icon appears or disappears.
fn reposition(hwnd: HWND) {
    unsafe {
        let parent = GetAncestor(hwnd, GA_PARENT);
        let notify = FindWindowExW(parent, null_mut(), wide("TrayNotifyWnd").as_ptr(), null());
        if parent.is_null() || notify.is_null() {
            return;
        }
        let (bar, tray, own) = (rect_of(parent), rect_of(notify), rect_of(hwnd));
        let x = tray.left - (own.right - own.left) - bar.left;
        if own.left - bar.left != x || own.top != bar.top {
            SetWindowPos(hwnd, HWND_TOP, x, 0, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // This window lives inside Explorer's taskbar: never block here.
    if msg == WM_TIMER && wp == REPOSITION_TIMER {
        reposition(hwnd);
        return 0;
    }
    match msg {
        WM_CLOSE => crate::log!("widget {hwnd:?}: WM_CLOSE"),
        WM_DESTROY => crate::log!("widget {hwnd:?}: WM_DESTROY"),
        WM_SHOWWINDOW => crate::log!("widget {hwnd:?}: WM_SHOWWINDOW show={wp} reason={lp}"),
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

pub struct Widget {
    hwnd: HWND,
    /// Tooltip control owned by `hwnd`; rebuilt whenever the content changes.
    tooltip: HWND,
    /// What is currently drawn, to skip redundant redraws.
    shown: Option<(Vec<DeviceStatus>, bool)>,
}

impl Default for Widget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget {
    pub fn new() -> Self {
        Widget {
            hwnd: null_mut(),
            tooltip: null_mut(),
            shown: None,
        }
    }

    /// Redraws the widget. Returns `false` when it cannot be shown (no
    /// taskbar, or embedding failed) so the caller can fall back to the tray.
    pub fn update(&mut self, statuses: &[DeviceStatus]) -> bool {
        let bar = taskbar();
        if bar.is_null() {
            crate::log!("widget: taskbar not found");
            self.hide();
            return false;
        }
        unsafe {
            // Explorer restarts destroy the taskbar together with our window.
            let embedded = !self.hwnd.is_null()
                && IsWindow(self.hwnd) != 0
                && GetAncestor(self.hwnd, GA_PARENT) == bar;
            if !embedded {
                self.hide();
                self.hwnd = create_window(bar);
                crate::log!("widget: created {:?} in taskbar {bar:?}", self.hwnd);
                if self.hwnd.is_null() {
                    return false;
                }
            }

            let light = light_taskbar();
            if embedded
                && self
                    .shown
                    .as_ref()
                    .is_some_and(|(s, l)| s == statuses && *l == light)
            {
                return true;
            }

            let bar_rect = rect_of(bar);
            let bitmap = render(
                statuses,
                bar_rect.bottom - bar_rect.top,
                GetDpiForWindow(bar),
                light,
            );
            if bitmap.width == 0 {
                ShowWindow(self.hwnd, SW_HIDE);
                self.shown = None;
                return true;
            }
            if !present(self.hwnd, &bitmap) {
                crate::log!(
                    "widget: UpdateLayeredWindow failed, error {}",
                    GetLastError()
                );
                self.hide();
                return false;
            }
            reposition(self.hwnd);
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);

            if !self.tooltip.is_null() {
                DestroyWindow(self.tooltip);
            }
            self.tooltip = create_tooltip(self.hwnd, statuses, &bitmap, light);
            self.shown = Some((statuses.to_vec(), light));
        }
        true
    }

    /// `true` while the embedded window exists (Explorer may destroy it).
    pub fn is_alive(&self) -> bool {
        !self.hwnd.is_null() && unsafe { IsWindow(self.hwnd) } != 0
    }

    pub fn hide(&mut self) {
        if !self.hwnd.is_null() {
            // Destroying the widget also destroys the tooltip it owns.
            unsafe { DestroyWindow(self.hwnd) };
            self.hwnd = null_mut();
        }
        self.tooltip = null_mut();
        self.shown = None;
    }
}

impl Drop for Widget {
    fn drop(&mut self) {
        self.hide();
    }
}

unsafe fn create_window(bar: HWND) -> HWND {
    unsafe {
        let instance = GetModuleHandleW(null());
        let class = wide(CLASS_NAME);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc); // fails harmlessly when already registered

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            null(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return hwnd;
        }
        SetParent(hwnd, bar);
        if GetAncestor(hwnd, GA_PARENT) != bar {
            DestroyWindow(hwnd);
            return null_mut();
        }
        SetTimer(hwnd, REPOSITION_TIMER, REPOSITION_MS, None);
        hwnd
    }
}

/// Creates a tooltip control with one hover area per device.
unsafe fn create_tooltip(
    owner: HWND,
    statuses: &[DeviceStatus],
    bitmap: &Bitmap,
    light: bool,
) -> HWND {
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES,
        };
        InitCommonControlsEx(&icc);

        let instance = GetModuleHandleW(null());
        let tooltip = CreateWindowExW(
            WS_EX_TOPMOST,
            wide("tooltips_class32").as_ptr(),
            null(),
            WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            owner,
            null_mut(),
            instance,
            null(),
        );
        if tooltip.is_null() {
            crate::log!("widget: tooltip creation failed, error {}", GetLastError());
            return tooltip;
        }
        if !light {
            SetWindowTheme(tooltip, wide("DarkMode_Explorer").as_ptr(), null());
        }
        for (i, (status, &(left, right))) in statuses.iter().zip(&bitmap.regions).enumerate() {
            let mut text = wide(&status.summary());
            let mut tool: TTTOOLINFOW = std::mem::zeroed();
            tool.cbSize = size_of::<TTTOOLINFOW>() as u32;
            tool.uFlags = TTF_SUBCLASS;
            tool.hwnd = owner;
            tool.uId = i;
            tool.rect = RECT {
                left,
                top: 0,
                right,
                bottom: bitmap.height,
            };
            tool.hinst = instance;
            tool.lpszText = text.as_mut_ptr();
            SendMessageW(tooltip, TTM_ADDTOOLW, 0, &tool as *const _ as LPARAM);
        }
        tooltip
    }
}

/// Pushes the bitmap to the layered window, resizing it to fit.
unsafe fn present(hwnd: HWND, bitmap: &Bitmap) -> bool {
    unsafe {
        let screen = GetDC(null_mut());
        let dc = CreateCompatibleDC(screen);
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = bitmap.width;
        bmi.bmiHeader.biHeight = -bitmap.height;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        let mut bits: *mut core::ffi::c_void = null_mut();
        let dib = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let mut ok = false;
        if !dib.is_null() {
            std::ptr::copy_nonoverlapping(
                bitmap.pixels.as_ptr(),
                bits as *mut u32,
                bitmap.pixels.len(),
            );
            let old = SelectObject(dc, dib);
            let size = SIZE {
                cx: bitmap.width,
                cy: bitmap.height,
            };
            let origin = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            ok = UpdateLayeredWindow(
                hwnd,
                screen,
                null(),
                &size,
                dc,
                &origin,
                0,
                &blend,
                ULW_ALPHA,
            ) != 0;
            SelectObject(dc, old);
            DeleteObject(dib);
        }
        DeleteDC(dc);
        ReleaseDC(null_mut(), screen);
        ok
    }
}
