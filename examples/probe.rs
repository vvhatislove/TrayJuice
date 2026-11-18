//! Diagnostic: list HID interfaces and print what every backend reports.
//! Run with `cargo run --example probe` (add `-- all` to list every vendor).

use hidapi::HidApi;
use trayjuice::device::all_sources;

fn main() {
    let api = HidApi::new().expect("failed to initialise hidapi");
    let all = std::env::args().any(|a| a == "all");

    println!("HID interfaces:");
    for d in api.device_list() {
        if !all && d.vendor_id() != 0x046d {
            continue;
        }
        println!(
            "  {:04x}:{:04x} usage_page={:04x} usage={:04x} iface={} {:?}",
            d.vendor_id(),
            d.product_id(),
            d.usage_page(),
            d.usage(),
            d.interface_number(),
            d.product_string().unwrap_or("?"),
        );
    }

    println!("\nBattery:");
    let mut found = false;
    for mut source in all_sources() {
        for status in source.poll(&api) {
            found = true;
            println!("  {}: {:?}", status.name, status.state);
        }
    }
    if !found {
        println!("  no supported devices found");
    }
}
