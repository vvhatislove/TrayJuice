# TrayJuice

Windows app in Rust that shows battery levels of wireless peripherals on the
taskbar: a device glyph and a percentage per device, left of the notification
area. A tray icon carries the menu, and number tiles in the tray are the
fallback when the taskbar widget is off or cannot be embedded.

## Commands

- `cargo build --release` builds `target\release\trayjuice.exe`.
- `cargo test` runs the unit tests (voltage curve, icon rendering).
- `cargo fmt` and `cargo clippy --all-targets` must be clean before a commit.
- `cargo run --example probe` prints HID interfaces and battery readings to
  the console. Add `-- all` to list every vendor, not only Logitech.
- `cargo run --example widget_preview` writes `widget_preview.bmp`: the
  taskbar widget for sample devices on a dark and a light background.
- `cargo run --example icon_preview` writes `icon_preview.bmp` with enlarged
  tray tiles for many states.
- `cargo run --example taskbar_probe` shows a magenta block in the taskbar
  for 20 seconds. Use it to check that embedding still works on a Windows build.

The release binary has `windows_subsystem = "windows"`, so it prints nothing.
Use the `probe` example to debug device communication. Stop a running
`trayjuice.exe` before rebuilding, otherwise the linker cannot replace it.

## Layout

- `src/device.rs`: `BatteryState`, `DeviceKind`, `DeviceStatus`, the
  `BatterySource` trait, `all_sources()`, voltage-to-percent interpolation.
- `src/logitech.rs`: Logitech HID++ headsets behind a USB receiver.
- `src/taskbar.rs`: the taskbar widget (window embedding, GDI text rendering).
- `src/icon.rs`: tray icons rendered to RGBA with a built-in 5x7 font.
- `src/main.rs`: winit event loop, tray icons, menu, background poll thread.

## Rules

### Devices and protocols

- New device families go in their own module as a `BatterySource` and are
  registered in `all_sources()`. Keep vendor specifics out of `main.rs`.
- Take protocol bytes from a primary source (HeadsetControl, Solaar, vendor
  docs) and link it in the module doc comment. Do not guess them.
- HeadsetControl and Solaar are GPL. Read them for protocol facts only
  (request bytes, response layout, calibration points). Never copy or
  translate their code into this MIT project.
- Only send read-only requests to devices. No writes that change settings,
  firmware or pairing.
- Only talk to interfaces matched by exact vendor ID, product ID, usage page
  and usage. Never send bytes to a device that is not in a model table.
- Other software (G HUB) shares the same receiver. Match replies by header,
  read in a loop with a deadline, and treat silence as `Offline`, never panic.
- Do not poll more often than once every 30 seconds per device, apart from a
  refresh the user asked for. Polling wakes the radio and drains the battery.
- Mark a device as "tested" in `README.md` only after running it on real
  hardware. The only device verified so far is the PRO X Wireless `046d:0aba`.

### Taskbar widget

- Embedding a window into `Shell_TrayWnd` is undocumented. All of that code,
  and all `unsafe` for it, stays in `src/taskbar.rs`.
- The app must stay usable when embedding fails: `Widget::update` returns
  `false` and `main.rs` falls back to tray tiles. Keep that path working.
- The widget window lives inside Explorer and shares its input queue. The
  main thread must never block: no HID access, no file or network I/O, no
  sleeps, no long loops in the window procedure or event handlers.
- Never restart or kill `explorer.exe` to test or fix something unless the
  maintainer agrees first.
- Use system fonts ("Segoe Fluent Icons" for glyphs, "Segoe UI" for text) and
  follow the taskbar theme. Scale every pixel size by the taskbar DPI.

### Threads

- All HID access happens on the poll thread; all window, tray and menu calls
  happen on the main thread. They talk through `EventLoopProxy` and an mpsc
  channel.

### Verifying changes

- A running process proves nothing about what is on screen. For any visual
  change, look at `widget_preview.bmp` or `icon_preview.bmp`, and check the
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
  use must stay at zero between polls.
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
