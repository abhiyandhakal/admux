# CLI Reference

## Core Session Commands

```bash
admux new [--name <session>] [--detach] [--cwd <dir>] [command...]
admux attach [session]
admux ls
admux kill <session>
admux save [session]
admux up [path]
admux up --rebuild [path]
```

Notes:

- `admux new` uses the current shell directory by default
- `admux new /path/to/project` uses that directory as the session cwd
- `admux up` reads `./admux.toml` when no path is given
- `admux save` writes workspace files into the session directory, not the caller's current directory

## Window and Pane Commands

```bash
admux split-pane <session> [--horizontal|--vertical]
admux new-window <session> [--name <window>] [command...]
admux list-windows <session>
admux list-panes <session>[:window]
admux select-window <session>:<window>
admux select-pane <session>:<window>.<pane>
admux resize-pane <session>:<window>.<pane> --left|--right|--up|--down <cells>
admux kill-window <session>:<window>
admux kill-pane <session>:<window>.<pane>
```

Pane numbering is window-local. Each new window starts at pane `0`.

## Buffers and Input

```bash
admux send-keys <target> <keys...>
admux list-buffers
admux show-buffer [name]
admux set-buffer <text>
admux paste-buffer [--target <session>[:window[.pane]]] [name]
admux delete-buffer [name]
admux save-buffer <path> [name]
admux load-buffer <path> [name]
```

## Config Reload

```bash
admux reload-config
```

This reloads runtime UI/keybinding behavior plus future creation defaults without restarting the daemon.
