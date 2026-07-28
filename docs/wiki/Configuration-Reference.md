# Configuration Reference

Global config lives at:

```text
~/.config/admux/config.toml
```

`admux` keeps config intentionally grouped and typed.

## Main Sections

- `[ui]`
- `[ui.status]`
- `[ui.dividers]`
- `[ui.theme.*]`
- `[mouse]`
- `[clipboard]`
- `[behavior]`
- `[defaults.session]`
- `[defaults.window]`
- `[keys]`
- `[keys.normal]`
- `[keys.leader]`
- `[keys.copy_mode]`

## Example

```toml
[ui]
status_position = "bottom"

[ui.status]
show_sessions = true
show_window_list = true
show_host = true
show_clock = true

[mouse]
enabled = true
focus_on_click = true
selection_copy = true
border_resize = true
wheel_scroll = true

[clipboard]
# The default is "osc52". For a local clipboard integration, configure an
# explicit command that accepts copied text on stdin.
backend = "external-command"
command = ["wl-copy", "--type", "text/plain"]

[behavior]
default_shell = "/bin/zsh"
scrollback_lines = 10000
workspace_snapshot_lines = 500
resize_step = 25
copy_page_size = 20

[defaults.session]
name_prefix = "work"

[defaults.window]
shell_name = "shell"
use_command_name = true

[keys]
leader = "Ctrl-b"
```

## Notes

- `admux reload-config` reloads UI/keybinding behavior plus future creation defaults
- duplicate key bindings and invalid key names fail explicitly
- legacy aliases still load where compatibility is supported
- already-running pane processes are not retroactively respawned on config reload
