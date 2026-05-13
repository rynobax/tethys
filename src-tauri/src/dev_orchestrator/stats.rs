//! Per-worktree memory readouts for the polling UI. Pulls `docker
//! stats --no-stream` for django/celery containers and `ps` for
//! rspack processes. Cheap enough to run on a 5s cadence.

use std::collections::HashMap;
use std::process::Command;

use serde::Serialize;
use serde_json::Value;

use super::config::OrchestratorConfig;

/// Live memory snapshot for one workspace. `0` for any service that
/// isn't currently running.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WorkspaceMemory {
    pub workspace_id: String,
    pub fe_mib: u64,
    pub be_mib: u64,
}

/// Per-workspace RAM. Caller passes the workspace ids it cares about
/// + their FE worktree paths (used to scope the rspack pgrep) and the
/// corresponding container names (computed from the `short` suffix).
///
/// Single docker call collects RAM for all known BE containers in one
/// shot; ps pulls all rspack workers at once.
pub fn workspace_memory(
    cfg: &OrchestratorConfig,
    workspaces: &[(String /*workspace_id*/, String /*short*/, std::path::PathBuf /*fe_dir*/)],
) -> Vec<WorkspaceMemory> {
    let containers: Vec<String> = workspaces
        .iter()
        .map(|(_, short, _)| cfg.be_container(short))
        .collect();
    let be_ram = docker_stats_mib(&containers);
    let fe_ram = rspack_ram_by_fe_dir(workspaces.iter().map(|(_, _, p)| p.as_path()));
    workspaces
        .iter()
        .map(|(wid, short, fe_dir)| WorkspaceMemory {
            workspace_id: wid.clone(),
            fe_mib: *fe_ram.get(fe_dir.as_path()).unwrap_or(&0),
            be_mib: *be_ram.get(&cfg.be_container(short)).unwrap_or(&0),
        })
        .collect()
}

/// `docker stats --no-stream --format '{{json .}}'` with **no** name args
/// — runs over every running container — and we filter to the names we
/// care about. Doing it this way (rather than passing names explicitly)
/// means a missing container is just absent from the output instead of
/// `docker stats` erroring with `No such container` and dropping the
/// whole batch (which would zero out every workspace's RAM whenever a
/// single dev stack is stopped).
fn docker_stats_mib(containers: &[String]) -> HashMap<String, u64> {
    let mut map = HashMap::new();
    if containers.is_empty() {
        return map;
    }
    let wanted: std::collections::HashSet<&str> =
        containers.iter().map(|s| s.as_str()).collect();
    let out = Command::new("docker")
        .args(["stats", "--no-stream", "--format", "{{json .}}"])
        .output();
    let Ok(out) = out else { return map };
    if !out.status.success() {
        return map;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let name = v
            .get("Name")
            .and_then(|n| n.as_str())
            .unwrap_or("");
        if !wanted.contains(name) {
            continue;
        }
        // "MemUsage" looks like "1.192GiB / 15.6GiB" — parse the left side.
        let mem = v
            .get("MemUsage")
            .and_then(|m| m.as_str())
            .unwrap_or("");
        if let Some(mib) = parse_mem_usage(mem) {
            map.insert(name.to_string(), mib);
        }
    }
    map
}

/// Parse the LHS of a docker-stats MemUsage string ("1.192GiB / 15.6GiB").
fn parse_mem_usage(s: &str) -> Option<u64> {
    let lhs = s.split('/').next()?.trim();
    let (num, unit) = lhs
        .find(|c: char| c.is_ascii_alphabetic())
        .map(|i| lhs.split_at(i))?;
    let num: f64 = num.trim().parse().ok()?;
    let mib = match unit.trim() {
        "B" => num / (1024.0 * 1024.0),
        "KiB" | "KB" => num / 1024.0,
        "MiB" | "MB" => num,
        "GiB" | "GB" => num * 1024.0,
        _ => return None,
    };
    Some(mib.round() as u64)
}

/// `ps -axo pid,rss,command` and aggregate rspack worker RSS by the
/// frontend dir embedded in the command line.
fn rspack_ram_by_fe_dir<'a>(
    fe_dirs: impl Iterator<Item = &'a std::path::Path>,
) -> HashMap<std::path::PathBuf, u64> {
    let dirs: Vec<std::path::PathBuf> = fe_dirs.map(|p| p.to_path_buf()).collect();
    let mut map = HashMap::new();
    for d in &dirs {
        map.insert(d.clone(), 0u64);
    }
    let Ok(out) = Command::new("ps").args(["-axo", "pid,rss,command"]).output() else {
        return map;
    };
    if !out.status.success() {
        return map;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let trimmed = line.trim_start();
        // Expect: <pid> <rss> <command...>
        let mut parts = trimmed.splitn(3, char::is_whitespace).filter(|s| !s.is_empty());
        let _pid = parts.next();
        let Some(rss_str) = parts.next() else { continue };
        let Some(cmd) = parts.next() else { continue };
        if !cmd.contains("rspack serve") {
            continue;
        }
        let Ok(rss_kib) = rss_str.parse::<u64>() else {
            continue;
        };
        let rss_mib = rss_kib / 1024;
        for d in &dirs {
            if cmd.contains(d.to_string_lossy().as_ref()) {
                let entry = map.entry(d.clone()).or_insert(0);
                *entry += rss_mib;
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_mem_units() {
        assert_eq!(parse_mem_usage("1.0GiB / 15.6GiB"), Some(1024));
        assert_eq!(parse_mem_usage("245.5MiB / 15.6GiB"), Some(246));
        assert_eq!(parse_mem_usage("512KiB / 15.6GiB"), Some(0));
        assert_eq!(parse_mem_usage("garbage"), None);
    }
}
