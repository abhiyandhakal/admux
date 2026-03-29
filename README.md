# admux

`admux` is a shared terminal workspace system written in Rust.

It gives a project a reproducible terminal layout, keeps sessions alive in the background, restores workspace state from files, and still feels familiar if you already know tmux.

## What It Is

`admux` is built around three ideas:

- a project can define its terminal workspace in `admux.toml`
- that workspace can be started with `admux up` and shared across machines or teammates
- the live session can be saved back into files with `admux save`, including a local snapshot for best-effort restore

It includes the usual multiplexer pieces too: sessions, windows, panes, split layouts, copy mode, a command prompt, buffer workflows, mouse support, and tmux-style leader-key navigation.

## Quick Start

Build and test:

```bash
cargo build
cargo test
```

Use the binary directly:

```bash
admux new
admux new --name work
admux up
admux save
admux ls
admux attach work
```

`admux` starts and talks to the background daemon automatically. You do not need to launch `admuxd` yourself for normal use.

If you are running from the repo without installing, substitute `cargo run --bin admux -- ...` for the commands above.

## Core Workflow

Start a fresh session:

```bash
admux new
```

- `admux new` starts in your current shell directory by default
- `admux new /path/to/project` uses that directory as the session cwd
- `admux new -- sh -lc "npm test"` starts the session with an explicit command

Reattach later:

```bash
admux ls
admux attach work
```

Save the current live session back into workspace files:

```bash
admux save
```

Inside `admux`, the prompt also supports:

```text
Ctrl-b :save-session
```

Default interactive essentials:

- `Ctrl-b %` split vertically
- `Ctrl-b "` split horizontally
- `Ctrl-b :` open the command prompt
- `Ctrl-b [` enter copy mode
- `Ctrl-b ]` paste the top buffer
- `Ctrl-b s` open the session and window chooser
- `Ctrl-b d` detach

## Workspace Sharing

`admux` treats `admux.toml` as a project-scoped workspace file.

Minimal example:

```toml
version = 1

[workspace]
name = "quiz-backend"
cwd = "."

[[windows]]
name = "editor"
root = { cwd = ".", command = ["nvim"] }

[[windows]]
name = "server"
root = { cwd = ".", command = ["cargo", "run"] }

[[windows.splits]]
target = 0
direction = "vertical"
size = 0.5
cwd = "."
command = ["cargo", "test", "--", "--watch"]
```

Key behavior:

- `admux up` reads `./admux.toml`
- rerunning `admux up` attaches to the existing mapped workspace instead of recreating it
- `admux up --rebuild` rebuilds from `admux.toml` only
- `admux save` writes `admux.toml` into the session directory, not the directory where you ran the command
- `admux save` also writes local restore state into `.admux/snapshot.json`

Snapshots are best-effort local restore data. They bring back recent terminal state and then rerun the saved pane commands; they are not process checkpointing.

## Configuration

Global config lives at `~/.config/admux/config.toml`.

The config is intentionally small and grouped around a few areas:

- UI and status row
- mouse behavior
- shell, scrollback, and workspace snapshot defaults
- key bindings

Example:

```toml
[ui]
status_position = "bottom"

[ui.status]
show_sessions = true
show_window_list = true
show_host = true
show_clock = true

[mouse]
enabled = true
focus_on_click = true
selection_copy = true
border_resize = true
wheel_scroll = true

[behavior]
default_shell = "/bin/zsh"
scrollback_lines = 10000
workspace_snapshot_lines = 500
resize_step = 25

[keys]
leader = "Ctrl-b"
```

Reload config without restarting:

```bash
admux reload-config
```

## Documentation

The top-level README is meant to stay product-facing and quick to scan.

For deeper documentation, use the GitHub wiki:

- <https://github.com/abhiyandhakal/admux/wiki>

Core wiki pages:

- [Home](https://github.com/abhiyandhakal/admux/wiki)
- [CLI Reference](https://github.com/abhiyandhakal/admux/wiki/CLI-Reference)
- [Keybindings and Interactive Commands](https://github.com/abhiyandhakal/admux/wiki/Keybindings-and-Interactive-Commands)
- [Workspace Manifests](https://github.com/abhiyandhakal/admux/wiki/Workspace-Manifests)
- [Workspace Snapshots and Restore](https://github.com/abhiyandhakal/admux/wiki/Workspace-Snapshots-and-Restore)
- [Configuration Reference](https://github.com/abhiyandhakal/admux/wiki/Configuration-Reference)
- [Architecture](https://github.com/abhiyandhakal/admux/wiki/Architecture)
- [Troubleshooting](https://github.com/abhiyandhakal/admux/wiki/Troubleshooting)

The staged source for those pages is kept in [docs/wiki](/home/abhiyan/coding/projects/admux/docs/wiki).

Engineering records remain in-repo:

- [Implementation log](/home/abhiyan/coding/projects/admux/docs/implementation-log.md)
- [Detailed status](/home/abhiyan/coding/projects/admux/docs/detailed-status.md)
