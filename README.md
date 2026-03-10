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
- a `crossterm` interactive attach loop with Unicode pane dividers, a status-row command prompt, a choose-tree session view, and tmux-style leader keys
- a tmux-style one-row statusline with session on the left, a centered window list, and host/date/time on the right

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
- multi-pane `crossterm` rendering with internal pane dividers, joined junction glyphs, and per-pane cursors
- leader-key commands for split, window navigation, pane focus, pane resize, and detach
- `Ctrl-b 0` through `Ctrl-b 9` for window index selection
- `Ctrl-b :` for a tmux-style status-row command prompt with command completion
- `Ctrl-b s` for a tmux-like session/window chooser with stacked pane previews
- `Ctrl-b ?` for a full-screen help overlay
- chooser-local search with `Ctrl-s`, repeat with `n`/`N`, and expand/collapse-all with `Alt-+` / `Alt--`
- chooser collapsed by default with `Tab` and `+` / `-` expand-collapse controls
- mouse focus, drag-selection copy, wheel scroll, and border resize
- modal copy mode with pane navigation, scrollback paging, selection, and yank
- persistent session/window/pane metadata across daemon restarts, with stale session listing when processes are gone
- resize handling that preserves pane state while shrinking and restores PTY history when panes expand again
- unit, integration, and binary smoke coverage

Not finished yet:

- live pane/process recovery across daemon restart

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
- `Ctrl-b [` enter copy mode
- `Ctrl-b s` open the session chooser
- `Ctrl-b ?` open help
- `Ctrl-b d` detach

Inside an existing `admux` pane, running `admux new` creates a sibling session and switches the current client to it on the next attach poll. It does not nest a second fullscreen `admux` UI inside the pane unless `-d` is used.

Run the daemon explicitly:

```bash
cargo run --bin admuxd -- serve
```

## Config

Configuration is read from `~/.config/admux/config.toml`.
Persistent session metadata is stored in `~/.config/admux/state.json`.

Example:

```toml
[ui]
status_position = "bottom"
show_pane_labels = true
status_clock = true
status_show_pane = true
status_show_window_list = true
status_style = "tmux-plus"

[mouse]
enabled = true

[behavior]
scrollback_lines = 10000

[keys]
leader = "Ctrl-b"
```

Statusline defaults:

- one row only, at the top or bottom according to `status_position`
- left segment with tmux-style `[session]`
- centered window list with `*` for current and `-` for last-selected windows
- right segment with short hostname and local date/time
- prompt, copy mode, chooser, and help temporarily repurpose the row instead of preserving the normal status content

Compatibility note:

- `status_show_pane`, `status_show_window_list`, and `status_style` are still accepted in config so older files keep loading, but the renderer now follows the tmux-style layout regardless

## Verification

The repository is developed in small TDD-oriented slices. Current verification includes:

- `cargo test`
- integration tests that execute `admux` and `admuxd`
- a direct shell-level smoke flow covering nested pane splits and window creation recorded in [`docs/implementation-log.md`](/home/abhiyan/coding/projects/admux/docs/implementation-log.md)

## More detail

- Detailed engineering log: [`docs/implementation-log.md`](/home/abhiyan/coding/projects/admux/docs/implementation-log.md)
- Detailed status and module overview: [`docs/detailed-status.md`](/home/abhiyan/coding/projects/admux/docs/detailed-status.md)
