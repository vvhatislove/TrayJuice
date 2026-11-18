# TrayJuice

Windows tray app in Rust that shows battery levels of wireless peripherals.
One tray icon per device; the icon is a coloured tile with the percentage.

## Commands

- `cargo build --release` builds `target\release\trayjuice.exe`.
- `cargo test` runs the unit tests (voltage curve, icon rendering).
- `cargo run --example probe` prints HID interfaces and battery readings to
  the console. Add `-- all` to list every vendor, not only Logitech.
- `cargo run --example icon_preview` writes `icon_preview.bmp` with enlarged
  icons for many states. Look at it after any change to `src/icon.rs`.

The release binary has `windows_subsystem = "windows"`, so it prints nothing.
Use the `probe` example to debug device communication. Stop a running
`trayjuice.exe` before rebuilding, otherwise the linker cannot replace it.

## Layout

- `src/device.rs`: `BatteryState`, `DeviceStatus`, the `BatterySource` trait,
  `all_sources()` and the voltage-to-percent interpolation.
- `src/logitech.rs`: Logitech HID++ headsets behind a USB receiver.
- `src/icon.rs`: renders the tray icon to RGBA with a built-in 5x7 font.
- `src/main.rs`: winit event loop, tray icons, menu, background poll thread.

## Rules

- New device families go in their own module as a `BatterySource` and are
  registered in `all_sources()`. Keep vendor specifics out of `main.rs`.
- Take protocol bytes from a primary source (HeadsetControl, Solaar, vendor
  docs) and link it in the module doc comment. Do not guess them.
- Other software (G HUB) shares the same receiver. Match replies by header,
  read in a loop with a deadline, and treat silence as `Offline`, never panic.
- Only send read-only requests to devices. No writes that change settings.
- Mark a device as "tested" in `README.md` only after running it on real
  hardware. The only device verified so far is the PRO X Wireless `046d:0aba`.
- All HID access happens on the poll thread; all tray and menu calls happen
  on the main thread. They talk through `EventLoopProxy` and an mpsc channel.
- Code, comments, README and commit messages are in English.
- The project is MIT licensed, copyright Vladislav Petrenko. Do not add
  dependencies with licenses incompatible with MIT.
