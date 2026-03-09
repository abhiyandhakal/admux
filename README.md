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
- a `crossterm` interactive attach loop with pane borders, a rich status line, and tmux-style leader keys

## Current scope

Implemented now:

- create, list, attach, kill, and send keys to sessions
- split panes horizontally and vertically
- create, list, cycle, and select windows
- list panes within the active window
- daemon autostart from `admux`
- session state stored in the daemon
- PTY-backed command execution per pane
- multi-pane `crossterm` rendering with borders and per-pane cursors
- leader-key commands for split, window navigation, pane focus, pane resize, and detach
- mouse focus, drag-selection copy, wheel scroll, and border resize
- unit, integration, and binary smoke coverage

Not finished yet:

- rename-window command and help overlay
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
- a direct shell-level smoke flow recorded in [`docs/implementation-log.md`](/home/abhiyan/coding/projects/admux/docs/implementation-log.md)

## More detail

- Detailed engineering log: [`docs/implementation-log.md`](/home/abhiyan/coding/projects/admux/docs/implementation-log.md)
- Detailed status and module overview: [`docs/detailed-status.md`](/home/abhiyan/coding/projects/admux/docs/detailed-status.md)
