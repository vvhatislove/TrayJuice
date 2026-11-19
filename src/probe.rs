//! A plain-text report of the HID interfaces on this PC and of what every
//! backend reads from them. Used by the `probe` example and by
//! `trayjuice.exe --probe`, which people run to ask for support of a device.
//!
//! The report holds vendor and product IDs, usage numbers and product names.
//! Serial numbers and device paths are left out on purpose.

use std::fmt::Write;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use hidapi::HidApi;

use crate::device::all_sources;

const VENDOR_LOGITECH: u16 = 0x046d;
/// Backends that listen need a moment to hear their first report.
const LISTEN: Duration = Duration::from_millis(2500);

/// Builds the report. `all_vendors` lists every HID interface, otherwise
/// only Logitech ones.
pub fn report(all_vendors: bool) -> String {
    let mut out = format!("TrayJuice {} probe\n\n", env!("CARGO_PKG_VERSION"));
    let api = match HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            let _ = writeln!(out, "failed to initialise hidapi: {e}");
            return out;
        }
    };

    out.push_str("HID interfaces:\n");
    for d in api.device_list() {
        if !all_vendors && d.vendor_id() != VENDOR_LOGITECH {
            continue;
        }
        let _ = writeln!(
            out,
            "  {}",
            interface_line(
                d.vendor_id(),
                d.product_id(),
                d.usage_page(),
                d.usage(),
                d.interface_number(),
                d.product_string().unwrap_or("?"),
            )
        );
    }

    out.push_str("\nBattery:\n");
    // The report is how an unproven backend gets its first run on hardware.
    let mut sources = all_sources(Arc::new(|| {}), true);
    for source in &mut sources {
        source.poll(&api);
    }
    thread::sleep(LISTEN);
    let mut found = false;
    for source in &mut sources {
        for status in source.poll(&api) {
            found = true;
            let _ = writeln!(out, "  {}: {:?}", status.name, status.state);
        }
    }
    if !found {
        out.push_str("  no supported devices found\n");
    }
    out
}

fn interface_line(
    vendor: u16,
    product: u16,
    usage_page: u16,
    usage: u16,
    interface: i32,
    name: &str,
) -> String {
    format!(
        "{vendor:04x}:{product:04x} usage_page={usage_page:04x} usage={usage:04x} iface={interface} {name:?}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_an_interface() {
        assert_eq!(
            interface_line(0x046d, 0x0aba, 0xff43, 0x0202, 3, "PRO X Wireless"),
            "046d:0aba usage_page=ff43 usage=0202 iface=3 \"PRO X Wireless\""
        );
    }
}
