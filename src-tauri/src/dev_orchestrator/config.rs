//! Project-specific configuration for the dev orchestrator.
//!
//! Tethys is generic — workspaces are `Vec<RepoLink>` with user-chosen
//! string keys. The dev orchestrator needs to know enough about the
//! project to spin up its FE/BE stack, and rather than scatter
//! newlantern-isms across the module, every such value lives in
//! `OrchestratorConfig`. A single constructor (`::newlantern()`) supplies
//! the values today; swapping that for `::from_file(path)` later is a
//! one-call refactor.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// Where the always-on infra stack lives (postgres, redis, mailhog,
    /// master-branch django on the canonical ports). The orchestrator
    /// runs `docker compose up -d` here when the health-check ports
    /// aren't bound.
    pub main_stack_dir: PathBuf,

    /// Ports that must be listening for the main stack to be considered
    /// healthy. Newlantern: postgres :5432, redis :6379.
    pub main_stack_health_ports: Vec<u16>,

    /// Containers whose names are hardcoded in the base compose file and
    /// thus orphan-leak when prior `docker compose up` runs didn't tear
    /// down. The main-stack starter force-removes these on conflict.
    pub main_stack_orphan_container_names: Vec<String>,

    /// Repo keys (matching `RepoLink.repo_key` in `state.rs`) that
    /// identify the FE and BE worktree dirs inside a workspace.
    pub fe_repo_key: String,
    pub be_repo_key: String,

    /// Base branch we diff against to decide whether the worktree has
    /// backend changes that justify spinning up its own BE container.
    pub master_branch: String,

    /// What the FE's API proxy points at when **no** worktree BE is
    /// running. Should resolve to a working master-branch BE.
    pub master_be_url: String,

    /// Env-var name the FE reads for its proxy target. Newlantern's
    /// webpack config reads `NL_PROXY_TARGET`.
    pub fe_proxy_env_var: String,

    /// First port to consider for the FE dev server. The orchestrator
    /// walks upward through `lsof`'d ports until a free one is found.
    pub fe_port_start: u16,

    /// Same for the BE dev server. Reserved space above the main
    /// stack's BE port (which lives at `master_be_url`'s port).
    pub be_port_start: u16,

    /// Shell command for the FE dev server. `{port}` is substituted at
    /// spawn time. Runs under a login zsh so direnv/yarn/docker resolve.
    pub fe_command_template: String,

    /// Shell command for the BE dev server. No substitution needed —
    /// `docker compose up` reads the per-worktree override that has
    /// the port baked in.
    pub be_command_template: String,

    /// Compose project name template — `{short}` substituted. The
    /// dotted naming yields per-worktree project isolation while
    /// keeping the prefix as a stable filter for `docker compose ls`.
    pub compose_project_template: String,

    /// django container name template — `{short}` substituted. The
    /// override file applies this to the django service so multiple
    /// worktree djangos don't fight over the bare name "django".
    pub be_container_template: String,

    /// Compose services to mark `profiles: ["inactive"]` in the
    /// per-worktree override, so they don't start in the worktree
    /// (they only run in the main stack).
    pub inactive_services: Vec<String>,

    /// Files to symlink from `main_stack_dir` into the worktree's BE
    /// dir before `docker compose up`. Their values are read by
    /// `env_file:` directives during compose-time on the host, so
    /// symlinks resolve fine. Updates on `main_stack_dir`'s file
    /// propagate to every worktree on next container restart.
    pub env_symlinks: Vec<String>,

    /// Files to bind-mount into the BE container at the named path.
    /// Use for files django needs to *open* at runtime inside the
    /// container (e.g. `serviceaccount.json`) — a host-path symlink
    /// in the worktree wouldn't resolve from inside the container.
    pub bind_mounts: Vec<BindMount>,

    /// Root dir for the orphan-override scrub. The orchestrator
    /// removes `docker-compose.override.yml` from any subdir of this
    /// path whose worktree no longer exists on disk.
    pub worktree_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct BindMount {
    pub host_path: PathBuf,
    pub container_path: String,
}

impl OrchestratorConfig {
    /// The one in-tree configuration today: newlantern's stack.
    /// Hardcoded so the rest of the orchestrator stays config-driven
    /// without paying for a parser yet. To support other projects,
    /// replace this caller with `OrchestratorConfig::from_file(path)`.
    pub fn newlantern() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/nathangugel".into());
        let main_stack_dir: PathBuf = format!("{home}/newlantern/nl-backend").into();
        Self {
            main_stack_dir: main_stack_dir.clone(),
            main_stack_health_ports: vec![5432, 6379],
            main_stack_orphan_container_names: vec![
                "django".into(),
                "celery-worker".into(),
                "celery-beat".into(),
            ],
            fe_repo_key: "frontend".into(),
            be_repo_key: "backend".into(),
            master_branch: "master".into(),
            master_be_url: "http://localhost:8000".into(),
            fe_proxy_env_var: "NL_PROXY_TARGET".into(),
            fe_port_start: 3000,
            // 8000 is reserved for the main stack's django.
            be_port_start: 8001,
            fe_command_template: "yarn install && PORT={port} yarn dev".into(),
            be_command_template: "docker compose up".into(),
            compose_project_template: "nl-backend-{short}".into(),
            be_container_template: "django-{short}".into(),
            inactive_services: vec![
                "worker".into(),
                "beat".into(),
                "postgres".into(),
                "redis".into(),
                "mailhog".into(),
                "pgadmin".into(),
            ],
            env_symlinks: vec![".env".into()],
            bind_mounts: vec![BindMount {
                host_path: main_stack_dir.join("serviceaccount.json"),
                container_path: "/app/serviceaccount.json".into(),
            }],
            worktree_root: format!("{home}/newlantern/tethys-newlantern").into(),
        }
    }

    /// Substitute `{short}` into the compose project name.
    pub fn compose_project(&self, short: &str) -> String {
        self.compose_project_template.replace("{short}", short)
    }

    /// Substitute `{short}` into the BE container name.
    pub fn be_container(&self, short: &str) -> String {
        self.be_container_template.replace("{short}", short)
    }

    /// Substitute `{port}` into the FE command.
    pub fn fe_command(&self, port: u16) -> String {
        self.fe_command_template.replace("{port}", &port.to_string())
    }
}
