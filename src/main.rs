#![windows_subsystem = "windows"]

use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use hidapi::HidApi;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use trayjuice::device::{DeviceStatus, Waker, all_sources};
use trayjuice::icon;
use trayjuice::taskbar::Widget;
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

const POLL_INTERVAL: Duration = Duration::from_secs(60);
const MENU_REFRESH: &str = "refresh";
const MENU_QUIT: &str = "quit";
const MENU_WIDGET: &str = "widget";

enum UserEvent {
    Statuses(Vec<DeviceStatus>),
    Menu(MenuEvent),
}

struct App {
    /// The only tray icon: carries the menu and a tooltip with every level.
    tray: TrayIcon,
    widget: Widget,
    widget_item: CheckMenuItem,
    /// Latest poll result, kept so the widget can be rebuilt when toggled.
    statuses: Vec<DeviceStatus>,
    refresh: mpsc::Sender<()>,
}

impl App {
    fn show(&mut self) {
        let wanted = self.widget_item.is_checked();
        trayjuice::log!("show: widget wanted={wanted}, statuses={:?}", self.statuses);

        let tooltip = if self.statuses.is_empty() {
            "TrayJuice: no supported devices".to_string()
        } else {
            let lines: Vec<String> = self.statuses.iter().map(DeviceStatus::summary).collect();
            lines.join("\n")
        };
        let _ = self.tray.set_tooltip(Some(tooltip));

        // When the widget is off or cannot be embedded, the levels stay
        // available in the tray icon's tooltip.
        if !wanted || !self.widget.update(&self.statuses) {
            self.widget.hide();
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        if let UserEvent::Menu(e) = &event {
            trayjuice::log!("menu: {:?}", e.id);
        }
        match event {
            UserEvent::Statuses(statuses) => {
                self.statuses = statuses;
                self.show();
            }
            // The check mark has already been toggled by the menu itself.
            UserEvent::Menu(e) if e.id.as_ref() == MENU_WIDGET => self.show(),
            UserEvent::Menu(e) if e.id.as_ref() == MENU_QUIT => {
                self.widget.hide();
                let _ = self.tray.set_visible(false);
                event_loop.exit();
            }
            UserEvent::Menu(e) if e.id.as_ref() == MENU_REFRESH => {
                let _ = self.refresh.send(());
            }
            UserEvent::Menu(_) => {}
        }
    }
}

/// Polls all devices every [`POLL_INTERVAL`], or sooner when `refresh` fires.
fn poll_loop(proxy: EventLoopProxy<UserEvent>, refresh: mpsc::Receiver<()>, wake: Waker) {
    let mut sources = all_sources(wake);
    let mut api = HidApi::new().ok();
    loop {
        let statuses = match api.as_mut() {
            Some(api) => {
                let _ = api.refresh_devices();
                sources.iter_mut().flat_map(|s| s.poll(api)).collect()
            }
            None => {
                api = HidApi::new().ok();
                Vec::new()
            }
        };
        if proxy.send_event(UserEvent::Statuses(statuses)).is_err() {
            return; // event loop is gone, the app is quitting
        }
        if let Err(RecvTimeoutError::Disconnected) = refresh.recv_timeout(POLL_INTERVAL) {
            return;
        }
        // Several requests that piled up during the poll need one poll, not many.
        while refresh.try_recv().is_ok() {}
    }
}

/// `false` when another TrayJuice already runs in this user session. Two
/// copies would draw their widgets on top of each other.
fn first_instance() -> bool {
    let name: Vec<u16> = "Local\\TrayJuice.SingleInstance"
        .encode_utf16()
        .chain([0])
        .collect();
    // The handle is never closed: the mutex must live as long as the process.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    handle.is_null() || unsafe { GetLastError() } != ERROR_ALREADY_EXISTS
}

fn main() {
    if !first_instance() {
        trayjuice::log!("another instance is already running, exiting");
        return;
    }

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);

    let menu_proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = menu_proxy.send_event(UserEvent::Menu(e));
    }));

    let widget_item = CheckMenuItem::with_id(MENU_WIDGET, "Show in taskbar", true, true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &widget_item,
        &MenuItem::with_id(MENU_REFRESH, "Refresh now", true, None),
        &MenuItem::with_id(MENU_QUIT, "Quit TrayJuice", true, None),
    ])
    .expect("failed to build menu");

    let (refresh_tx, refresh_rx) = mpsc::channel();
    let poll_proxy = event_loop.create_proxy();
    // Backends ask for a poll through the same channel as "Refresh now".
    let wake_tx = refresh_tx.clone();
    let wake: Waker = Arc::new(move || {
        let _ = wake_tx.send(());
    });
    let watching = trayjuice::hotplug::watch(wake.clone());
    trayjuice::log!("hotplug: watching={watching}");
    thread::spawn(move || poll_loop(poll_proxy, refresh_rx, wake));

    // winit made the process DPI-aware above, so this is the real pixel size.
    let icon_size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as usize;

    let image = Icon::from_rgba(
        icon::app_icon(icon_size),
        icon_size as u32,
        icon_size as u32,
    )
    .expect("icon buffer has the right size");
    let tray = TrayIconBuilder::new()
        .with_icon(image)
        .with_tooltip("TrayJuice")
        .with_menu(Box::new(menu))
        .build()
        .expect("failed to create tray icon");

    let mut app = App {
        tray,
        widget: Widget::new(),
        widget_item,
        statuses: Vec::new(),
        refresh: refresh_tx,
    };
    event_loop.run_app(&mut app).expect("event loop failed");
}
