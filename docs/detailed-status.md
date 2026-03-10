# Detailed Status

## Implemented

### Binaries

- `admux`: clap-based client CLI
- `admuxd`: daemon entrypoint

### Configuration and paths

- TOML config parsing with defaults in `src/config.rs`
- XDG-aware path resolution with `ADMUX_SOCKET` and `ADMUX_CONFIG` overrides in `src/paths.rs`

### Protocol and daemon

- typed request/response IPC in `src/ipc.rs`
- Unix domain socket daemon in `src/server.rs`
- daemon autostart from the client when the socket is missing

### Session and process model

- session store in the daemon
- PTY-backed command spawning through `portable-pty`
- session now owns ordered windows and each window owns a split-tree of panes
- VT100-backed screen parsing of PTY output
- pane output capture for clipped pane regions and attach previews from parsed screen state
- pane/window/session cleanup when child processes exit
- resize path from the client into all pane PTYs plus stored viewport geometry
- PTY resize handling now keeps the current screen stable while shrinking and rebuilds from raw history when panes expand again

### Layout and pane foundations

- pane and window identifiers plus render rects in `src/pane.rs`
- ratio-based layout tree, directional focus, and pane removal collapse in `src/layout.rs`
- session/window runtime model in `src/session.rs`
- window summaries in `src/window.rs`

### Client interaction

- non-interactive attach path that prints session name and pane preview
- interactive `crossterm` attach path with:
  - alternate screen entry
  - multi-pane redraws from daemon snapshots
  - internal pane dividers with joined Unicode junction glyphs
  - reverse-video status line with window list
  - `Ctrl-b d` detach
  - `Ctrl-b 0` through `Ctrl-b 9` window index selection
  - `Ctrl-b :` status-row command prompt with tmux-style command names and completion
  - `Ctrl-b s` tmux-like chooser with a stacked session list and pane preview grid
  - chooser starts collapsed and supports `Tab` plus `+` / `-` expand-collapse controls
  - `Ctrl-b ?` full-screen help overlay
  - chooser-local search on `Ctrl-s`, repeat on `n` / `N`, and expand/collapse-all on `Alt-+` / `Alt--`
  - direct key forwarding to the active PTY
  - control-key forwarding such as `Ctrl-l`
  - arrow/home/end/delete forwarding
  - split/window/focus/resize leader-key actions
  - resize propagation from the current terminal into pane PTYs
  - mouse focus, selection copy, wheel scroll, and border resize
  - nested `admux new` redirects the outer client to the new session instead of nesting another fullscreen client in the pane

### Input and copy-mode foundations

- leader-state input handler in `src/input.rs`
- copy-mode search helper in `src/copy_mode.rs`

## Verification performed

### Automated

- CLI parser tests
- config parsing tests
- path resolution tests
- protocol serialization tests
- layout tests
- PTY capture tests
- session store tests
- render tests
- binary smoke tests that execute `admux` and `admuxd`
- direct binary smoke for split-pane and new-window command flow
- direct binary smoke for nested mixed-axis splits
- prompt command parser and completion tests
- chooser search helper tests

### Manual

The following direct shell-level smoke path was run against built binaries:

1. Start `target/debug/admuxd serve --socket <temp-socket>`
2. Run `ADMUX_SOCKET=<temp-socket> target/debug/admux new -d --name work -- sh -lc "printf base; sleep 5"`
3. Run `ADMUX_SOCKET=<temp-socket> target/debug/admux split-pane work --vertical`
4. Run `ADMUX_SOCKET=<temp-socket> target/debug/admux list-panes work`
5. Run `ADMUX_SOCKET=<temp-socket> target/debug/admux new-window work --name logs -- sh -lc "printf logs; sleep 5"`
6. Run `ADMUX_SOCKET=<temp-socket> target/debug/admux list-windows work`

Observed result:

- session creation succeeded
- pane split succeeded
- pane list showed two panes in the first window
- second window creation succeeded
- window list showed the new active `logs` window
- prompt parser and separator-only renderer tests passed under `cargo test`
- mixed-axis divider glyph tests and PTY resize-history tests passed under `cargo test`

## Commit history so far

- `afdc324` `chore: bootstrap admux cargo project`
- `cd41cc2` `chore: ignore cargo build artifacts`
- `9b9eb6e` `feat: add config loading and ipc foundations`
- `29eb401` `feat: add daemon lifecycle and core cli commands`
- `da946e8` `feat: add pty-backed sessions and interactive attach`
- `a15bf48` `feat: add tmux-style prompt and chooser`
- `f5b5f46` `feat: add help overlay and chooser search`
- pending layout regression fix commit in current worktree
- pending resize and divider stability commit in current worktree
- pending nested session redirect commit in current worktree

## Known gaps

- copy mode is still drag-selection focused rather than a full modal copy-mode UI
- session state is in-memory only while `admuxd` is alive

## Module map

- `src/cli.rs`: clap CLI definitions
- `src/client.rs`: client request flow, autostart, attach behavior
- `src/commands.rs`: tmux-style interactive command parsing and completion
- `src/config.rs`: TOML config types and loaders
- `src/ipc.rs`: request/response protocol
- `src/paths.rs`: runtime/config path resolution
- `src/server.rs`: daemon loop and session request handling
- `src/session.rs`: session runtime model
- `src/pty.rs`: PTY-backed pane process management
- `src/layout.rs`: layout tree
- `src/render.rs`: custom `crossterm` drawing
- `src/input.rs`: leader-mode input handling
- `src/copy_mode.rs`: copy/search helper
- `src/test_support.rs`: shared test helpers
