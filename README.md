# admux

`admux` is an opinionated terminal multiplexer written in Rust.

The repository currently contains a working foundation with:

- `admux` as the user-facing CLI
- `admuxd` as the background daemon
- TOML config loading from `~/.config/admux/config.toml`
- Unix socket client/daemon IPC
- PTY-backed pane processes
- non-interactive attach previews
- a `crossterm` interactive attach loop with a status line and `Ctrl-b d` detach

## Current scope

Implemented now:

- create, list, attach, kill, and send keys to sessions
- daemon autostart from `admux`
- session state stored in the daemon
- PTY-backed command execution per session
- custom `crossterm` rendering for interactive attach
- leader-key input state and detach handling
- unit, integration, and binary smoke coverage

Not finished yet:

- multi-pane splitting from the CLI or interactive client
- multi-window navigation
- copy-mode UI beyond the search helper foundation
- mouse actions in the interactive client
- restart recovery or persistent session metadata

## Build and run

```bash
cargo build
cargo test
```

Examples:

```bash
cargo run --bin admux -- new --name work -- sh -lc "printf hello"
cargo run --bin admux -- ls
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
