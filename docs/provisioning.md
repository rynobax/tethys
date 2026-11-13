# Provisioning

## Setup queue

Provisioning runs one workspace at a time (`provision_queue.rs`, a one-permit semaphore, FIFO). Five parallel clones and installs pushed setup scripts past `setup_timeout_secs`, and a timeout rolls the workspace back. Every path takes it: the create dialog, adding repos, handoffs.

A waiting job is `WorkspaceStatus::Queued`. `provision_workspace` re-checks `deleted_at` on getting the slot and leaves a called-off job `Queued`, not `CreationFailed`. The queue isn't persisted.

No provisioning log is a modal: a modal froze the app behind a job that was often just queued. Runs live in `App` (`addRepoRuns`, like `CreationRunner`) so the log keeps accumulating with nothing mounted.

## Per-worktree env

Worktrees sit in folders named after their repo, and Docker Compose names projects after the folder, so every workspace shared one Compose project. A `docker compose down -v` in one deleted the main checkout's database on 2026-11-03.

A repo's `[repo.env]` in `repos.toml` is upserted into the worktree's `.env` after `copy_files` and exported to the setup script. Values take `{repo}` and `{workspace}` (lowercased, Compose-safe). nl-backend sets `COMPOSE_PROJECT_NAME = "{repo}-{workspace}"`.

`teardown_script` runs on purge with the same rendered env, which depends only on the path, so it targets the right project even for worktrees whose `.env` predates the table. Existing worktrees were deliberately not backfilled. nl-backend's teardown uses `-p "${COMPOSE_PROJECT_NAME:?}"` so it can never fall back to the shared project. Fixed `container_name`s and host ports still collide across workspaces.
