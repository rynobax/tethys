# Provisioning

Provisioning is serialized because parallel clones and installs pushed setup scripts past `setup_timeout_secs`, and a timeout rolls the whole workspace back. Nothing about it is a modal: a job can sit queued for minutes, and a modal froze the app behind it.

## The spare

Every workspace gets the same repos: each one in `repos.toml` not marked `enabled = false`. Because the set never varies, Tethys builds the next workspace's worktrees ahead of time in `<worktree_root>/.spare`, detached and with their setup scripts already run. A create moves them into place (`git worktree move`), switches each to the branch, and runs the setup script again. nl-frontend's `yarn install` took 46s in a fresh worktree and 6s in a claimed spare (2026-11-17).

Copying `node_modules` from somewhere instead was measured and lost. Even a copy-on-write `clonefile` of its 136k files took 21s, and Yarn re-links whatever is stale, so the copy only pays off if its source is current. pnpm would write 9% fewer files. The cost is writing the files, which any package manager pays.

- A fresh branch starts from the clone's HEAD after the pull, not from the commit the spare was cut at, so a stale spare can't hand out old code.
- The spare's build holds the setup queue. A create arriving mid-build waits for it rather than starting over, since it's already part of the way there. An `add_repos` can end up queued behind a spare build.
- `.tethys-spare.json` is written last and removed when a create starts taking the spare apart. At boot, a spare with a marker that matches the enabled repos is kept; anything else in `.spare` is discarded and rebuilt.
- A create called off while queued gives the spare back untouched.
- A failed build leaves no spare, and creates start from scratch. The next create retries the build.

## Per-worktree env

Docker Compose names a project after its folder, and every worktree folder is named after its repo. So every workspace shared one project, and a `docker compose down -v` in one deleted the main checkout's database on 2026-11-03. `[repo.env]` sets `COMPOSE_PROJECT_NAME` per workspace.

Existing worktrees were deliberately not backfilled: their running stacks would be orphaned. That's why nl-backend's teardown passes `-p "${COMPOSE_PROJECT_NAME:?}"`; falling back to the folder name would hit the shared project again. Fixed `container_name`s and ports still collide.
