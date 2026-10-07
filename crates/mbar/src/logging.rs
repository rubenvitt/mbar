//! Daemon log output.
//!
//! * [`daemon_log`]: SketchyBar's daemon log (stdout, `[YYYY-MM-DD HH:MM:SS] ` prefix,
//!   `cli.md` §2.4) for `Effect::Log` and the config messages of §13.2.
//! * A minimal `log` backend on stderr, level from `MBAR_LOG`
//!   (`error|warn|info|debug|trace`, default `warn`).

use std::io::Write;

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::max_level()
    }

    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            let _ = writeln!(
                std::io::stderr().lock(),
                "[{}] mbar {} {}: {}",
                timestamp(),
                r.level(),
                r.target(),
                r.args()
            );
        }
    }

    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

pub fn init() {
    let level = match std::env::var("MBAR_LOG").as_deref() {
        Ok("off") => log::LevelFilter::Off,
        Ok("error") => log::LevelFilter::Error,
        Ok("info") => log::LevelFilter::Info,
        Ok("debug") => log::LevelFilter::Debug,
        Ok("trace") => log::LevelFilter::Trace,
        _ => log::LevelFilter::Warn,
    };
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level);
    }
}

/// Writes one daemon log message to stdout with the local-time prefix. A trailing newline
/// is added when missing.
pub fn daemon_log(msg: &str) {
    let mut out = std::io::stdout().lock();
    let nl = if msg.ends_with('\n') { "" } else { "\n" };
    let _ = write!(out, "[{}] {msg}{nl}", timestamp());
    let _ = out.flush();
}

/// Local time `YYYY-MM-DD HH:MM:SS`.
pub fn timestamp() -> String {
    // SAFETY: `time` accepts NULL; `localtime_r` writes into the provided `tm`.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::from("0000-00-00 00:00:00");
        }
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn timestamp_shape() {
        let t = super::timestamp();
        assert_eq!(t.len(), 19);
        assert_eq!(&t[4..5], "-");
        assert_eq!(&t[10..11], " ");
    }
}
