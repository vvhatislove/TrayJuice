//! Diagnostic: passively prints input reports from one HID collection.
//! Sends nothing to the device.
//!
//! `cargo run --example hid_listen -- <vid> <pid> <usage_page> <usage> [seconds]`
//! (hex without `0x`, e.g. `1d57 fa60 000a 0000 5`).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use hidapi::HidApi;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!("usage: hid_listen <vid> <pid> <usage_page> <usage> [seconds]");
        return;
    }
    let hex = |s: &String| u16::from_str_radix(s, 16).expect("hex number expected");
    let (vid, pid, page, usage) = (hex(&args[0]), hex(&args[1]), hex(&args[2]), hex(&args[3]));
    let seconds: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(5);

    let api = HidApi::new().expect("failed to initialise hidapi");
    let Some(info) = api.device_list().find(|d| {
        d.vendor_id() == vid
            && d.product_id() == pid
            && d.usage_page() == page
            && d.usage() == usage
    }) else {
        println!("collection not found");
        return;
    };
    let dev = match info.open_device(&api) {
        Ok(dev) => dev,
        Err(e) => {
            println!("cannot open: {e}");
            return;
        }
    };

    // Count identical reports instead of flooding the console.
    let mut seen: BTreeMap<Vec<u8>, (usize, f32)> = BTreeMap::new();
    let start = Instant::now();
    let mut buf = [0u8; 64];
    while start.elapsed() < Duration::from_secs(seconds) {
        match dev.read_timeout(&mut buf, 250) {
            Ok(0) => {}
            Ok(n) => {
                let entry = seen
                    .entry(buf[..n].to_vec())
                    .or_insert((0, start.elapsed().as_secs_f32()));
                entry.0 += 1;
            }
            Err(e) => {
                println!("read error: {e}");
                break;
            }
        }
    }
    if seen.is_empty() {
        println!("no reports in {seconds} s");
    }
    for (report, (count, first)) in seen {
        let bytes: Vec<String> = report.iter().map(|b| format!("{b:02x}")).collect();
        println!("{count:>5}x first at {first:>5.2}s: {}", bytes.join(" "));
    }
}
