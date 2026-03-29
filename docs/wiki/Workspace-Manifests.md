# Workspace Manifests

`admux.toml` defines a project-scoped workspace layout.

## Minimal Example

```toml
version = 1

[workspace]
name = "quiz-backend"
cwd = "."

[[windows]]
name = "editor"
root = { cwd = ".", command = ["nvim"] }

[[windows]]
name = "server"
root = { cwd = ".", command = ["cargo", "run"] }

[[windows.splits]]
target = 0
direction = "vertical"
size = 0.5
cwd = "."
command = ["cargo", "test", "--", "--watch"]
```

## Rules

- `admux up` reads `./admux.toml` when no path is given
- one manifest defines one session
- relative `cwd` values resolve from the manifest directory
- commands are argv arrays, not shell strings
- use `["sh", "-lc", "..."]` when shell parsing is needed

## Lifecycle

- `admux up` creates the workspace if it does not exist
- rerunning `admux up` attaches to the existing mapped workspace
- `admux up --rebuild` rebuilds the workspace from the manifest
- `admux save` writes a live session back into `admux.toml`
