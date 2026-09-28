//! Tethys only starts the sampler. It's a detached singleton that outlives
//! Tethys on purpose: Tethys is one of the suspects it watches.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use tracing::{info, warn};

const DEFAULT_INTERVAL_SECS: u32 = 20;

/// Cargo never copies the script, so an installed Tethys.app samples only
/// while the checkout stays put.
fn script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/memwatch.sh")
}

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

    // It execs `top` before detaching; keep that off the setup thread.
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

    #[test]
    fn sampler_script_is_where_we_think_it_is() {
        let path = script_path();
        assert!(path.exists(), "sampler missing at {}", path.display());
    }

    #[test]
    fn off_disables_and_bad_values_fall_back() {
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
