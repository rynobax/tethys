//! Sanitize the environment Tethys passes to commands it runs *inside child
//! repos* (setup scripts, Claude sessions).
//!
//! When Tethys is itself launched via `yarn` (Yarn Berry / PnP), Yarn injects
//! package-manager context into our process environment — most fatally
//! `NODE_OPTIONS=--require <tethys>/.pnp.cjs --experimental-loader …`. Every
//! subprocess inherits it, so a `yarn install`/`node` invocation in a *different*
//! repo obeys it and tries to load Tethys's PnP runtime, crashing with
//! `Cannot find module '…/.pnp.cjs'`. The child repo is an independent project
//! with its own toolchain; it must start clean.

use std::env;

/// Whether `key` is a variable Yarn Berry / npm inject to advertise the
/// package-manager invocation that launched the current process. These pin a
/// child to *Tethys's* project, so they must not leak into other repos.
fn is_injected_pm_var(key: &str) -> bool {
    matches!(key, "BERRY_BIN_FOLDER" | "PROJECT_CWD" | "INIT_CWD") || key.starts_with("npm_")
}

/// Node loader flags whose argument is a path; Yarn uses these to bootstrap PnP.
fn is_loader_flag(flag: &str) -> bool {
    matches!(
        flag,
        "--require" | "-r" | "--loader" | "--experimental-loader" | "--import"
    )
}

/// A PnP runtime path, e.g. `…/.pnp.cjs` or `file://…/.pnp.loader.mjs`.
fn is_pnp_path(value: &str) -> bool {
    value.contains(".pnp.")
}

/// Remove Yarn PnP loader entries from a `NODE_OPTIONS` value, keeping any
/// unrelated options the user set. Handles both `--require <path>` and
/// `--require=<path>` forms. Returns `None` when nothing meaningful survives,
/// signalling the variable should be dropped entirely.
fn strip_pnp_from_node_options(value: &str) -> Option<String> {
    let tokens: Vec<&str> = value.split_whitespace().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        if let Some((flag, val)) = tok.split_once('=') {
            if is_loader_flag(flag) && is_pnp_path(val) {
                i += 1;
                continue;
            }
        } else if is_loader_flag(tok) {
            if let Some(next) = tokens.get(i + 1) {
                if is_pnp_path(next) {
                    i += 2;
                    continue;
                }
            }
        }
        kept.push(tok);
        i += 1;
    }
    if kept.is_empty() {
        None
    } else {
        Some(kept.join(" "))
    }
}

/// What to do with a single environment variable when cleaning a child command.
#[derive(Debug, PartialEq, Eq)]
enum EnvAction {
    Remove,
    Set(String),
}

/// Compute the environment overrides needed to clean a child-repo command,
/// given the variables of the current process. Pure, so it's exercised
/// directly by the tests below.
fn child_env_overrides<I>(vars: I) -> Vec<(String, EnvAction)>
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut overrides = Vec::new();
    for (key, value) in vars {
        if key == "NODE_OPTIONS" {
            match strip_pnp_from_node_options(&value) {
                None => overrides.push((key, EnvAction::Remove)),
                Some(stripped) if stripped != value => {
                    overrides.push((key, EnvAction::Set(stripped)))
                }
                Some(_) => {}
            }
        } else if is_injected_pm_var(&key) {
            overrides.push((key, EnvAction::Remove));
        }
    }
    overrides
}

/// A command builder whose inherited environment we can edit. Implemented for
/// both [`tokio::process::Command`] and [`portable_pty::CommandBuilder`], the
/// two ways Tethys spawns processes into child repos.
pub trait ChildCommandEnv {
    fn remove_var(&mut self, key: &str);
    fn set_var(&mut self, key: &str, value: &str);
}

