//! macOS memory-pressure reader. We use `sysctl
//! kern.memorystatus_vm_pressure_level` for the level (1=Normal,
//! 2=Warning, 4=Critical) — it's what Activity Monitor reads — and
//! `vm_stat` for the free-pages count so we can compute a free-% to
//! show in the UI.

use std::process::Command;

use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pressure {
    #[default]
    Normal,
    Warning,
    Critical,
    /// `sysctl` returned an unexpected value (or didn't run). Treated
    /// as Normal by gating logic but rendered distinctly in the UI.
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct SystemMemory {
    pub level: Pressure,
    /// Approx percentage of physical memory that's free or available
    /// for new allocations. Pulled from `vm_stat`'s "Pages free" +
    /// "Pages inactive" / total. Inactive pages are reclaimable, so
    /// they count toward "free" for capacity-planning purposes.
    pub free_pct: u8,
    pub free_mib: u64,
    /// Total physical memory in MiB. Constant across ticks; included so
    /// the UI can render `<free> / <total>` without needing a separate
    /// `sysctl hw.memsize` round-trip.
    pub total_mib: u64,
}

/// Read the current pressure level via `sysctl`. Cheap (~5ms).
pub fn current_level() -> Pressure {
    let out = match Command::new("/usr/sbin/sysctl")
        .args(["-n", "kern.memorystatus_vm_pressure_level"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Pressure::Unknown,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    match text.trim() {
        "1" => Pressure::Normal,
        "2" => Pressure::Warning,
        "4" => Pressure::Critical,
        _ => Pressure::Unknown,
    }
}

/// Total physical RAM in bytes, via `sysctl hw.memsize`. This is what
/// Activity Monitor labels "Physical Memory" and is the most reliable
/// source — summing `vm_stat` page categories misses buckets (purgeable,
/// throttled, file-backed, etc.) and undercounts.
fn sysctl_memsize_bytes() -> Option<u64> {
    let out = Command::new("/usr/sbin/sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().ok()
}

/// Parse `vm_stat` for the page counts that Activity Monitor sums
/// into "Memory Used". Apple's formula (per WWDC talks and the
/// `top` tool's PhysMem line) is:
///
///   used = anonymous + wired_down + occupied_by_compressor
///
/// Where:
///   - "Anonymous pages" is Activity Monitor's "App Memory" (active +
///     inactive *non-file-backed* pages — i.e. heap-like data the OS
///     can't just drop).
///   - "Wired down" is the kernel/driver pinned set.
///   - "Pages occupied by compressor" is the in-RAM compressor footprint
///     (NOT "Pages stored in compressor" — that's the logical count of
///     compressed pages, which is much larger and isn't what AM shows).
///
/// File-backed pages and pure speculative/free are NOT in "used" — AM
/// shows file-backed separately as "Cached Files" and counts them as
/// reclaimable.
fn read_vm_stat_used() -> Option<u64> {
    let out = Command::new("/usr/bin/vm_stat").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut page_size: u64 = 16384; // fallback; first line typically overrides
    let mut anonymous_pages: u64 = 0;
    let mut wired_pages: u64 = 0;
    let mut compressed_pages: u64 = 0;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Mach Virtual Memory Statistics: (page size of ") {
            if let Some(num) = rest.split_whitespace().next() {
                if let Ok(v) = num.parse::<u64>() {
                    page_size = v;
                }
            }
        } else if let Some((name, value)) = line.split_once(':') {
            let v = value.trim().trim_end_matches('.').replace(',', "");
            let parsed = v.parse::<u64>().ok();
            if let Some(parsed) = parsed {
                match name.trim() {
                    "Anonymous pages" => anonymous_pages = parsed,
                    "Pages wired down" => wired_pages = parsed,
                    "Pages occupied by compressor" => compressed_pages = parsed,
                    _ => {}
                }
            }
        }
    }
    let used_pages = anonymous_pages + wired_pages + compressed_pages;
    let used_mib = used_pages.saturating_mul(page_size) / 1024 / 1024;
    Some(used_mib)
}

pub fn current() -> SystemMemory {
    let level = current_level();
    // Total physical RAM from sysctl (authoritative).
    let total_mib = sysctl_memsize_bytes()
        .map(|b| b / 1024 / 1024)
        .unwrap_or(0);
    // "Used" matches Activity Monitor's Memory Used = anonymous + wired
    // + compressed. File-backed / inactive / speculative aren't counted
    // (AM shows them as "Cached Files" or reclaimable, not used).
    let used_mib = read_vm_stat_used().unwrap_or(0);
    let free_mib = total_mib.saturating_sub(used_mib);
    let free_pct = if total_mib == 0 {
        0
    } else {
        ((free_mib * 100) / total_mib).min(100) as u8
    };
    SystemMemory {
        level,
        free_pct,
        free_mib,
        total_mib,
    }
}
