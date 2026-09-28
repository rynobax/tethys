//! A Tethys launched via Yarn PnP inherits `NODE_OPTIONS=--require
//! <tethys>/.pnp.cjs …` and friends; left in place, `node` in any child repo
//! tries to load Tethys's PnP runtime and dies with `Cannot find module`.

use std::env;

fn is_injected_pm_var(key: &str) -> bool {
    matches!(key, "BERRY_BIN_FOLDER" | "PROJECT_CWD" | "INIT_CWD") || key.starts_with("npm_")
}

fn is_loader_flag(flag: &str) -> bool {
    matches!(
        flag,
        "--require" | "-r" | "--loader" | "--experimental-loader" | "--import"
    )
}

fn is_pnp_path(value: &str) -> bool {
    value.contains(".pnp.")
}

/// `None` when nothing survives and the variable should be dropped.
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

#[derive(Debug, PartialEq, Eq)]
enum EnvAction {
    Remove,
    Set(String),
}

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

pub fn sanitize_for_child_repo<C: ChildCommandEnv>(cmd: &mut C) {
    // `vars()` panics on a non-UTF-8 var; the ones we strip are always UTF-8.
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

        assert_eq!(action_for(&overrides, "NODE_OPTIONS"), Some(&EnvAction::Remove));
        assert_eq!(action_for(&overrides, "BERRY_BIN_FOLDER"), Some(&EnvAction::Remove));
        assert_eq!(action_for(&overrides, "npm_config_user_agent"), Some(&EnvAction::Remove));
        assert_eq!(action_for(&overrides, "npm_execpath"), Some(&EnvAction::Remove));
        assert_eq!(action_for(&overrides, "PROJECT_CWD"), Some(&EnvAction::Remove));
        assert_eq!(action_for(&overrides, "INIT_CWD"), Some(&EnvAction::Remove));
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