impl ChildCommandEnv for tokio::process::Command {
    fn remove_var(&mut self, key: &str) {
        self.env_remove(key);
    }
    fn set_var(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
}

impl ChildCommandEnv for portable_pty::CommandBuilder {
    fn remove_var(&mut self, key: &str) {
        self.env_remove(key);
    }
    fn set_var(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
}

/// Strip the package-manager context Tethys inherited from its own launcher so
/// `cmd` starts with a clean toolchain when run inside a child repo. Both
/// command types inherit the full process environment by default; this edits
/// only the leaked variables, leaving `PATH`, `HOME`, etc. intact.
pub fn sanitize_for_child_repo<C: ChildCommandEnv>(cmd: &mut C) {
    // `vars_os` (not `vars`) so a non-UTF-8 var elsewhere in the environment
    // can't panic us mid-spawn; the leaked Yarn/PnP vars are always UTF-8, so
    // skipping the rest is safe.
    let vars = env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)));
    for (key, action) in child_env_overrides(vars) {
        match action {
            EnvAction::Remove => cmd.remove_var(&key),
            EnvAction::Set(value) => cmd.set_var(&key, &value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action_for<'a>(overrides: &'a [(String, EnvAction)], key: &str) -> Option<&'a EnvAction> {
        overrides.iter().find(|(k, _)| k == key).map(|(_, a)| a)
    }

    /// The exact poisoned environment observed when Tethys is launched via
    /// `yarn`: PnP `NODE_OPTIONS` plus Yarn's bootstrap vars. All of it must be
    /// stripped, while unrelated vars (`PATH`, `HOME`) are left untouched.
    #[test]
    fn strips_yarn_pnp_context_from_child_env() {
        let env = vec![
            (
                "NODE_OPTIONS".to_string(),
                "--require /Users/ryan/code/tethys/.pnp.cjs --experimental-loader file:///Users/ryan/code/tethys/.pnp.loader.mjs".to_string(),
            ),
            ("BERRY_BIN_FOLDER".to_string(), "/tmp/xfs-abc".to_string()),
            ("npm_config_user_agent".to_string(), "yarn/4.11.0".to_string()),
            ("npm_execpath".to_string(), "/tmp/xfs-abc/yarn".to_string()),
            ("PROJECT_CWD".to_string(), "/Users/ryan/code/tethys".to_string()),
            ("INIT_CWD".to_string(), "/Users/ryan/code/tethys".to_string()),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("HOME".to_string(), "/Users/ryan".to_string()),
        ];

        let overrides = child_env_overrides(env);

        // The whole NODE_OPTIONS was PnP, so it's dropped entirely.
        assert_eq!(action_for(&overrides, "NODE_OPTIONS"), Some(&EnvAction::Remove));
        assert_eq!(action_for(&overrides, "BERRY_BIN_FOLDER"), Some(&EnvAction::Remove));
        assert_eq!(action_for(&overrides, "npm_config_user_agent"), Some(&EnvAction::Remove));
        assert_eq!(action_for(&overrides, "npm_execpath"), Some(&EnvAction::Remove));
        assert_eq!(action_for(&overrides, "PROJECT_CWD"), Some(&EnvAction::Remove));
        assert_eq!(action_for(&overrides, "INIT_CWD"), Some(&EnvAction::Remove));
        // Unrelated vars are not mentioned, so they pass through inherited.
        assert_eq!(action_for(&overrides, "PATH"), None);
        assert_eq!(action_for(&overrides, "HOME"), None);
    }

    #[test]
    fn drops_only_pnp_entries_keeping_user_node_options() {
        let input =
            "--max-old-space-size=4096 --require /Users/ryan/code/tethys/.pnp.cjs --enable-source-maps";
        assert_eq!(
            strip_pnp_from_node_options(input).as_deref(),
            Some("--max-old-space-size=4096 --enable-source-maps")
        );
    }

    #[test]
    fn keeps_non_pnp_require() {
        let input = "--require /some/other/preload.js";
        assert_eq!(strip_pnp_from_node_options(input).as_deref(), Some(input));
    }

    #[test]
    fn handles_equals_form() {
        let input = "--experimental-loader=file:///x/.pnp.loader.mjs --require=/x/.pnp.cjs";
        assert_eq!(strip_pnp_from_node_options(input), None);
    }

    #[test]
    fn fully_pnp_node_options_is_removed() {
        let input = "--require /x/.pnp.cjs --experimental-loader file:///x/.pnp.loader.mjs";
        assert_eq!(strip_pnp_from_node_options(input), None);
    }
}
