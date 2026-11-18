//! Experiment: does reading feature report `0xA0` from the `1d57:fa60`
//! receiver make the mouse send its battery message?
//!
//! Sends exactly one `GET_FEATURE 0xA0` (a read, no settings are written)
//! and prints every message that arrives on the battery collection.
//! `0xA0` is the "read access status" report documented in
//! <https://github.com/HarukaYamamoto0/attack-shark-x11-driver>
//! (`docs/protocols/report-id-reading.md`). Never read report `0x10`: that
//! document warns it can put the mouse into bootloader mode.
//!
//! `cargo run --example delux_query -- [listen-only]`

use std::time::{Duration, Instant};

use hidapi::{HidApi, HidDevice};

const VENDOR: u16 = 0x1d57;
const RECEIVER: u16 = 0xfa60;
/// Windows wants the full feature report length of the config collection.
const FEATURE_LEN: usize = 262;

fn open(api: &HidApi, usage_page: u16) -> Option<HidDevice> {
    api.device_list()
        .find(|d| {
            d.vendor_id() == VENDOR
                && d.product_id() == RECEIVER
                && d.usage_page() == usage_page
                && d.usage() == 0
        })?
        .open_device(api)
        .map_err(|e| println!("cannot open {usage_page:04x}: {e}"))
        .ok()
}

fn hex(bytes: &[u8]) -> String {
    let parts: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    parts.join(" ")
}

fn listen(events: &HidDevice, start: Instant, seconds: u64) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut buf = [0u8; 16];
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match events.read_timeout(&mut buf, left.as_millis() as i32) {
            Ok(0) => {}
            Ok(n) => println!(
                "{:>6.2}s event: {}",
                start.elapsed().as_secs_f32(),
                hex(&buf[..n])
            ),
            Err(e) => {
                println!("read error: {e}");
                return;
            }
        }
    }
}

fn main() {
    let listen_only = std::env::args().any(|a| a == "listen-only");
    let api = HidApi::new().expect("failed to initialise hidapi");
    let Some(events) = open(&api, 0x000a) else {
        println!("battery collection not found");
        return;
    };
    let start = Instant::now();

    println!("listening for 3 s before doing anything");
    listen(&events, start, 3);
    if listen_only {
        return;
    }

    let Some(config) = open(&api, 0x000b) else {
        println!("config collection not found");
        return;
    };
    let mut buf = [0u8; FEATURE_LEN];
    buf[0] = 0xa0;
    match config.get_feature_report(&mut buf) {
        Ok(n) => println!(
            "{:>6.2}s GET_FEATURE a0 -> {} bytes: {}",
            start.elapsed().as_secs_f32(),
            n,
            hex(&buf[..n.min(8)])
        ),
        Err(e) => println!(
            "{:>6.2}s GET_FEATURE a0 failed: {e}",
            start.elapsed().as_secs_f32()
        ),
    }
    drop(config);

    println!("listening for 6 s after the read");
    listen(&events, start, 6);
}
