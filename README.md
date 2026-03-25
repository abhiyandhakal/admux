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
- a tmux-style one-row statusline with the session list on the left, a centered window list, and host/date/time on the right
- `admux up` workspace manifests from a project-local `admux.toml`

## Current scope

Implemented now:

- create, list, attach, kill, and send keys to sessions
- split panes horizontally and vertically
- create, list, cycle, and select windows
- list panes within the active window
- pane numbering is window-local and starts at `0` in each window
- rename the active window from the interactive prompt
- daemon autostart from `admux`
- explicit workspace sharing with `admux up` and `admux.toml`
- `admux save` to export a live session back into `admux.toml` in the session directory
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
- daemon-owned paste buffers with CLI/prompt support for list/show/set/delete/paste/save/load
- `Ctrl-b ]`, `Ctrl-b #`, `Ctrl-b -`, and `Ctrl-b =` for top-buffer paste, list, delete, and choose-buffer flows
- keyboard and mouse copy now create paste buffers before mirroring to OSC52
- persistent session/window/pane metadata across daemon restarts, with stale session listing when processes are gone
- live pane/process recovery across `admuxd` restart through per-pane helper processes
- resize handling that preserves pane state while shrinking and restores PTY history when panes expand again
- unit, integration, and binary smoke coverage

Not finished yet:

- fuller choose-tree / chooser parity
- broader tmux default-key and command-surface parity

## Build and run

```bash
cargo build
cargo test
```

Examples:

```bash
cargo run --bin admux -- new --name work -- sh
cargo run --bin admux -- new -d /path/to/project
cargo run --bin admux -- up
cargo run --bin admux -- up /path/to/admux.toml
cargo run --bin admux -- save
cargo run --bin admux -- save work
cargo run --bin admux -- ls
cargo run --bin admux -- split-pane work --vertical
cargo run --bin admux -- list-panes work
cargo run --bin admux -- new-window work --name logs -- sh -lc "tail -f /var/log/messages"
cargo run --bin admux -- list-windows work
cargo run --bin admux -- set-buffer "hello world"
cargo run --bin admux -- list-buffers
cargo run --bin admux -- paste-buffer --target work
cargo run --bin admux -- attach work
cargo run --bin admux -- kill work
```

Interactive defaults:

- `Ctrl-b %` vertical split
- `Ctrl-b "` horizontal split
- `Ctrl-b 0` through `Ctrl-b 9` select windows by index
- `Ctrl-b :` open the command prompt
- `Ctrl-b [` enter copy mode
- `Ctrl-b ]` paste the top buffer
- `Ctrl-b #` list buffers in the status row
- `Ctrl-b -` delete the top buffer
- `Ctrl-b =` open the buffer chooser
- `Ctrl-b s` open the session chooser
- `Ctrl-b ?` open help
- `Ctrl-b d` detach

Inside an existing `admux` pane, running `admux new` creates a sibling session and switches the current client to it on the next attach poll. It does not nest a second fullscreen `admux` UI inside the pane unless `-d` is used.

Pane target notes:

- pane targets use `session:window.pane`
- `pane` is local to that window, not global across the session
- each new window starts its pane numbering at `0`

`admux new` cwd notes:

- without `--cwd`, new sessions now start in your current shell directory
- `admux new -d /path/to/project` treats the lone directory argument as the session cwd
- if you want to pass a command, keep using `--`, for example `admux new -- sh`

Run the daemon explicitly:

```bash
cargo run --bin admuxd -- serve
```

## Config

Configuration is read from `~/.config/admux/config.toml`.
Persistent session metadata is stored in `~/.config/admux/state.json`.
Project-local workspace manifests are read from `./admux.toml` when using `admux up`.
Local workspace snapshots are stored in `.admux/snapshot.json`.

Workspace manifest example:

```toml
version = 1

[workspace]
name = "shared-work"
cwd = "."
active_window = 0

[[windows]]
name = "editor"
root = { cwd = ".", command = ["nvim"] }

[[windows.splits]]
target = 0
direction = "vertical"
size = 0.5
cwd = "."
command = ["cargo", "watch", "-x", "test"]

[[windows]]
name = "shell"
root = { cwd = ".", command = ["zsh"] }
```

Workspace notes:

- `admux up` looks only for `./admux.toml` when no path is given
- one `admux.toml` defines one shared session
- rerunning `admux up` attaches to the existing mapped workspace session instead of recreating it
- when no live workspace exists, `admux up` can seed panes from `.admux/snapshot.json` and then rerun the pane commands
- `admux up --rebuild` kills and recreates the mapped workspace from the manifest only; it ignores `.admux/snapshot.json`
- manifest `cwd` values are resolved relative to the manifest directory
- pane commands are argv arrays; use `["sh", "-lc", "..."]` for shell strings
- `admux save` writes `admux.toml` into the session directory, not the caller's current directory
- `admux save` also writes `.admux/snapshot.json` plus `.admux/.gitignore` in the session directory
- `admux save` with no session argument uses the current `ADMUX_SESSION` when invoked inside `admux`, otherwise it falls back to the daemon's current session resolution
- the command prompt also supports `save-session`, so `Ctrl-b :save-session` saves the current session workspace and snapshot
- snapshots are local sidecar state intended for best-effort restore, not exact process checkpointing

Example:

```toml
[ui]
status_position = "bottom"

[ui.status]
show_sessions = true
show_window_list = true
show_host = true
show_clock = true

[ui.dividers]
charset = "unicode"
highlight_active = true

[ui.theme.active_window]
bold = true
reverse = true

[mouse]
enabled = true
focus_on_click = true
selection_copy = true
border_resize = true
wheel_scroll = true

[behavior]
scrollback_lines = 10000
workspace_snapshot_lines = 500
default_shell = "/bin/zsh"
resize_step = 25
copy_page_size = 20

[defaults.session]
name_prefix = "work"

[defaults.window]
shell_name = "shell"
use_command_name = true

[keys]
leader = "Ctrl-b"

[keys.leader]
reload_config = "r"
new_window = "c"

[keys.copy_mode]
copy_yank = "Enter"
```

Config notes:

- primary schema is grouped TOML: `[ui.status]`, `[ui.dividers]`, `[ui.theme.*]`, `[mouse]`, `[behavior]`, `[defaults.*]`, and per-mode key tables under `[keys.*]`
- legacy aliases like `[ui] status_clock = true` and `[keys] leader = "Ctrl-b"` still load for compatibility
- key bindings are typed and validated at load time; duplicate bindings, unknown actions, and invalid key names fail explicitly
- `admux reload-config` now reloads both the daemon-side creation defaults and the client-side interactive key/UI config without restarting the daemon
- future sessions/windows/panes use the reloaded defaults; already-running pane processes are not retroactively respawned

Statusline defaults:

- one row only, at the top or bottom according to `status_position`
- left segment with the session list, highlighting the current session
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
