//! Taskbar widget: device glyph + percentage drawn directly on the taskbar,
//! left of the notification area.
//!
//! Windows has no public API for this. The window is created as a layered
//! popup and re-parented into `Shell_TrayWnd`, the same approach tools such as
//! TrafficMonitor use. It can stop working after a Windows update, so the rest
//! of the app must keep working when [`Widget::update`] returns `false`.
//!
//! The window ignores the mouse (`WS_EX_TRANSPARENT`), so clicks reach the
//! taskbar underneath. Hover tooltips are driven by a timer that compares the
//! cursor position with the device areas.
//!
//! All `unsafe` Win32 code of the widget lives in this module.

use std::cell::RefCell;
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
const TICK_TIMER: usize = 1;
/// How often the widget re-checks its position and what the cursor hovers.
const TICK_MS: u32 = 400;
const FLUENT_ICONS: &str = "Segoe Fluent Icons";
const MDL2_ICONS: &str = "Segoe MDL2 Assets";

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

impl Bitmap {
    fn empty() -> Self {
        Bitmap {
            width: 0,
            height: 0,
            pixels: Vec::new(),
            regions: Vec::new(),
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// `true` when GDI really selected `face` and did not substitute another
/// font. "Segoe Fluent Icons" does not exist on Windows 10.
unsafe fn has_face(dc: HDC, font: HFONT, face: &str) -> bool {
    unsafe {
        let old = SelectObject(dc, font);
        let mut name = [0u16; 64];
        let len = GetTextFaceW(dc, name.len() as i32, name.as_mut_ptr());
        SelectObject(dc, old);
        let len = (len.max(1) - 1) as usize; // the count includes the NUL
        String::from_utf16_lossy(&name[..len]).eq_ignore_ascii_case(face)
    }
}

/// The gamepad glyph of "Segoe Fluent Icons" is a plain pill; the one in the
/// older "Segoe MDL2 Assets" font has grips and reads as a controller.
fn uses_mdl2(kind: DeviceKind) -> bool {
    kind == DeviceKind::Gamepad
}

/// Glyph code point; the same in "Segoe Fluent Icons" and "Segoe MDL2 Assets".
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
    font: HFONT,
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
        let icon_font = font(FLUENT_ICONS, px(16));
        let mdl2_font = font(MDL2_ICONS, px(16));
        let text_font = font("Segoe UI", px(13));
        // Windows 10 has no Fluent icon font; MDL2 has the same code points.
        let fluent = has_face(dc, icon_font, FLUENT_ICONS);
        let old_font = SelectObject(dc, icon_font);
        let delete_fonts = || {
            DeleteObject(icon_font);
            DeleteObject(mdl2_font);
            DeleteObject(text_font);
        };

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
                let font = match (icon, fluent && !uses_mdl2(status.kind)) {
                    (true, true) => icon_font,
                    (true, false) => mdl2_font,
                    (false, _) => text_font,
                };
                SelectObject(dc, font);
                let mut size = SIZE { cx: 0, cy: 0 };
                GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32, &mut size);
                runs.push(Run {
                    text,
                    font,
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
            delete_fonts();
            DeleteDC(dc);
            return Bitmap::empty();
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
        if dib.is_null() || bits.is_null() {
            SelectObject(dc, old_font);
            delete_fonts();
            DeleteDC(dc);
            return Bitmap::empty();
        }
        let old_bitmap = SelectObject(dc, dib);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, 0x00ff_ffff);
        for run in &runs {
            SelectObject(dc, run.font);
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

        // Pixels between the runs still hold what GDI left there: clear them.
        let covered = |col: i32| runs.iter().any(|r| (r.x..r.x + r.width).contains(&col));
        for col in (0..width).filter(|&col| !covered(col)) {
            for row in 0..height {
                pixels[(row * width + col) as usize] = 0;
            }
        }

        SelectObject(dc, old_bitmap);
        SelectObject(dc, old_font);
        DeleteObject(dib);
        delete_fonts();
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
///
/// Returns `false` when the notification area cannot be found. The widget
/// must not stay visible then: its default position is the far left of the
/// taskbar, on top of the Start button.
fn reposition(hwnd: HWND) -> bool {
    unsafe {
        let parent = GetAncestor(hwnd, GA_PARENT);
        if parent.is_null() {
            return false;
        }
        let notify = FindWindowExW(parent, null_mut(), wide("TrayNotifyWnd").as_ptr(), null());
        if notify.is_null() {
            return false;
        }
        let (bar, tray, own) = (rect_of(parent), rect_of(notify), rect_of(hwnd));
        let x = tray.left - (own.right - own.left) - bar.left;
        if own.left - bar.left != x || own.top != bar.top {
            SetWindowPos(hwnd, HWND_TOP, x, 0, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        }
        true
    }
}

/// Hover tooltip state. Only the main thread touches the widget, so a
/// thread-local is enough to share it with the window procedure.
struct Hover {
    /// Unowned tracking tooltip, null when there is none.
    tooltip: HWND,
    /// Horizontal extent of each device inside the widget.
    regions: Vec<(i32, i32)>,
    /// NUL-terminated tooltip text of each device.
    texts: Vec<Vec<u16>>,
    /// Device whose tooltip is showing right now.
    active: Option<usize>,
}

thread_local! {
    static HOVER: RefCell<Hover> = const {
        RefCell::new(Hover {
            tooltip: null_mut(),
            regions: Vec::new(),
            texts: Vec::new(),
            active: None,
        })
    };
}

/// The single tool of the tracking tooltip. The size leaves out the last
/// field so the struct is also accepted without Common Controls v6.
fn tool_info(widget: HWND) -> TTTOOLINFOW {
    let mut tool: TTTOOLINFOW = unsafe { std::mem::zeroed() };
    tool.cbSize = (size_of::<TTTOOLINFOW>() - size_of::<*mut core::ffi::c_void>()) as u32;
    tool.uFlags = TTF_TRACK | TTF_ABSOLUTE;
    tool.hwnd = widget;
    tool
}

fn drop_tooltip() {
    HOVER.with_borrow_mut(|hover| {
        if !hover.tooltip.is_null() {
            unsafe { DestroyWindow(hover.tooltip) };
            hover.tooltip = null_mut();
        }
        hover.regions.clear();
        hover.texts.clear();
        hover.active = None;
    });
}

/// Index of the device under the cursor, if the cursor is over the visible
/// widget.
fn hovered_device(widget: HWND, regions: &[(i32, i32)]) -> Option<usize> {
    unsafe {
        if IsWindowVisible(widget) == 0 {
            return None;
        }
        let mut cursor = POINT { x: 0, y: 0 };
        GetCursorPos(&mut cursor);
        let own = rect_of(widget);
        if !(own.left..own.right).contains(&cursor.x) || !(own.top..own.bottom).contains(&cursor.y)
        {
            return None;
        }
        // A fullscreen app can cover the taskbar while the cursor is over
        // this spot. Only show the tooltip when the taskbar is what is there.
        let bar = GetAncestor(widget, GA_PARENT);
        let under = WindowFromPoint(cursor);
        if under.is_null() || GetAncestor(under, GA_ROOT) != bar {
            return None;
        }
        let x = cursor.x - own.left;
        regions
            .iter()
            .position(|&(left, right)| (left..right).contains(&x))
    }
}

/// Shows, moves or hides the tooltip to match what the cursor hovers.
fn update_hover(widget: HWND) {
    HOVER.with_borrow_mut(|hover| {
        if hover.tooltip.is_null() {
            return;
        }
        let now = hovered_device(widget, &hover.regions);
        if now == hover.active {
            return;
        }
        hover.active = now;
        let mut tool = tool_info(widget);
        let tool_ptr = &mut tool as *mut TTTOOLINFOW;
        unsafe {
            let Some(text) = now.and_then(|i| hover.texts.get_mut(i)) else {
                SendMessageW(hover.tooltip, TTM_TRACKACTIVATE, 0, tool_ptr as LPARAM);
                return;
            };
            (*tool_ptr).lpszText = text.as_mut_ptr();
            SendMessageW(hover.tooltip, TTM_UPDATETIPTEXTW, 0, tool_ptr as LPARAM);
            SendMessageW(hover.tooltip, TTM_TRACKACTIVATE, 1, tool_ptr as LPARAM);
            place_tooltip(widget, hover.tooltip);
        }
    });
}

/// Puts the tooltip just above the taskbar, centred on the cursor.
///
/// The tooltip is not owned by the taskbar, so the taskbar would cover it if
/// it were shown over it.
fn place_tooltip(widget: HWND, tooltip: HWND) {
    unsafe {
        let bar = GetAncestor(widget, GA_PARENT);
        if bar.is_null() {
            return;
        }
        let (bar_rect, tip) = (rect_of(bar), rect_of(tooltip));
        let (width, height) = (tip.right - tip.left, tip.bottom - tip.top);
        let gap = 8 * GetDpiForWindow(bar) as i32 / 96;
        let mut cursor = POINT { x: 0, y: 0 };
        GetCursorPos(&mut cursor);
        let x = (cursor.x - width / 2)
            .clamp(bar_rect.left, (bar_rect.right - width).max(bar_rect.left));
        let y = bar_rect.top - height - gap;
        SetWindowPos(
            tooltip,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // This window lives inside Explorer's taskbar: never block here.
    if msg == WM_TIMER && wp == TICK_TIMER {
        if reposition(hwnd) {
            update_hover(hwnd);
        } else {
            crate::log!("widget {hwnd:?}: notification area not found, removing");
            unsafe { DestroyWindow(hwnd) };
        }
        return 0;
    }
    match msg {
        WM_CLOSE => crate::log!("widget {hwnd:?}: WM_CLOSE"),
        WM_DESTROY => {
            crate::log!("widget {hwnd:?}: WM_DESTROY");
            // Also reached when Explorer destroys the taskbar: the unowned
            // tooltip would otherwise stay behind.
            drop_tooltip();
        }
        WM_SHOWWINDOW => crate::log!("widget {hwnd:?}: WM_SHOWWINDOW show={wp} reason={lp}"),
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// What the widget currently shows; a redraw is skipped while it is unchanged.
#[derive(PartialEq)]
struct Shown {
    statuses: Vec<DeviceStatus>,
    light: bool,
    height: i32,
    dpi: u32,
}

pub struct Widget {
    hwnd: HWND,
    shown: Option<Shown>,
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

            let bar_rect = rect_of(bar);
            let wanted = Shown {
                statuses: statuses.to_vec(),
                light: light_taskbar(),
                height: bar_rect.bottom - bar_rect.top,
                dpi: GetDpiForWindow(bar),
            };
            if embedded && self.shown.as_ref() == Some(&wanted) {
                return true;
            }

            let bitmap = render(statuses, wanted.height, wanted.dpi, wanted.light);
            if bitmap.width == 0 {
                ShowWindow(self.hwnd, SW_HIDE);
                drop_tooltip();
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
            if !reposition(self.hwnd) {
                crate::log!("widget: notification area not found");
                self.hide();
                return false;
            }
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);

            drop_tooltip();
            let tooltip = create_tooltip(self.hwnd, wanted.light);
            HOVER.with_borrow_mut(|hover| {
                hover.tooltip = tooltip;
                hover.regions = bitmap.regions.clone();
                hover.texts = statuses.iter().map(|s| wide(&s.summary())).collect();
            });
            self.shown = Some(wanted);
        }
        true
    }

    /// `true` while the embedded window exists (Explorer may destroy it).
    pub fn is_alive(&self) -> bool {
        !self.hwnd.is_null() && unsafe { IsWindow(self.hwnd) } != 0
    }

    pub fn hide(&mut self) {
        if !self.hwnd.is_null() {
            unsafe { DestroyWindow(self.hwnd) };
            self.hwnd = null_mut();
        }
        drop_tooltip();
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
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc); // fails harmlessly when already registered

        let hwnd = CreateWindowExW(
            // WS_EX_TRANSPARENT: the mouse goes to the taskbar underneath.
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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
        SetTimer(hwnd, TICK_TIMER, TICK_MS, None);
        hwnd
    }
}

/// Creates the tracking tooltip that [`update_hover`] shows and hides.
///
/// The tooltip is deliberately created without an owner. An owner inside the
/// taskbar resolves to `Shell_TrayWnd` itself, and a topmost popup owned by
/// the taskbar keeps the taskbar above fullscreen apps.
unsafe fn create_tooltip(widget: HWND, light: bool) -> HWND {
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES,
        };
        InitCommonControlsEx(&icc);

        let instance = GetModuleHandleW(null());
        let tooltip = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            wide("tooltips_class32").as_ptr(),
            null(),
            WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            null_mut(),
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
        let mut text = wide("");
        let mut tool = tool_info(widget);
        tool.lpszText = text.as_mut_ptr();
        SendMessageW(tooltip, TTM_ADDTOOLW, 0, &tool as *const _ as LPARAM);
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
