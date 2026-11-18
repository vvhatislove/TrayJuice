# TrayJuice

Windows app in Rust that shows battery levels of wireless peripherals on the
taskbar: a device glyph and a percentage per device, left of the notification
area. A single tray icon carries the menu and a tooltip with every level,
which is also the fallback when the taskbar widget is off or cannot be
embedded. The maintainer does not want per-device icons in the tray.

## Commands

- `cargo build --release` builds `target\release\trayjuice.exe`.
- `cargo test` runs the unit tests (curves, report parsing, icon).
- `cargo fmt` and `cargo clippy --all-targets` must be clean before a commit.
- `cargo run --example probe` prints HID interfaces and battery readings to
  the console. Add `-- all` to list every vendor, not only Logitech.
- `cargo run --example widget_preview` writes `widget_preview.bmp`: the
  taskbar widget for sample devices on a dark and a light background.
- `cargo run --example taskbar_probe` shows a magenta block in the taskbar
  for 20 seconds. Use it to check that embedding still works on a Windows build.

- `cargo run --example hid_listen -- <vid> <pid> <usage_page> <usage>`
  passively prints input reports of one HID collection (sends nothing).
- `cargo run --example delux_query` listens on the Delux receiver and sends
  one harmless `GET_FEATURE 0xA0`; add `listen-only` to send nothing.
- `cargo run --example widget_live` embeds the real widget with sample
  devices for 30 seconds and reports whether its window stays alive.

The release binary has `windows_subsystem = "windows"`, so it prints nothing.
Set the `TRAYJUICE_LOG` environment variable to a file path before starting
it to get a trace of polls, display mode and widget window events
(`src/log.rs`, `trayjuice::log!`). Logging is the one allowed file write on
the main thread, and only when that variable is set.
Use the `probe` example to debug device communication. Stop a running
`trayjuice.exe` before rebuilding, otherwise the linker cannot replace it.

## Layout

- `src/device.rs`: `BatteryState`, `DeviceKind`, `DeviceStatus`, the
  `BatterySource` trait, `all_sources()`, voltage-to-percent interpolation.
- `src/logitech.rs`: Logitech HID++ headsets behind a USB receiver. A
  listener thread per receiver waits for the report the headset sends when
  its charging cable changes and asks for a poll. On the charger the level
  comes from a separate measured table, not from the discharge polynomial.
- `src/delux.rs`: Delux / Attack Shark mice on the `1d57:fa60` receiver.
  Listen-only. Keeps its handle open while the receiver is plugged in,
  because the battery reports stop for good once nothing holds the receiver
  open (see the module docs). The last level is remembered for 10 minutes.
- `src/xbox.rs`: Xbox / XInput gamepads through `XInputGetBatteryInformation`
  (four coarse levels, nothing for a controller on a cable).
- `src/taskbar.rs`: the taskbar widget (window embedding, GDI text rendering).
- `build.rs`: embeds the application manifest (Common Controls v6 for themed
  tooltips, per-monitor DPI awareness). It applies to the binary only, so
  examples get classic-style tooltips.
- `src/log.rs`: optional debug log enabled by `TRAYJUICE_LOG`.
- `src/icon.rs`: the app's tray icon, drawn in code.
- `src/main.rs`: winit event loop, tray icon, menu, background poll thread,
  single-instance mutex.

## Rules

### Devices and protocols

- New device families go in their own module as a `BatterySource` and are
  registered in `all_sources()`. Keep vendor specifics out of `main.rs`.
- Take protocol bytes from a primary source (HeadsetControl, Solaar, vendor
  docs) and link it in the module doc comment. Do not guess them.
- HeadsetControl and Solaar are GPL, OpenMouse is AGPL. Read them for protocol facts only
  (request bytes, response layout, calibration points). Never copy or
  translate their code into this MIT project.
- Prefer listening to reports the device sends by itself over asking for
  them. Some receivers (the `1d57` family) can freeze the radio link when
  written to.
- The `1d57` receiver freezes its radio link when two writes come less than
  a second apart (OpenMouse measurements). Never read feature report `0x10`:
  it can put the mouse into bootloader mode. Any write to this receiver
  needs the maintainer's go-ahead first.
