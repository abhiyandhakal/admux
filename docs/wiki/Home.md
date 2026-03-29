# admux Wiki

`admux` is a shared terminal workspace system written in Rust.

It combines tmux-style terminal multiplexing with project-scoped workspaces, reproducible layouts from `admux.toml`, and best-effort local snapshot restore.

## Start Here

- [CLI Reference](CLI-Reference)
- [Keybindings and Interactive Commands](Keybindings-and-Interactive-Commands)
- [Workspace Manifests](Workspace-Manifests)
- [Workspace Snapshots and Restore](Workspace-Snapshots-and-Restore)
- [Configuration Reference](Configuration-Reference)
- [Architecture](Architecture)
- [Troubleshooting](Troubleshooting)

## Core Workflow

```bash
admux new
admux up
admux save
admux ls
admux attach work
```

Key points:

- `admux` autostarts the daemon; normal usage does not require launching `admuxd` manually
- `admux new` starts in the current shell directory by default
- `admux up` reads `./admux.toml`
- `admux save` writes `admux.toml` and `.admux/snapshot.json` into the session directory

## Product Model

`admux` is built around three layers:

- live terminal sessions with sessions, windows, panes, buffers, and copy mode
- shareable workspace manifests in `admux.toml`
- local snapshot sidecars in `.admux/` for best-effort restore
