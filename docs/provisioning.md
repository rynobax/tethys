# Provisioning

Provisioning is serialized because parallel clones and installs pushed setup scripts past `setup_timeout_secs`, and a timeout rolls the whole workspace back. Nothing about it is a modal: a job can sit queued for minutes, and a modal froze the app behind it.

## Per-worktree env

Docker Compose names a project after its folder, and every worktree folder is named after its repo. So every workspace shared one project, and a `docker compose down -v` in one deleted the main checkout's database on 2026-11-03. `[repo.env]` sets `COMPOSE_PROJECT_NAME` per workspace.

Existing worktrees were deliberately not backfilled: their running stacks would be orphaned. That's why nl-backend's teardown passes `-p "${COMPOSE_PROJECT_NAME:?}"`; falling back to the folder name would hit the shared project again. Fixed `container_name`s and ports still collide.
