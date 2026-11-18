//! Optional debug log. Set the `TRAYJUICE_LOG` environment variable to a file
//! path to get a timestamped trace; without it logging is a no-op.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static LOG: OnceLock<Option<(Mutex<File>, Instant)>> = OnceLock::new();

pub fn write(args: std::fmt::Arguments) {
    let log = LOG.get_or_init(|| {
        let path = std::env::var_os("TRAYJUICE_LOG")?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        Some((Mutex::new(file), Instant::now()))
    });
    if let Some((file, start)) = log
        && let Ok(mut file) = file.lock()
    {
        let _ = writeln!(file, "{:>9.3} {args}", start.elapsed().as_secs_f32());
    }
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::log::write(format_args!($($arg)*)) };
}
