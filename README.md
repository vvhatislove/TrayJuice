# TrayJuice

Battery levels of your wireless peripherals, right in the Windows tray.

TrayJuice is a small Rust app that puts one icon per device next to the clock.
The icon shows the charge as a number, so you never have to open a vendor app
to find out whether your headset is about to die.

## What the icon means

| Icon | Meaning |
|---|---|
| green tile | more than 50% |
| amber tile | 21–50% |
| red tile | 20% or less |
| blue tile | charging |
| grey tile with `--` | receiver is plugged in, device is off or out of range |

Hover the icon for the device name and exact level. Right-click for
**Refresh now** and **Quit TrayJuice**. Levels refresh once a minute.

## Supported devices

| Device | Connection | Status |
|---|---|---|
| Logitech PRO X Wireless (`046d:0aba`) | USB receiver | tested |
| Logitech PRO X Wireless (`046d:0aaa`), G PRO (`046d:0aa7`), PRO X 2 (`046d:0afb`, `046d:0afc`) | USB receiver | same protocol, untested |

Logitech headsets report battery voltage, not a percentage, so the number is
an estimate from a discharge curve and can differ from G HUB by a few percent.

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
2. Implement `BatterySource` (see `src/device.rs`) in a new module. Use
   `src/logitech.rs` as a reference.
3. Register it in `all_sources()`.
4. Add the device to the table above, marked as tested only if you ran it on
   real hardware.

`cargo run --example icon_preview` writes `icon_preview.bmp` with the icons
for a range of states, which helps when changing the rendering.

## Credits

The Logitech battery protocol and discharge curve follow
[HeadsetControl](https://github.com/Sapd/HeadsetControl).

## License

[MIT](LICENSE) © Vladislav Petrenko. Use it however you like; keep the
copyright notice.
