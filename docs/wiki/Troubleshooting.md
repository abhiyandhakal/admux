# Troubleshooting

## `admux` feels stale after a rebuild

Run:

```bash
admux up --rebuild
```

This rebuilds from `admux.toml` and ignores the local snapshot sidecar.

## My saved workspace restored old terminal content

That is expected. Snapshot restore seeds recent terminal state first and then reruns the saved command.

## `admux save` wrote files into the wrong directory

`admux save` writes into the session directory, not the directory where you run the command.

## I changed config and nothing happened

Run:

```bash
admux reload-config
```

This reloads runtime UI/keybindings and future creation defaults.

## Pane numbering looks different from tmux

Pane numbers are window-local in `admux`. Each window starts at pane `0`.

## Do I need to run `admuxd` manually?

No for normal usage. `admux` autostarts the daemon when needed.