- Close device handles between polls unless a backend documents why it must
  not. `src/delux.rs` is the exception: closing the last handle of the
  `1d57` receiver stops its battery reports until a replug. The Delux app
  showing "No Device" is that same symptom, not a conflict with us. The
  listener in `src/logitech.rs` is the other one: a report sent while
  nothing is open would be lost. It only reads.
- Write down only what was observed. When a cause is a guess, say so in the
  code comment and in `README.md`; several confident explanations of the
  Delux battery stream turned out wrong.
- Only send read-only requests to devices. No writes that change settings,
  firmware or pairing.
- Only talk to interfaces matched by exact vendor ID, product ID, usage page
  and usage. Never send bytes to a device that is not in a model table.
- Other software (G HUB) shares the same receiver. Match replies by header,
  read in a loop with a deadline, and treat silence as `Offline`, never panic.
- Do not poll more often than once every 30 seconds per device, apart from a
  refresh the user asked for or a change the device announced by itself
  (repeats of the same state within 5 seconds are skipped). Polling wakes the radio and drains
  the battery.
- Mark a device as "tested" in `README.md` only after running it on real
  hardware. Verified so far: PRO X Wireless `046d:0aba`, Delux M800 on the
  `1d57:fa60` receiver, and an Xbox controller on the Xbox Wireless Adapter.

### Taskbar widget

- Embedding a window into `Shell_TrayWnd` is undocumented. All of that code,
  and all `unsafe` for it, stays in `src/taskbar.rs`.
- The app must stay usable when embedding fails: `Widget::update` returns
  `false` and the levels stay in the tray icon's tooltip. Keep that working.
- The widget window lives inside Explorer and shares its input queue. The
  main thread must never block: no HID access, no file or network I/O, no
  sleeps, no long loops in the window procedure or event handlers.
- Never restart or kill `explorer.exe` to test or fix something unless the
  maintainer agrees first.
- Nothing we create may be owned by a window inside the taskbar: a topmost
  popup owned by `Shell_TrayWnd` keeps the taskbar above fullscreen apps.
  The tooltip is unowned and placed above the taskbar.
- The widget is `WS_EX_TRANSPARENT`: it must never take mouse input, or the
  taskbar buttons under it stop working. Hover is detected on the 400 ms
  tick from the cursor position, and only when the taskbar is what is under
  the cursor (not a fullscreen app).
- If the notification area (`TrayNotifyWnd`) is not found, remove the widget
  instead of leaving it at its default position over the Start button.
- Use system fonts ("Segoe Fluent Icons" for glyphs, "Segoe MDL2 Assets" for
  the gamepad whose Fluent glyph is a plain pill, "Segoe UI" for text) and
  follow the taskbar theme. Scale every pixel size by the taskbar DPI.

### Threads

- All HID access happens on the poll thread and on listener threads a
  backend starts (they block in a read and send nothing); all window, tray
  and menu calls happen on the main thread. They talk through
  `EventLoopProxy` and an mpsc channel.

### Verifying changes

- A running process proves nothing about what is on screen. For any visual
  change, look at `widget_preview.bmp`, and check the
  real taskbar with a screen capture of the taskbar strip. If a fullscreen
  app hides the taskbar, report the result as unverified.
- Say plainly what was checked on hardware and what was not (device off,
  charging, other DPI scales, light theme, Windows 10).
- Add a unit test for pure logic (curves, parsing of replies, layout maths).

### Footprint and privacy

- No network access, no telemetry, no update checks.
- No administrator rights. Nothing is installed; settings, if added, live
  under the app's own key in `HKCU` or in `%APPDATA%\TrayJuice`.
- Keep the app small: no GUI frameworks, webviews or async runtimes. Idle CPU
  use must stay negligible between polls: the only periodic work allowed is
  the widget's 400 ms tick (a few window queries).
- Add a dependency only when it replaces substantial code, and only with a
  license compatible with MIT. Commit `Cargo.lock`.

### Repository

- Code, comments, README and commit messages are in English. Talk to the
  maintainer in Russian.
- The repository is public. Commit locally when a piece of work is done, but
  push only when the maintainer says so.
- Never commit build output, preview images or anything with personal data
  (device serial numbers, user paths).
- Update the "Supported devices" table and "Known limitations" in
  `README.md` in the same commit as the change they describe.
- The project is MIT licensed, copyright Vladislav Petrenko. Keep the
  copyright line and the credits to protocol sources.
