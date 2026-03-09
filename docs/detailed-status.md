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
- VT100-backed screen parsing of PTY output
- pane output capture for attach previews from parsed screen state
- kill path that terminates pane processes when a session is removed
- resize path from the client into the active PTY

### Layout and pane foundations

- pane identifiers and pane snapshots in `src/pane.rs`
- layout tree and split model in `src/layout.rs`
- session model that owns layout state and panes in `src/session.rs`

### Client interaction

- non-interactive attach path that prints session name and pane preview
- interactive `crossterm` attach path with:
  - alternate screen entry
  - custom redraws
  - reverse-video status line
  - `Ctrl-b d` detach
  - direct key forwarding to the active PTY
  - control-key forwarding such as `Ctrl-l`
  - arrow/home/end/delete forwarding
  - resize propagation from the current terminal into the active PTY

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

### Manual

The following direct shell-level smoke path was run against built binaries:

1. Start `target/debug/admuxd serve --socket <temp-socket>`
2. Run `target/debug/admux new --name demo -- sh -lc "printf demo-ok"`
3. Poll `target/debug/admux attach demo` until pane output appears

Observed result:

- session creation succeeded
- attach succeeded
- pane output `demo-ok` was observed from the PTY-backed process

## Commit history so far

- `afdc324` `chore: bootstrap admux cargo project`
- `cd41cc2` `chore: ignore cargo build artifacts`
- `9b9eb6e` `feat: add config loading and ipc foundations`
- `29eb401` `feat: add daemon lifecycle and core cli commands`
- `da946e8` `feat: add pty-backed sessions and interactive attach`

## Known gaps

- no exposed pane-splitting command yet, even though layout support exists internally
- no exposed window management yet
- copy mode is not wired into the interactive client
- mouse events are ignored in the interactive client
- attach currently targets a single active pane preview rather than a full multi-pane compositor
- session state is in-memory only while `admuxd` is alive

## Module map

- `src/cli.rs`: clap CLI definitions
- `src/client.rs`: client request flow, autostart, attach behavior
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
