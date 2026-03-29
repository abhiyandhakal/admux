# Architecture

`admux` is split into three runtime roles:

## 1. `admux`

The user-facing CLI and interactive client.

Responsibilities:

- parse commands
- autostart and connect to the daemon
- run the interactive attach loop
- render panes, status row, chooser, help, and copy mode
- send direct input to pane helpers on the hot path

## 2. `admuxd`

The background daemon.

Responsibilities:

- own sessions, windows, panes, and layout state
- coordinate workspace manifests and saves
- manage paste buffers
- persist runtime metadata
- reconnect to pane helpers after daemon restart

## 3. `admux-pane`

A per-pane helper process.

Responsibilities:

- own the PTY and child process
- maintain terminal parser state and scrollback
- export persistent snapshots
- survive daemon restarts so live pane recovery works

## Storage Model

- `admux.toml`: shareable workspace structure
- `.admux/snapshot.json`: local best-effort restore state
- `~/.config/admux/config.toml`: global user config
- `~/.config/admux/state.json`: daemon metadata
