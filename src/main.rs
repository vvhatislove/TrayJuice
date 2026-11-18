#![windows_subsystem = "windows"]

use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use hidapi::HidApi;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use trayjuice::device::{BatteryState, DeviceKind, DeviceStatus, all_sources};
use trayjuice::icon;
use trayjuice::taskbar::Widget;
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
    /// Tile mode: one tray icon per device, in the order the poller reports them.
    icons: Vec<(DeviceStatus, TrayIcon)>,
    /// Widget mode: a single tray icon that carries the menu and the tooltip.
    app_icon: Option<TrayIcon>,
    widget: Widget,
    widget_item: CheckMenuItem,
    /// Latest poll result, kept so the display can be rebuilt on a mode switch.
    statuses: Vec<DeviceStatus>,
    menu: Menu,
    icon_size: usize,
    refresh: mpsc::Sender<()>,
}

impl App {
    fn show(&mut self) {
        let wanted = self.widget_item.is_checked();
        trayjuice::log!("show: widget wanted={wanted}, statuses={:?}", self.statuses);
        if wanted && !self.statuses.is_empty() && self.widget.update(&self.statuses) {
            self.icons.clear();
            self.show_app_icon();
        } else {
            trayjuice::log!("show: falling back to tray tiles");
            self.widget.hide();
            self.app_icon = None;
            self.show_tiles();
        }
    }

    fn show_app_icon(&mut self) {
        let tooltip = self
            .statuses
            .iter()
            .map(DeviceStatus::summary)
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        match &self.app_icon {
            Some(tray) => {
                let _ = tray.set_tooltip(Some(tooltip));
            }
            None => {
                let size = self.icon_size;
                let image = Icon::from_rgba(icon::app_icon(size), size as u32, size as u32)
                    .expect("icon buffer has the right size");
                self.app_icon = Some(self.build_tray(image, tooltip));
            }
        }
    }

    fn show_tiles(&mut self) {
        let mut statuses = self.statuses.clone();
        if statuses.is_empty() {
            statuses.push(DeviceStatus {
                name: "No supported devices".into(),
                kind: DeviceKind::Other,
                state: BatteryState::Offline,
            });
        }
        // Tray icons cannot be reordered, so rebuild when the device set changes.
        let same_devices = statuses.len() == self.icons.len()
            && statuses
                .iter()
                .zip(&self.icons)
                .all(|(s, (old, _))| s.name == old.name);
        if !same_devices {
            self.icons.clear();
        }
        for (i, status) in statuses.into_iter().enumerate() {
            let image = self.image(&status.state);
            let tooltip = status.summary();
            match self.icons.get_mut(i) {
                Some((old, tray)) => {
                    if *old != status {
                        let _ = tray.set_icon(Some(image));
                        let _ = tray.set_tooltip(Some(tooltip));
                        *old = status;
                    }
                }
                None => {
                    let tray = self.build_tray(image, tooltip);
                    self.icons.push((status, tray));
                }
            }
        }
    }

    fn build_tray(&self, image: Icon, tooltip: String) -> TrayIcon {
        TrayIconBuilder::new()
            .with_icon(image)
            .with_tooltip(tooltip)
            .with_menu(Box::new(self.menu.clone()))
            .build()
            .expect("failed to create tray icon")
    }

    fn image(&self, state: &BatteryState) -> Icon {
        let size = self.icon_size;
        Icon::from_rgba(icon::render(state, size), size as u32, size as u32)
            .expect("icon buffer has the right size")
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
                self.icons.clear();
                self.app_icon = None;
                self.widget.hide();
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
fn poll_loop(proxy: EventLoopProxy<UserEvent>, refresh: mpsc::Receiver<()>) {
    let mut sources = all_sources();
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
    }
}

fn main() {
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
    thread::spawn(move || poll_loop(poll_proxy, refresh_rx));

    // winit made the process DPI-aware above, so this is the real pixel size.
    let icon_size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as usize;

    let mut app = App {
        icons: Vec::new(),
        app_icon: None,
        widget: Widget::new(),
        widget_item,
        statuses: Vec::new(),
        menu,
        icon_size,
        refresh: refresh_tx,
    };
    event_loop.run_app(&mut app).expect("event loop failed");
}
