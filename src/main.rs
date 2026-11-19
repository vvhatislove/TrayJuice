#![windows_subsystem = "windows"]

use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use hidapi::HidApi;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use trayjuice::alert::{self, LowBattery};
use trayjuice::device::{DeviceStatus, Waker, all_sources};
use trayjuice::icon;
use trayjuice::notify::Notifier;
use trayjuice::settings::{self, Setting};
use trayjuice::taskbar::Widget;
use trayjuice::update::{self, Offer};
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
const MENU_AUTOSTART: &str = "autostart";
const MENU_ALERTS: &str = "alerts";
const MENU_UPDATE: &str = "update";

enum UserEvent {
    Statuses(Vec<DeviceStatus>),
    Menu(MenuEvent),
    /// Where "Check for updates" has got to.
    Update(Offer),
    /// An installer was started and waits for this copy to leave.
    QuitForUpdate,
}

struct App {
    /// The only tray icon: carries the menu and a tooltip with every level.
    tray: TrayIcon,
    widget: Widget,
    widget_item: CheckMenuItem,
    autostart_item: CheckMenuItem,
    alerts_item: CheckMenuItem,
    low_battery: LowBattery,
    notifier: Notifier,
    update_item: MenuItem,
    update: Offer,
    proxy: EventLoopProxy<UserEvent>,
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

    /// Tells about devices that have just become low. The state is kept up
    /// to date even while alerts are off, so turning them on does not bring
    /// up old news.
    fn alert(&mut self) {
        let Some(text) = self.low_battery.check(&self.statuses) else {
            return;
        };
        if self.alerts_item.is_checked() {
            let shown = self.notifier.show(alert::TITLE, &text);
            trayjuice::log!("alert: {text:?}, shown={shown}");
        }
    }
}

impl App {
    fn set_update(&mut self, offer: Offer) {
        trayjuice::log!("update: {offer:?}");
        self.update_item.set_text(offer.label());
        self.update_item.set_enabled(offer.clickable());
        self.update = offer;
    }

    /// A click on the update item. The network and the disk are only
    /// touched on a thread started here, and only because of this click.
    fn update_clicked(&mut self) {
        let proxy = self.proxy.clone();
        match self.update.clone() {
            Offer::Checking | Offer::Downloading => {}
            Offer::Available {
                version,
                installed: true,
            } => {
                self.set_update(Offer::Downloading);
                thread::spawn(move || {
                    let started = update::download_installer(version)
                        .is_some_and(|path| update::run_installer(&path));
                    trayjuice::log!("update: installer started={started}");
                    let _ = if started {
                        proxy.send_event(UserEvent::QuitForUpdate)
                    } else {
                        // No installer in that release, or no connection.
                        update::open_release_page(version);
                        proxy.send_event(UserEvent::Update(Offer::Idle))
                    };
                });
            }
            Offer::Available { version, .. } => {
                thread::spawn(move || update::open_release_page(version));
            }
            Offer::Idle | Offer::UpToDate | Offer::Failed => {
                self.set_update(Offer::Checking);
                thread::spawn(move || {
                    let _ = proxy.send_event(UserEvent::Update(update::check()));
                });
            }
        }
    }

    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        self.widget.hide();
        let _ = self.tray.set_visible(false);
        event_loop.exit();
    }
}

/// Stores a menu choice. Off the main thread: it shares its input queue
/// with Explorer.
fn save(setting: Setting, on: bool) {
    thread::spawn(move || {
        let done = settings::set(setting, on);
        trayjuice::log!("settings: {setting:?}={on}, done={done}");
    });
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
                self.alert();
            }
            // The check mark has already been toggled by the menu itself.
            // Only a choice made here is stored, not a widget that could
            // not be embedded.
            UserEvent::Menu(e) if e.id.as_ref() == MENU_WIDGET => {
                save(Setting::ShowInTaskbar, self.widget_item.is_checked());
                self.show();
            }
            UserEvent::Menu(e) if e.id.as_ref() == MENU_ALERTS => {
                save(Setting::LowBatteryAlerts, self.alerts_item.is_checked());
            }
            UserEvent::Menu(e) if e.id.as_ref() == MENU_AUTOSTART => {
                // Off the main thread: it shares its input queue with Explorer.
                let wanted = self.autostart_item.is_checked();
                thread::spawn(move || {
                    let done = trayjuice::autostart::set(wanted);
                    trayjuice::log!("autostart: set {wanted}, done={done}");
                });
            }
            UserEvent::Menu(e) if e.id.as_ref() == MENU_UPDATE => self.update_clicked(),
            UserEvent::Update(offer) => self.set_update(offer),
            UserEvent::Menu(e) if e.id.as_ref() == MENU_QUIT => self.quit(event_loop),
            UserEvent::QuitForUpdate => self.quit(event_loop),
            UserEvent::Menu(e) if e.id.as_ref() == MENU_REFRESH => {
                let _ = self.refresh.send(());
            }
            UserEvent::Menu(_) => {}
        }
    }
}

/// Polls all devices every [`POLL_INTERVAL`], or sooner when `refresh` fires.
fn poll_loop(proxy: EventLoopProxy<UserEvent>, refresh: mpsc::Receiver<()>, wake: Waker) {
    // Read on the poll thread, once, at start.
    let unproven = std::env::var_os("TRAYJUICE_HIDPP").is_some();
    trayjuice::log!("unproven backends: {unproven}");
    let mut sources = all_sources(wake, unproven);
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

/// `--probe`: writes the device report to a file in the temp folder and
/// opens it in Notepad, because this executable has no console to print to.
fn write_probe_report() {
    let path = std::env::temp_dir().join("trayjuice-probe.txt");
    if std::fs::write(&path, trayjuice::probe::report(true)).is_ok() {
        let _ = std::process::Command::new("notepad.exe").arg(&path).spawn();
    }
}

fn main() {
    if std::env::args().any(|a| a == "--probe") {
        write_probe_report();
        return;
    }
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

    let widget_item = CheckMenuItem::with_id(
        MENU_WIDGET,
        "Show in taskbar",
        true,
        settings::get(Setting::ShowInTaskbar),
        None,
    );
    let alerts_item = CheckMenuItem::with_id(
        MENU_ALERTS,
        "Low battery alerts",
        true,
        settings::get(Setting::LowBatteryAlerts),
        None,
    );
    let autostart_item = CheckMenuItem::with_id(
        MENU_AUTOSTART,
        "Start with Windows",
        true,
        trayjuice::autostart::is_enabled(),
        None,
    );
    let update_item = MenuItem::with_id(MENU_UPDATE, Offer::Idle.label(), true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &widget_item,
        &alerts_item,
        &autostart_item,
        &MenuItem::with_id(MENU_REFRESH, "Refresh now", true, None),
        &update_item,
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

    let notifier = Notifier::new(tray.window_handle());
    let mut app = App {
        tray,
        alerts_item,
        low_battery: LowBattery::new(),
        notifier,
        update_item,
        update: Offer::Idle,
        proxy: event_loop.create_proxy(),
        widget: Widget::new(),
        widget_item,
        autostart_item,
        statuses: Vec::new(),
        refresh: refresh_tx,
    };
    event_loop.run_app(&mut app).expect("event loop failed");
}
