# admux

`admux` is an opinionated terminal multiplexer written in Rust.

The repository currently contains a working foundation with:

- `admux` as the user-facing CLI
- `admuxd` as the background daemon
- TOML config loading from `~/.config/admux/config.toml`
- Unix socket client/daemon IPC
- PTY-backed pane and window processes
- ratio-based split layouts
- VT100-backed screen parsing and clipped pane rendering
- a `crossterm` interactive attach loop with split separators, a status-row command prompt, a choose-tree session view, and tmux-style leader keys

## Current scope

Implemented now:

- create, list, attach, kill, and send keys to sessions
- split panes horizontally and vertically
- create, list, cycle, and select windows
- list panes within the active window
- rename the active window from the interactive prompt
- daemon autostart from `admux`
- session state stored in the daemon
- PTY-backed command execution per pane
- multi-pane `crossterm` rendering with no persistent pane borders in normal mode and per-pane cursors
- leader-key commands for split, window navigation, pane focus, pane resize, and detach
- `Ctrl-b 0` through `Ctrl-b 9` for window index selection
- `Ctrl-b :` for a tmux-style status-row command prompt with command completion
- `Ctrl-b s` for a tmux-like session/window chooser with stacked pane previews
- `Ctrl-b ?` for a full-screen help overlay
- chooser-local search with `Ctrl-s`, repeat with `n`/`N`, and expand/collapse-all with `Alt-+` / `Alt--`
- mouse focus, drag-selection copy, wheel scroll, and border resize
- unit, integration, and binary smoke coverage

Not finished yet:

- copy-mode UI beyond drag-selection
- restart recovery or persistent session metadata

## Build and run

```bash
cargo build
cargo test
```

Examples:

```bash
cargo run --bin admux -- new --name work -- sh
cargo run --bin admux -- ls
cargo run --bin admux -- split-pane work --vertical
cargo run --bin admux -- list-panes work
cargo run --bin admux -- new-window work --name logs -- sh -lc "tail -f /var/log/messages"
cargo run --bin admux -- list-windows work
cargo run --bin admux -- attach work
cargo run --bin admux -- kill work
```

Interactive defaults:

- `Ctrl-b %` vertical split
- `Ctrl-b "` horizontal split
- `Ctrl-b 0` through `Ctrl-b 9` select windows by index
- `Ctrl-b :` open the command prompt
- `Ctrl-b s` open the session chooser
- `Ctrl-b ?` open help
- `Ctrl-b d` detach

Run the daemon explicitly:

```bash
cargo run --bin admuxd -- serve
```

## Config

Configuration is read from `~/.config/admux/config.toml`.

Example:

```toml
[ui]
status_position = "bottom"
show_pane_labels = true

[mouse]
enabled = true

[behavior]
scrollback_lines = 10000

[keys]
leader = "Ctrl-b"
```

## Verification

The repository is developed in small TDD-oriented slices. Current verification includes:

- `cargo test`
- integration tests that execute `admux` and `admuxd`
- a direct shell-level smoke flow covering nested pane splits and window creation recorded in [`docs/implementation-log.md`](/home/abhiyan/coding/projects/admux/docs/implementation-log.md)

## More detail

- Detailed engineering log: [`docs/implementation-log.md`](/home/abhiyan/coding/projects/admux/docs/implementation-log.md)
- Detailed status and module overview: [`docs/detailed-status.md`](/home/abhiyan/coding/projects/admux/docs/detailed-status.md)
