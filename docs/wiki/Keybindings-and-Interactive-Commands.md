# Keybindings and Interactive Commands

`admux` uses a tmux-style leader workflow by default.

## Default Leader

- `Ctrl-b`

## Everyday Keys

- `Ctrl-b %` split vertically
- `Ctrl-b "` split horizontally
- `Ctrl-b 0..9` select windows by index
- `Ctrl-b h/j/k/l` move pane focus
- `Ctrl-b H/J/K/L` resize the active pane
- `Ctrl-b c` create a new window
- `Ctrl-b n` / `p` next or previous window
- `Ctrl-b x` kill the active pane
- `Ctrl-b d` detach

## Prompt and Views

- `Ctrl-b :` open the command prompt
- `Ctrl-b s` open the session and window chooser
- `Ctrl-b ?` open the help overlay

The command prompt supports tmux-style interactive commands such as:

- `save-session`
- `reload-config`
- `split-window`
- `new-window`
- `select-window`
- `list-sessions`
- `list-windows`
- `list-panes`

## Copy Mode and Buffers

- `Ctrl-b [` enter copy mode
- `Ctrl-b ]` paste the top buffer
- `Ctrl-b #` list buffers
- `Ctrl-b -` delete the top buffer
- `Ctrl-b =` choose a buffer

Mouse selection and copy-mode yank both feed the global paste-buffer store.
