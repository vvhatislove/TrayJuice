//! Diagnostic: list HID interfaces and print what every backend reports.
//! Run with `cargo run --example probe` (add `-- all` to list every vendor).
//! `trayjuice.exe --probe` writes the same report, for every vendor, to a file.

fn main() {
    let all = std::env::args().any(|a| a == "all");
    print!("{}", trayjuice::probe::report(all));
}
