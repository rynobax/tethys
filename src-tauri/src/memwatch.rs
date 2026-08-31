//! Launches the memory sampler (`scripts/memwatch.sh`) at boot.
//!
//! The sampler exists because "iTerm2 and Tethys are both at 32GB and the
//! machine is hung" is only diagnosable from data taken *during* the event,
//! and the event is intermittent. Leaving it to be started by hand meant it
//! was never running when one happened — the recorded samples stop twelve days
//! before the reports do.
//!
//! Tethys does not own the sampler's lifetime, only its start. The script is a
//! singleton (a second launch exits immediately) and detaches into a session
//! that ignores `SIGHUP`/`SIGINT`, so it outlives both the `pnpm tauri dev`
//! that spawned it and Tethys itself. That is deliberate: Tethys is one of the
//! suspects, and a watchdog that dies with the suspect cannot record the
//! aftermath. `pkill -f memwatch.sh` stops it.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use tracing::{info, warn};

/// Seconds between samples when `TETHYS_MEMWATCH` doesn't say otherwise.
const DEFAULT_INTERVAL_SECS: u32 = 20;

/// Resolved against the crate source directory rather than the executable:
/// `companion_bin`'s next-to-the-exe trick doesn't apply to a file Cargo never
/// copies. This holds for `pnpm install:app` too, since the bundle is built on
/// the machine that has the repo — but it does mean a Tethys.app keeps
/// sampling only as long as the checkout stays put.
fn script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/memwatch.sh")
}

/// `off` disables sampling; an integer overrides the interval in seconds.
fn interval_from_env() -> Option<u32> {
    match std::env::var("TETHYS_MEMWATCH") {
        Err(_) => Some(DEFAULT_INTERVAL_SECS),
        Ok(v) if v.eq_ignore_ascii_case("off") => None,
        Ok(v) => match v.parse::<u32>() {
            Ok(n) if n > 0 => Some(n),
            _ => {
                warn!(value = %v, "TETHYS_MEMWATCH not `off` or a positive integer; using default");
                Some(DEFAULT_INTERVAL_SECS)
            }
        },
    }
}

/// Start the sampler unless it is already running or disabled. Never fatal:
/// a missing script or a failed spawn costs diagnostics, not the app.
pub fn spawn() {
    let Some(interval) = interval_from_env() else {
        info!("memwatch disabled by TETHYS_MEMWATCH=off");
        return;
    };

    let script = script_path();
    if !script.exists() {
        warn!(path = %script.display(), "memwatch script missing; memory sampling disabled");
        return;
    }

    // The script backgrounds its loop and returns in milliseconds, but it does
    // fork and exec `top` on the way, so wait for it off the setup thread.
    std::thread::spawn(move || {
        let result = Command::new("/bin/bash")
            .arg(&script)
            .arg(interval.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();

        match result {
            Ok(out) => {
                // The script reports on stderr either way: which pid it started
                // as, or that another sampler already holds the pidfile.
                let note = String::from_utf8_lossy(&out.stderr);
                let note = note.trim();
                if out.status.success() {
                    info!(interval_secs = interval, "{note}");
                } else {
                    warn!(status = ?out.status, "memwatch launcher failed: {note}");
                }
            }
            Err(e) => warn!(error = %e, "failed to launch memwatch"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `script_path` is a compile-time guess at a file Cargo never touches, so
    /// moving or renaming the sampler would otherwise turn into a warning at
    /// boot that nobody reads until the next hang goes unrecorded.
    #[test]
    fn sampler_script_is_where_we_think_it_is() {
        let path = script_path();
        assert!(path.exists(), "sampler missing at {}", path.display());
    }

    #[test]
    fn off_disables_and_bad_values_fall_back() {
        // `TETHYS_MEMWATCH` is read straight from the environment, so drive
        // the parsing through the same env var this runs on.
        for (value, expected) in [
            ("off", None),
            ("OFF", None),
            ("5", Some(5)),
            ("0", Some(DEFAULT_INTERVAL_SECS)),
            ("banana", Some(DEFAULT_INTERVAL_SECS)),
        ] {
            unsafe { std::env::set_var("TETHYS_MEMWATCH", value) };
            assert_eq!(interval_from_env(), expected, "for {value}");
        }
        unsafe { std::env::remove_var("TETHYS_MEMWATCH") };
        assert_eq!(interval_from_env(), Some(DEFAULT_INTERVAL_SECS));
    }
}
