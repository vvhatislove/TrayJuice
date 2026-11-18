# TrayJuice

Battery levels of your wireless peripherals, right on the Windows taskbar.

TrayJuice is a small Rust app that draws a device glyph and a percentage for
each device next to the notification area, in the style of the system icons,
so you never have to open a vendor app to find out whether your headset is
about to die.

## What you see

Each device is a glyph (headset, mouse, keyboard, gamepad) followed by its
level, for example `30%`. The text follows the taskbar theme and turns red at
20% or less, green while charging, and dims to a dash when the receiver is
plugged in but the device is off or out of range.

Hover a device on the taskbar to see its name in a tooltip above the
taskbar. A TrayJuice icon in the tray
shows all levels on hover. Right-click it for:

- **Show in taskbar**: turn the taskbar widget on or off. When it is off,
  every device gets its own tray icon with the level as a number on a
  coloured tile (green above 50%, amber 21–50%, red 20% or less, blue while
  charging, grey when off).
- **Refresh now** and **Quit TrayJuice**.

Levels refresh once a minute.

## Known limitations

- The taskbar widget relies on embedding a window into the taskbar, which
  Windows has no public API for. It was built on Windows 11 build 26200 at
  100% scaling and may stop working after a Windows update; the tray tiles
  keep working in that case.
- With many open apps the widget can overlap taskbar buttons next to the
  notification area.
- Other display scales, the light theme, vertical or secondary taskbars and
  Windows 10 are untested.
- The "Show in taskbar" choice is not saved between runs.

## Supported devices

| Device | Connection | Status |
|---|---|---|
| Logitech PRO X Wireless (`046d:0aba`) | USB receiver | tested |
| Logitech PRO X Wireless (`046d:0aaa`), G PRO (`046d:0aa7`), PRO X 2 (`046d:0afb`, `046d:0afc`) | USB receiver | same protocol, untested |
| Delux M800 (receiver `1d57:fa60`, model byte `3a`) | USB receiver | experimental, off by default (see below) |
| Delux M600 Pro, Attack Shark X11 (same `1d57:fa60` receiver) | USB receiver | experimental, untested |

Logitech headsets report battery voltage, not a percentage, so the number is
an estimate from a discharge curve and can differ from G HUB by a few percent.

Mice on the `1d57:fa60` receiver are **off by default**. Set the
`TRAYJUICE_EXPERIMENTAL_DELUX` environment variable to turn them on, and
mind the problems seen with a Delux M800:

- After TrayJuice kept the receiver's battery channel open for a few
  minutes, the Delux app showed "No Device" until the receiver was
  replugged. TrayJuice now opens it only for 2.5 seconds per poll, but that
  has not been proven safe.
- The mouse reports its level only while the Delux app window is open, so
  the number is picked up then and kept for two hours.

TrayJuice only listens on this receiver and sends nothing to the mouse.

Bluetooth devices and other vendors are not supported yet. Adding one means
implementing a single trait, see [Adding a device](#adding-a-device).

## Build and run

Requires Windows 10 or 11 and a [Rust toolchain](https://rustup.rs) (MSVC).

```
cargo build --release
```

The binary is `target\release\trayjuice.exe`. It has no window and no
installer: run it and the icon appears. To start it with Windows, put a
shortcut to it into the folder that `shell:startup` opens.

## Adding a device

1. Run `cargo run --example probe -- all` to list HID interfaces and see what
   the existing backends report.
2. Implement `BatterySource` (see `src/device.rs`) in a new module and set
   the right `DeviceKind`. Use `src/logitech.rs` as a reference.
3. Register it in `all_sources()`.
4. Add the device to the table above, marked as tested only if you ran it on
   real hardware.

`cargo run --example widget_preview` and `cargo run --example icon_preview`
write BMP images of the taskbar widget and the tray tiles for a range of
states, which helps when changing the rendering.

## Credits

Protocol facts come from other open projects; no code was copied from them.

- Logitech headsets: [HeadsetControl](https://github.com/Sapd/HeadsetControl).
- Delux and Attack Shark mice:
  [OpenMouse mouse-protocol](https://github.com/OpenMouse-Project/mouse-protocol)
  and [mouse-battery-tray](https://github.com/incconutwo/mouse-battery-tray).

## License

[MIT](LICENSE) © Vladislav Petrenko. Use it however you like; keep the
copyright notice.
