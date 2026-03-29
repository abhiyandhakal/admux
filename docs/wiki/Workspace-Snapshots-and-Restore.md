# Workspace Snapshots and Restore

`admux save` writes two kinds of workspace state:

- `admux.toml` for shareable structure
- `.admux/snapshot.json` for local best-effort restore

## What Save Captures

- session, window, and pane layout
- active window and active panes
- pane cwd and command argv
- recent terminal state and bounded scrollback tail

## What Restore Does

When no live workspace exists, `admux up` can:

1. rebuild the structure from `admux.toml`
2. seed panes from `.admux/snapshot.json`
3. rerun the saved pane commands

This is best-effort restore. It is meant to bring terminal context back quickly, not to checkpoint and resume process memory exactly.

## What `--rebuild` Means

`admux up --rebuild` ignores `.admux/snapshot.json` and rebuilds from `admux.toml` only.

## Local Sidecar Files

`admux save` also writes:

- `.admux/.gitignore`

That keeps the snapshot sidecar local by default while allowing `admux.toml` to remain the shared source of truth.
