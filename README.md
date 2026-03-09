# admux

`admux` is an opinionated terminal multiplexer written in Rust.

## Current status

The project is being built in small, test-driven slices. The current repository contains the Cargo package layout, the `admux` and `admuxd` binaries, CLI scaffolding, and the module structure the rest of the implementation will grow into.

## Planned shape

- `admux` is the user-facing CLI.
- `admuxd` is the background daemon.
- Configuration lives in `~/.config/admux/config.toml`.
- Rendering is custom and `crossterm`-based.

## Development workflow

- Use `cargo test` for automated verification.
- Add behavior in focused modules.
- Commit each completed slice with a conventional commit.
