# admux audit repair progress

This is an agent tracking file. Do not commit it.

## Summary through the current worktree

- The imported 151-item source audit is being worked through as small, independently tested reliability batches; the work is on draft PR [#1](https://github.com/abhiyandhakal/admux/pull/1).
- The completed work has hardened daemon/client IPC, state recovery and durability, helper and session lifecycle cleanup, workspace save/restore safety, terminal input/rendering, paste buffers, and copy-mode configuration.
- Recent batches removed large helper-snapshot duplication, made pane restore data avoid `argv` limits, retained configured scrollback across large resizes, made failed split/window creation roll back cleanly, preserved manifest comments and unknown fields on save, and implemented client-local rendered viewports for differently sized attached clients.
- Pane helpers now isolate accepted connections, so an incomplete request can time out in its own bounded worker without freezing the helper's subsequent requests.
- Active-window PTY synchronization is now transactional: helpers are preflighted, successful resizes are rolled back on a later failure, and failed layout resizes restore their prior layout.
- Nested session creation now records the originating interactive client at input submission and redirects only that client, rather than whichever viewer next polls the source session.
- Ordinary interactive input now goes straight to one helper send request; it no longer opens a separate helper connection solely to validate liveness first.
- Chooser and help overlay `border`/`title` configuration is now honored instead of being parsed but inert.
- `ui.status_show_pane` now controls a safe active-pane identifier/title segment in the status bar.
- Pane-addressed mouse fallback, copy, and scroll requests now include their snapshot window and resolve configured public IDs instead of being redirected by a concurrent active-window change.
- Wheel requests now use the client-local pane hit test and local coordinates, so status-bar/divider scrolling is ignored and differently sized viewers cannot be remapped through a daemon-global geometry lookup.
- Copy mode now retains selections as stable scrollback-history coordinates, supports copying across pages, and sends `g`/`G` to the helper's actual history bounds.
- `ui.show_pane_labels` now visibly controls safe active/inactive pane title labels in the renderer.
- Mouse fallback, focus, resize, and selection failures now remain visible as interactive status messages instead of being discarded or terminating attachment.
- Choose-tree previews of stale or concurrently removed sessions now remain inside the chooser with a cached unavailable placeholder instead of terminating the interactive client.
- Choose-tree rebuilding and “expand all” now fetch a complete daemon-side hierarchy in one request rather than issuing one list request per session, window, and pane.
- Successfully started pane helpers now have their process handles reaped asynchronously after exit, avoiding zombie helper children under a long-lived daemon.
- Persistent helper snapshots no longer infer or serialize foreground command argv; workspace save/restore continues to use declared pane commands plus VT state, avoiding both process-identification and lossy argv-reconstruction risks.
- Daemon integration tests now clean up every isolated live session before terminating their daemon, including assertion-unwind paths, so test runs cannot leave pane helpers behind.
- Pasting text while choose-tree is open now starts or extends its search input instead of being discarded.
- Copy-selection extraction clamps untrusted history positions to actual retained scrollback before walking rows, preventing oversized IPC coordinates from forcing a `u32`-sized helper loop.
- PTY unit tests now wrap spawned pane helpers in a test-only drop guard that sends orderly shutdown, preventing temporary-directory teardown from leaving unreachable helpers running.
- User-facing status and configuration documentation now reflects independent runtime-path overrides, default public numbering of one, and bounded liveness cleanup semantics rather than the older behavior.
- `ui.status_style` now has observable behavior: the default `tmux-plus` multi-zone bar and a compact `minimal` status mode, eliminating a parsed-but-inert configuration field.
- Status-related theme fields now style the actual terminal output for bar fill, sessions, windows, messages, prompt/copy mode, and right-side host/clock rather than being parsed but ignored.
- The fresh complete-suite result recorded below includes every committed remediation batch through configured selection styling. Every batch also has its own rebuilt-binary, focused regression coverage plus `cargo check` and `git diff --check`.

## Scope and process

- Source audit: `ISSUE.md` (imported, uncommitted), containing the prior 151-item source-level audit.
- Work began from the highest-confidence lifecycle, IPC, persistence, and client correctness defects.
- Every completed batch was committed separately. `ISSUE.md`, this file, `.admux/`, `.codex`, and `admux.toml` are intentionally untracked.
- Helper-oriented tests require rebuilding `admux-pane` first; the reliable full validation command is:

  ```sh
  cargo build --bins && cargo test && cargo check
  ```

## Completed commits

### Runtime paths, lifecycle, and session integrity

- `979c727 fix runtime isolation and pane cleanup`
  - Independent config/state/socket overrides; safer XDG/HOME fallback and effective-UID runtime fallback.
  - Test daemons use isolated state/config paths.
  - Dead pane pruning shuts helpers down and retains control state on failure.
  - Bounded helper socket filenames and better helper startup diagnostics.

- `27a0d80 fix session selection and mutation invariants`
  - Reject duplicate sessions; avoid automatic-name collisions.
  - Fix cross-window pane selection, invalid resize/focus handling, inactive-window kill focus changes, and targetless directional focus.
  - Attach updates `last_session`; invalid splits do not mutate focus or leak panes.

- `3deccec keep session state consistent during pane shutdown`
  - Do not remove a pane/window model entry until shutdown succeeds.
  - Window shutdown attempts all panes and retains a consistent partially-cleaned model if a helper fails.

- `27a02ba treat exited pane children as shutdown complete`
  - Correct helper cleanup race for a naturally exited child.

- `7a0598d synchronize pane helper shutdown`
  - A successful shutdown response now waits for the helper socket to disappear before callers remove the only remaining process control handle.
  - Fixes the retry-pruning lifecycle race discovered by the full suite.
  - Targeted validation: rebuilt helper plus both pane-pruning lifecycle tests and `cargo check` (all passed).

- `c7f6968 preserve sessions when shutdown fails`
  - Session shutdown attempts every pane and aggregates failures; destructive session removal and workspace rebuild now surface shutdown errors instead of reporting a false success.
  - Targeted validation: all session tests, last-pane server regression, `cargo check`, and `git diff --check` (all passed).

- `a3af536 propagate scrollback helper failures`
  - Scrollback requests now return helper transport/protocol errors through the session and daemon instead of producing a false `Scrolled` response.
  - Targeted validation: rebuilt helper, PTY transport-failure regression, all session tests, and `cargo check` (all passed).

- `f718808 reject malformed helper snapshots safely`
  - Persistent-snapshot Base64 and UTF-8 decoding is fallible and contextual; malformed helper protocol data cannot panic the daemon.
  - Targeted validation: `cargo test pty::tests::invalid_persistent_snapshot_wire_is_an_error_not_a_panic`, `cargo check`, and `git diff --check` (all passed).

- `b5f918c bound paste buffer storage`
  - All buffers, including explicitly named ones, share the 50-buffer capacity; names and contents are bounded, and `load-buffer` reads at most 1 MiB before rejecting an oversized file.
  - Targeted validation: buffer tests, oversized-file regression, `cargo check`, and `git diff --check` (all passed).

- `fa490de report final pane removal accurately`
  - Final pane removal reports `SessionKilled`; empty non-final window reports `WindowKilled`.

### IPC robustness and persistence

- `298641b harden daemon and pane IPC handling`
  - Daemon connections are isolated workers with bounded payloads/timeouts; malformed peers no longer terminate the daemon.
  - Helper/client requests have bounds and timeouts; helper readiness verifies a socket connection.

- `1dfbdfa revalidate daemon protocol on every request`
  - The client no longer caches protocol validation by socket pathname; every request connection performs a fresh handshake so a replaced daemon cannot evade compatibility checking.
  - Targeted validation: Unix-socket revalidation regression, `cargo check`, and `git diff --check` (all passed).

- `d75d4b0 route wheel events to pane local cells`
  - Mouse-wheel events are hit-tested to a pane, translated from terminal-global to pane-local cells, and rejected outside pane content rather than falling back to the active pane.
  - Targeted validation: wheel-coordinate regression, `cargo check`, and `git diff --check` (all passed).

- `26defac version persisted daemon state`
  - State schema version and unique temporary state filenames.

- `025b0a2 recover from corrupt daemon state files`
  - Corrupt JSON state is quarantined as `*.corrupt.<pid>.<counter>` and startup continues.
  - Valid future schema versions remain a hard compatibility error.

- `f696d16 validate persisted session invariants on recovery`
  - Validate recovered window order, IDs, active pointers, pane/layout membership, duplicate panes, split ratios, and viewport sizes.
  - Normalize legacy `next_pane_id` so pane 0 cannot be overwritten.

- `28bf47b advance recovered window allocator past persisted ids`
  - Prevent global window-ID reuse after stale/edited persisted counters.

- `5f5270e surface daemon state persistence failures`
  - State-write failures are reported to clients rather than silently ignored.

- `6e747ae skip state writes for read-only daemon requests`
  - Removes the full-state rewrite on hello/list/preview/idle attachment requests; still persists meaningful state changes and pruning.

- `bd00b0b harden daemon state persistence`
  - Serializes state loads, recovery, and saves with a private advisory lock; fsyncs state and directory metadata.
  - Retains a private previous-state backup, restores it after corruption or a missing primary, and keeps backup recovery viable across repeated failures.

- `328174c prevent concurrent daemon startup`
  - Holds a non-blocking private advisory lock for the daemon lifetime, so concurrent autostarts cannot replace a live daemon socket.
  - Probes an existing socket before removal and removes it only when the kernel reports a refused connection; inaccessible or live sockets are preserved.
  - Targeted validation: `cargo test server::tests` (21 passed).

### Input, prompt, client, and rendering

- `1ddb4aa preserve default copy mode bindings`
  - Partial `[keys.copy_mode]` now extends defaults.

- `dd6dd43 parse page key aliases correctly`
  - `page-up` and `page-down` aliases parse correctly.

- `f0fb6bd validate behavior configuration sizes`
  - Reject zero resize/page sizes.

- `abb3156 match shifted character bindings correctly`
  - Uppercase bindings become explicit shifted-character bindings, and `Shift-x` matches terminal events consistently.
  - Existing punctuation bindings retain their required Shift tolerance.
  - Targeted validation: `cargo test config::tests`, `cargo test input::tests`, and `cargo check` (all passed).

- `4c05f55 reject unknown configuration fields`
  - Typed configuration sections now use strict deserialization, so misspelled fields fail configuration loading instead of silently doing nothing.
  - Targeted validation: `cargo test config::tests` (17 passed), `cargo check`, and `git diff --check`.

- `8ed7556 forward bracketed paste delimiters`
  - Outer paste preserves bracketed-paste markers for children.

- `3a54f48 forward the leader key on double press`
  - Prefix-prefix forwards the literal leader control key.

- `b6ed07e decode named keys in send-keys`
  - Decodes control/meta, standard navigation, and F1–F12 names while keeping ordinary quoted text literal.

- `a05eae6 forward extended terminal keys`
  - Interactive input now forwards F1–F12, Insert, Page Up/Down, modified cursor/navigation keys, and conventional control-punctuation bytes.
  - Targeted validation: `cargo test input::tests` (17 passed) and `cargo check`.

- `021cd4f edit prompt text at unicode boundaries`
  - Fixes UTF-8 prompt cursor editing panic; cursor drawing no longer uses a byte count.

- `3b78efc accept pasted text in command prompt`
  - Prompt paste inserts safely at the UTF-8 cursor.

- `29a4eb9 saturate copy mode cursor movement`
  - Prevents `u16` overflow/wrap in copy movement.

- `5fd2e3b keep interactive status messages visible`
  - Stops clearing status feedback after one frame.

- `ee08b27 show interactive action failures in the status bar`
  - Leader-key action responses now expose daemon errors rather than discarding them.

- `457686d keep command prompt open on command errors`
  - Prompt parse/command errors remain visible and do not enter history.

- `1621206 preserve session on rejected prompt switch`
  - Prompt-driven session switches only update local state after an `Attached` response.

- `a0f2959 validate chooser session selection`
  - Chooser selection verifies attach and safely handles stale sessions.

- `7c8fd33 hide stale terminal cursors between frames`
  - Renderer hides before drawing and shows only a valid current cursor.

- `2100f9f restore terminal state on interactive errors`
  - Raw mode, alternate screen, cursor, paste, keyboard enhancement, and enabled mouse capture now restore through a drop guard on every return path.
  - Mouse capture is enabled only when `mouse.enabled` is true.
  - Targeted validation: `cargo check`, `cargo test client::tests` (21 passed), and `git diff --check`.

- `ca9fd0b reload the attached client from the prompt`
  - Prompt `reload-config` now refreshes the client’s live keymap/behavior state as well as reloading the daemon, matching the leader-key path.
  - Targeted validation: client reload regression, `cargo check`, and `git diff --check` (all passed).

- `f7e56c1 require terminal stdin for interactive attach`
  - Interactive attach and nested-session switching now require both stdin and stdout to be terminals. A terminal stdout with redirected/closed stdin falls back to the noninteractive response path instead of entering a raw event loop that cannot receive input.
  - Targeted validation: `cargo test client::tests` (26 passed), `cargo check`, and `git diff --check` (all passed).

- `4692c3c cycle command prompt completions correctly`
  - Tab now selects the first completion before advancing and retains the original candidate list for subsequent Tab presses, instead of skipping the first command and immediately collapsing completion cycling.
  - Targeted validation: `cargo test client::tests` (27 passed), `cargo check`, and `git diff --check` (all passed).

- `9ad9b84 surface daemon errors from prompt commands`
  - All prompt-command daemon responses now reject `CommandResponse::Error`, including mutation, list, buffer, and workspace-save commands. The existing prompt error path keeps the prompt open with the server message instead of silently closing as though the command succeeded.
  - Targeted validation: `cargo test client::tests` (28 passed), `cargo check`, and `git diff --check` (all passed).

- `41adc6d keep chooser open on selection errors`
  - Choose-tree window and pane selection now handle daemon errors in place, preserving the chooser and local session state while displaying the failure rather than returning from interactive mode.
  - Targeted validation: `cargo test client::tests` (29 passed), `cargo check`, and `git diff --check` (all passed).

- `d1dae68 implement prompt chooser and detach commands`
  - `choose-tree`, `choose-buffer`, and `detach-client` are now functional prompt commands: they open the corresponding overlay or detach the current client, rather than returning instructions to use a leader shortcut.
  - Targeted validation: `cargo test client::tests` (30 passed), `cargo check`, and `git diff --check` (all passed).

- `a35fd14 refresh focused snapshot before subsequent input`
  - After a successful focus, window, split, or active-pane-kill action, the client now fetches the new snapshot immediately rather than frame-throttling it. A following key therefore cannot be routed via the stale focused helper socket.
  - Targeted validation: `cargo test client::tests` (31 passed), `cargo check`, and `git diff --check` (all passed).

- `5f793b8 honor application cursor key mode`
  - Pane snapshots now carry the VT parser’s application-cursor state. The interactive client sends unmodified arrows as SS3 (`ESC O A/B/C/D`) when the focused application requests DEC application-cursor mode, while modified arrows retain standard xterm CSI sequences.
  - Targeted validation: rebuilt helper, input tests (18 passed), PTY tests (12 passed, including mode propagation), session tests (9 passed), `cargo check`, and `git diff --check` (all passed).

- `16cda88 fall back after direct helper send failure`
  - Direct input now falls back to daemon-routed `SendKeys` when an already-connected helper fails during `send_keys`, not only when connection/liveness setup fails. Daemon `Error` responses are propagated instead of ignored.
  - Targeted validation: direct helper-send fallback regression, `cargo test client::tests` (32 passed), `cargo check`, and `git diff --check` (all passed).

- `ab989a0 preserve unreachable panes in render snapshots`
  - Helper render failures no longer silently omit panes. The snapshot retains the pane’s rect, focus, and helper reference with a neutral unavailable placeholder, preventing an active-pane ID from referring to no rendered pane.
  - Targeted validation: rebuilt helper, session tests (10 passed, including unreachable-helper regression), `cargo check`, and `git diff --check` (all passed).

- `b70334a prefer live sessions for implicit attachment`
  - Targetless attach/save resolution now uses a live remembered session when possible, otherwise any live session, before considering persisted-only stale metadata. An old `last_session` entry can no longer hide an available session.
  - Targeted validation: rebuilt helper, server tests (26 passed, including stale-last-session regression), `cargo check`, and `git diff --check` (all passed).

- `8e90d3a validate window names before mutation`
  - Explicit window names are bounded, nonempty, and control-safe for both `new-window` and `rename-window`. Invalid new-window names are rejected before allocating an ID or launching a helper.
  - Targeted validation: rebuilt helper, server tests (27 passed), `cargo check`, and `git diff --check` (all passed).

- `3152a96 harden renderer index and width bounds`
  - Chooser/help/pane/preview rendering now bounds `usize` indexes before converting them to terminal coordinates, preventing wraparound after 65,535 entries or lines. Preview-title width arithmetic uses checked, saturating conversion.
  - Targeted validation: renderer tests (18 passed), `cargo check`, and `git diff --check` (all passed).

- `8221aa1 enforce private daemon and helper socket paths`
  - Daemon runtime/helper directories are ownership-checked and private; daemon and helper Unix sockets are explicitly restricted to 0600. Helper creation also restricts its explicit directory to 0700, avoiding permissions inherited from a permissive umask.
  - Targeted validation: rebuilt helper, PTY tests (14 passed, including permission regressions), server tests (27 passed), `cargo check`, and `git diff --check` (all passed).

- `c707c0a version pane helper protocol`
  - Pane helper connections now perform a version handshake before liveness or control operations. A daemon reconnecting to a surviving incompatible helper fails with a clear protocol mismatch rather than decoding an arbitrary old wire format.
  - Targeted validation: rebuilt helper, PTY tests (15 passed, including incompatible-helper regression), direct-helper fallback regression, `cargo check`, and `git diff --check` (all passed).

- `85d8f0a scale mouse border resizing to terminal geometry`
  - Mouse drag resizing now converts cell movement into a bounded fraction of the rendered terminal span instead of multiplying it by the keyboard resize step. One-cell drags are proportional across narrow and wide layouts.
  - Targeted validation: client tests (33 passed), `cargo check`, and `git diff --check` (all passed).

- `dffcc6b retain mouse press ownership across panes`
  - Application mouse drags and releases now remain bound to the pane that received the initial left-button press. Coordinates are clamped pane-local when the pointer crosses a divider or exits the pane, so the child always receives a matching mouse-up.
  - Targeted validation: client tests (34 passed, including outside-pane capture regression), `cargo check`, and `git diff --check` (all passed).

- `6498eca escape terminal controls in UI text`
  - Names, prompt/status/chooser text, and buffer UI render control bytes visibly; formatted pane VT output remains intact.

### Workspace and buffers

- `63eb607 allow rebuild without workspace snapshots`
  - `up --rebuild` bypasses corrupt/stale snapshot loading.

- `25eab76 validate workspace cwd paths before launch`
  - Workspace, window, root-pane, and split-pane cwd values are resolved to existing directories during manifest parsing, preventing partial process creation from an invalid cwd.
  - Targeted validation: `cargo test workspace::tests` (10 passed), `cargo check`, and `git diff --check`.

- `776ef3e stage workspace rebuilds transactionally`
  - Workspace construction stays off-map until complete and cleans partial panes on failure; rebuild constructs the replacement first, preserving the existing session if construction or shutdown fails.
  - `--rebuild` consistently bypasses all snapshot seeds, including split panes and active-pane selection.
  - Targeted validation: rebuilt helper, server/workspace suites, failed-rebuild regression, `cargo check`, and `git diff --check` (all passed).

- `741e67f reject unsupported workspace split ratios`
  - Workspace split ratios outside the supported 0.1–0.9 range now fail manifest validation instead of being silently rewritten.
  - Targeted validation: ratio regressions, `cargo check`, and `git diff --check` (all passed).

- `3c5c823 inherit active pane cwd when splitting`
  - Normal splits now inherit the active pane cwd, then its window cwd, then the session cwd, rather than always using the session root.
  - Targeted validation: rebuilt helper, split-CWD regression test, `cargo check`, and `git diff --check` (all passed).

- `8a71dfa protect workspace snapshots with gitignore`
  - An existing `.admux/.gitignore` is now preserved and amended with the required catch-all protection for `snapshot.json`; a hand-maintained ignore file can no longer leave private snapshot contents trackable merely because it already existed.
  - Targeted validation: rebuilt binaries, `cargo test workspace::tests` (12 passed), `cargo check`, and `git diff --check` (all passed).

- `573cc82 use a durable workspace snapshot digest`
  - Snapshot sidecars now identify their manifest digest as SHA-256 and use the defined SHA-256 output rather than Rust's non-format-stable `DefaultHasher`. Legacy or unknown digest algorithms are safely treated as stale snapshots.
  - Targeted validation: rebuilt binaries, `cargo test workspace::tests` (13 passed), `cargo check`, and `git diff --check` (all passed).

- `54f5982 validate workspace snapshots before restore`
  - Matching snapshots are now checked against the manifest before any helper starts: exact unique window/pane coverage, valid active targets, sane dimensions, bounded file/VT/title data, and digest algorithm compatibility. Invalid sidecars fail before workspace construction; `up --rebuild` continues to bypass them.
  - Targeted validation: rebuilt binaries, `cargo test workspace::tests` (13 passed, including corrupted-snapshot regression), `cargo check`, and `git diff --check` (all passed).

- `5647731 keep runtime commands out of workspace files`
  - `admux save` now retains each pane's declared command rather than helper-reported foreground argv, and snapshots no longer serialize command arguments at all. This prevents transient credentials or one-off invocation arguments from being written to shareable manifests or snapshot sidecars.
  - Targeted validation: rebuilt binaries, `cargo test workspace::tests` (13 passed, including declared-command/no-snapshot-command regression), `cargo check`, and `git diff --check` (all passed).

- `b87224a stage workspace manifest and snapshot saves`
  - Workspace manifests and snapshots are now written, fsynced, and renamed from unique temporary files. The snapshot is committed first, so it is either paired with the new manifest or digest-mismatched and ignored; failures before snapshot staging preserve the existing manifest.
  - Targeted validation: rebuilt binaries, `cargo test workspace::tests` (14 passed, including failed-save manifest-preservation regression), `cargo check`, and `git diff --check` (all passed).

- `dcdcddd bind helper sockets before starting pane commands`
  - The pane helper now binds and permissions its control socket before creating the PTY child command. If child startup fails, the prebound socket is removed, preventing a started pane from outliving an unsuccessful helper bind.
  - Targeted validation: rebuilt binaries, `cargo test pty::tests` (16 passed, including failed-startup socket-cleanup regression), `cargo check`, and `git diff --check` (all passed).

- `2ce6bff replay terminal history for mixed-axis expansion`
  - A PTY resize now reconstructs its parser from history whenever either dimension expands. Mixed changes such as 20x10 to 10x80 no longer retain narrow-width wrapping merely because the row count decreased.
  - Targeted validation: rebuilt binaries, `cargo test pty::tests` (17 passed, including mixed-axis expansion regression), `cargo check`, and `git diff --check` (all passed).

- `bc33e95 trim PTY history at replay-safe boundaries`
  - The fixed-size raw-history buffer now trims after complete UTF-8 and VT sequence boundaries rather than at an arbitrary byte. Unterminated control strings are discarded instead of being replayed into a fresh parser as malformed input.
  - Targeted validation: rebuilt binaries, `cargo test pty::tests` (19 passed, including UTF-8/CSI and unterminated-string regressions), `cargo check`, and `git diff --check` (all passed).

- `10ef90c move copy line end to visible content`
  - Copy-mode `$` now targets the last non-whitespace character of the selected rendered row rather than the pane's rightmost column, avoiding trailing-padding yanks.
  - Targeted validation: `cargo test copy_mode::tests` (6 passed), `cargo test client::tests` (34 passed), `cargo check`, and `git diff --check` (all passed).

- `7041541 replace machine-local README links`
  - Repository documentation links now use portable relative paths, so the staged wiki source and engineering records work from GitHub and every checkout.
  - Targeted validation: searched README/docs for the former machine-local repository path; `git diff --check` passed.

- `507091a propagate pane selection-copy failures`
  - Selection extraction now reports helper transport/protocol failures through session and daemon responses rather than silently returning an empty `SelectionCopied` payload.
  - Targeted validation: rebuilt binaries, `cargo test pty::tests` (20 passed, including selection transport failure), `cargo test session::tests` (10 passed), `cargo test server::tests` (27 passed), `cargo check`, and `git diff --check` (all passed).

- `8071294 forward middle and right application mouse buttons`
  - Middle/right press, drag, and release events are now captured to the original application pane and forwarded through direct-helper and daemon paths. Left remains reserved for existing selection and border-resize behavior when the pane is not mouse-reporting.
  - Targeted validation: rebuilt binaries, `cargo test client::tests` (35 passed, including button mapping regression), `cargo test session::tests` (10 passed), `cargo test pty::tests` (20 passed), `cargo check`, and `git diff --check` (all passed).

- `9b35cae preview the chooser's selected window or pane`
  - Choose-tree previews now carry the selected target to a read-only daemon preview route. Window/pane selections render their own window and focus the selected pane without mutating the live attachment; session selections retain the all-window overview.
  - Targeted validation: rebuilt binaries, `cargo test session::tests` (11 passed, selected-preview regression), `cargo test server::tests` (27 passed after isolated daemon-lock rerun), `cargo test client::tests` (35 passed), `cargo check`, and `git diff --check` (all passed).

- `4673afb cache idle chooser previews`
  - Choose-tree and buffer chooser previews are cached for the currently selected item, preventing full preview IPC and buffer cloning on every idle render frame. Tree rebuilds invalidate their cache.
  - Targeted validation: `cargo test client::tests` (35 passed), `cargo check`, and `git diff --check` (all passed).

- `76ee241 keep selected chooser items in a scrolling viewport`
  - Tree and buffer choosers now render a viewport centered around the selected item instead of hard-clipping to their first eight entries. Keyboard navigation beyond eight entries remains visible.
  - Targeted validation: `cargo test render::tests` (19 passed, viewport regression), `cargo test client::tests` (35 passed), `cargo check`, and `git diff --check` (all passed).

- `1561830 reap pane children before successful helper shutdown`
  - Helper shutdown now waits for a successfully killed child to exit before returning `Ok`, so daemon model cleanup cannot discard the final control path while the child remains unreaped.
  - Targeted validation: rebuilt binaries, `cargo test pty::tests` (20 passed), `cargo check`, and `git diff --check` (all passed).

- `2498cc7 only autostart a genuinely absent daemon`
  - Client autostart now occurs only for a missing socket or connection refusal. Permission-denied, invalid-path, and other transport failures return their original error without spawning an additional daemon.
  - Targeted validation: `cargo test client::tests` (36 passed, autostart classification regression), `cargo check`, and `git diff --check` (all passed).

- `179e289 retain daemon autostart diagnostics in a private log`
  - Autostart now sends daemon stdout/stderr to a 0600 `admuxd-startup.log` beside state files, and post-autostart connection errors name that log. The log intentionally avoids creating the private socket directory before the daemon validates it.
  - Targeted validation: `cargo test client::tests` (37 passed, log-path regression), `cargo check`, and `git diff --check` (all passed).

- `caa97e1 honor negotiated mouse protocols`
  - Helper mouse forwarding now honors the child application's requested xterm encoding (default, UTF-8, or SGR) and reporting mode (X10 press-only, VT200 press/release, or motion tracking). It emits legacy release reports correctly, avoids unsupported drag reports, and rejects coordinates the default one-byte format cannot represent instead of wrapping them.
  - Targeted validation: rebuilt binaries, `cargo test pty::tests` (23 passed, including encoding/motion/bounds regressions), `cargo check`, and `git diff --check` (all passed).

- `b953599 resize sessions after manual client switches`
  - Prompt and choose-tree session switches now use the same state transition as automatic redirects, clearing the cached viewport whenever the attached session changes. The next interactive loop sends the current terminal dimensions to the new session rather than retaining its former/default PTY size.
  - Targeted validation: `cargo test client::tests` (38 passed, including prompt-switch viewport regression), `cargo check`, and `git diff --check` (all passed).

- `1efc958 avoid idle terminal redraws`
  - The interactive client retains 16 ms input polling but samples panes at 10 Hz and redraws only after snapshot or UI changes. An unchanged idle session no longer emits a full clear/redraw each poll; choose-tree previews still refresh at the reduced sampling interval.
  - Targeted validation: `cargo test client::tests` (39 passed, including refresh-interval regression), `cargo check`, and `git diff --check` (all passed).

- `b5d3a33 salvage reachable panes during recovery`
  - Startup now reconnects each persisted pane independently. Missing helper sockets collapse only their affected layout branches; windows with no reachable panes are removed, while surviving windows and active-window state are normalized and immediately persisted without stale pane metadata.
  - Targeted validation: rebuilt binaries, `cargo test session::tests` (12 passed, sibling-pane recovery regression), `cargo test server::tests` (27 passed), `cargo check`, and `git diff --check` (all passed).

- `10afcca propagate nested command errors`
  - Nested `new` and `up` still suppress ordinary response printing for the parent-client redirect, but now convert daemon `Error` responses into client failures instead of exiting successfully after a rejected operation.
  - Targeted validation: `cargo test client::tests` (39 passed, including command-response error handling), `cargo check`, and `git diff --check` (all passed).

- `e4a085b avoid redundant helper liveness probes`
  - Direct key/mouse forwarding no longer sends an `IsAlive` request after its version handshake and before the actual control request. Send failures still fall back through the daemon, while daemon recovery retains explicit live-child verification.
  - Targeted validation: rebuilt binaries, `cargo test pty::tests` (23 passed), `cargo test client::tests` (39 passed, direct-send fallback now verifies no liveness probe), `cargo test session::tests` (12 passed), `cargo check`, and `git diff --check` (all passed).

- `224b818 resize only visible session windows`
  - Viewport and layout synchronization now resizes only the active window, avoiding IPC and partial-failure exposure for every hidden PTY. Selecting a window or pane in another window synchronizes the newly visible panes and restores the prior selection if that resize fails; viewport dimensions commit only after successful active-window sizing with rollback on failure.
  - Targeted validation: rebuilt binaries, `cargo test session::tests` (13 passed, hidden-window socket regression), `cargo test server::tests` (27 passed), `cargo check`, and `git diff --check` (all passed).

- `cb43c2f preserve raw terminal input bytes`
  - Interactive input now has a byte-oriented helper/daemon protocol path, so non-UTF-8 terminal bytes no longer pass through lossy string conversion on direct forwarding or daemon fallback. Textual `send-keys` remains token-aware and is deliberately separate.
  - Targeted validation: rebuilt binaries, `cargo test ipc::tests` (4 passed, arbitrary-byte round trip), `cargo test pty::tests` (23 passed), `cargo test client::tests` (39 passed), `cargo test server::tests` (27 passed), `cargo check`, and `git diff --check` (all passed).

- `1eaa247 render unicode text by terminal cells`
  - UI fitting, truncation, status accounting, prompt cursor placement, ANSI clipping, copy line-end movement, and selection overlays now use Unicode terminal-cell widths and preserve grapheme clusters. Wide CJK/emoji cells no longer misalign later content or map a continuation cell to another character.
  - Targeted validation: `cargo test render::tests` (21 passed, wide/combining/grapheme regressions), `cargo test copy_mode::tests` (7 passed), `cargo check`, and `git diff --check` (all passed).

- `970adcc keep paste buffers out of persistent state`
  - Paste buffers remain usable for the daemon lifetime but are no longer serialized or restored. Startup redacts legacy buffer contents immediately, and state backups are generated from a sanitized JSON representation so copied secrets do not survive in either primary or backup metadata.
  - Targeted validation: rebuilt binaries, `cargo test persistence::tests` (7 passed, legacy primary/backup redaction regression), `cargo test server::tests` (27 passed, restart intentionally clears buffers), `cargo check`, and `git diff --check` (all passed).

- `b91b5ca rate limit global pane liveness pruning`
  - The daemon no longer walks every session/window/pane on every request. Global helper pruning runs at most once per second; direct operations still surface their own helper transport errors immediately.
  - Targeted validation: `cargo test server::tests` (28 passed, pruning cadence regression), `cargo check`, and `git diff --check` (all passed).

- ba76dd1 reuse pane snapshots in attach responses
  - An attach now renders each pane once and derives the legacy active-pane preview, formatted preview, and cursor fields from that focused snapshot pane. This removes the three redundant active-helper snapshot round trips per attach while keeping old response consumers compatible.
  - Targeted validation: rebuilt binaries; server tests (28 passed, preview-field equivalence regression); renderer tests (21 passed); client tests (39 passed); cargo check and git diff --check passed.

- `caa774b propagate pane screen size failures`
  - Removed unused preview/cursor/row convenience APIs that converted helper failures into empty strings, empty rows, or a fake `24x80` screen. The remaining screen-size query is now fallible and preserves helper protocol/transport errors for callers.
  - Targeted validation: rebuilt binaries; PTY tests (24 passed, including screen-size error regression); session tests (13 passed); workspace tests (14 passed); `cargo check` and `git diff --check` passed.

- `fdcb4ed move pane restore data out of argv`
  - Pane startup now serializes helper arguments, including restored VT state, to a unique private 0600 file in the helper directory and passes only its path to `admux-pane`. The helper consumes and removes that file before starting, eliminating OS single-argument limits for large restore snapshots.
  - Targeted validation: rebuilt binaries; PTY tests (25 passed, including a 256 KiB restore-seed regression); session tests (13 passed); workspace tests (14 passed); `cargo check` and `git diff --check` passed.

- `a7e4e6a retain scrollback across large pane resizes`
  - Raw replay history now scales with configured scrollback and terminal width instead of a fixed 2 MiB cap, with a 64 MiB hard ceiling. Behavior configuration now rejects zero or unsafe upper bounds for scrollback, workspace snapshot lines, resize step, and copy page size.
  - Targeted validation: rebuilt binaries; config tests (17 passed); PTY tests (27 passed, including a 30,000-line, over-2-MiB resize/replay regression); session tests (13 passed); workspace tests (14 passed); `cargo check` and `git diff --check` passed.

- `a9c0dde implement external clipboard commands`
  - Added typed `[clipboard]` configuration. OSC52 remains the default; users can now select `external-command` with an explicit argv list, which receives copied text on stdin and reports execution failures. Empty external commands are rejected during config resolution.
  - Targeted validation: config tests (18 passed); client tests (40 passed, including external-command stdin regression); `cargo check` and `git diff --check` passed.

- `3cf28c7 roll back failed pane and window creation`
  - Session construction now shuts down its root helper if initial sizing fails. Split and new-window creation now roll back their model entries, layout, and helper after a resize failure; if helper shutdown itself fails, the new object remains discoverable and the error explicitly reports the failed rollback instead of orphaning it.
  - Targeted validation: rebuilt binaries; session tests (14 passed, including hidden-helper split rollback/socket-cleanup regression); server tests (28 passed); workspace tests (14 passed); `cargo check` and `git diff --check` passed.

- `0a90ead preserve workspace manifest annotations on save`
  - `admux save` now edits an existing manifest in place rather than reconstructing it wholesale. Known workspace fields are refreshed while existing comments, value decoration/formatting, and unknown root/workspace/window/pane fields are retained; inline and table pane representations are both handled.
  - Targeted validation: rebuilt binaries; workspace tests (15 passed, including comment/unknown-field preservation regression); server tests (28 passed); `cargo check` and `git diff --check` passed.

- `7ddbded honor copy mode hint configuration`
  - `ui.copy_mode.show_hints` now controls whether the copy-mode status bar prints its full key-reference text or only the compact mode indicator.
  - Targeted validation: renderer tests (22 passed, including disabled-hints regression); `cargo check` and `git diff --check` passed.

- `cd31272 render sessions per attached client viewport`
  - Interactive attaches now carry a client viewport lease. Each client receives a render snapshot laid out for its own dimensions, while the active pane PTYs use the maximum rows and columns of all live five-second leases. The protocol version was bumped to reject incompatible clients cleanly.
  - Targeted validation: rebuilt binaries; IPC tests (4 passed); client tests (40 passed); server tests (29 passed, including simultaneous 24x80 and 50x160 client regression); session tests (14 passed); `cargo check` and `git diff --check` passed.

- `0391a1b isolate pane helper connections`
  - The helper listener now accepts clients independently, bounds concurrent request workers at 64, and uses a shutdown signal to retire the listener after a successful shutdown response. A stalled or malformed peer therefore consumes only its own timed-out worker instead of blocking every helper operation.
  - Targeted validation: rebuilt binaries; PTY tests (28 passed, including incomplete-client concurrency regression); session tests (14 passed); `cargo check` and `git diff --check` passed. Repository-wide `cargo fmt --check` remains pre-existingly noisy outside this batch.

- `c045a90 make pane resize synchronization transactional`
  - Pane-size synchronization now probes every active-window helper before changing any PTY, tracks each successful resize, and restores prior actual sizes if a subsequent resize fails. Failed active-pane layout resizing restores its original layout rather than leaving changed split ratios behind.
  - Explicit resize operations on inactive windows now update only their persisted layout; their helpers are not contacted until that window becomes visible.
  - Targeted validation: rebuilt binaries; session tests (16 passed, including failure/layout rollback and inactive-window helper-isolation regressions); server tests (29 passed); workspace tests (15 passed); `cargo check` and `git diff --check` passed.

- `c38f096 scope nested session redirects to clients`
  - Interactive Enter/newline submission now registers the source session/window/pane and client ID before forwarding input. Nested `new` and `up` look up that short-lived source ownership and queue redirects by `(source session, client ID)`; only the originating interactive attach can consume it. Protocol version 7 rejects older clients cleanly.
  - Targeted validation: rebuilt binaries; IPC tests (4 passed); client tests (41 passed, including submission-registration ordering); server tests (29 passed, including distinct author/viewer redirect regression); `cargo check` and `git diff --check` passed. A daemon-lock test flaked once during the first parallel server-module run, then passed in isolation and the subsequent complete server run.

- `9b91ea9 avoid duplicate helper probes for input`
  - The direct interactive input hot path now sends bytes through a single helper request instead of performing a separate helper protocol probe followed by the send. Failed direct sends still fall back to daemon-mediated input.
  - Targeted validation: rebuilt binaries; PTY tests (28 passed); client tests (41 passed, with first helper request asserted to be `SendBytes`); server tests (29 passed); `cargo check` and `git diff --check` passed.

- `f2d7614 honor overlay border and title settings`
  - Tree chooser, buffer chooser, and help overlays now apply their configured `border` and `title` settings. Disabling both removes the separator row; enabling either renders only the requested element, with safe terminal text and correct preview/body geometry.
  - Targeted validation: rebuilt binaries; renderer tests (23 passed, including all border/title combinations); config tests (18 passed); client tests (41 passed); `cargo check` and `git diff --check` passed.

- `bb90f15 render configured active pane status`
  - `ui.status_show_pane` now survives config resolution and adds or omits a terminal-safe `pane:<id>:<title>` status segment as configured.
  - Targeted validation: rebuilt binaries; config tests (19 passed); renderer tests (24 passed, including on/off status-pane rendering); client tests (41 passed); `cargo check` and `git diff --check` passed.

- `9969c77 target pane requests to explicit windows`
  - `MousePane`, `CopySelection`, and `ScrollPane` now carry an explicit public window identifier. The session resolves that window and public pane number through configured numbering before reaching the helper, rather than implicitly using the active window.
  - Targeted validation: rebuilt binaries; IPC tests (4 passed); session tests (17 passed, including inactive target plus nonzero window/pane bases); client tests (41 passed); server tests (29 passed); `cargo check` and `git diff --check` passed.

- `a7468a6 route wheel events to explicit panes`
  - `MouseScroll` now carries the snapshot's public window and pane IDs plus coordinates already translated to that pane's local cells. The client declines wheel events outside pane content, so status bars and dividers cannot fall through to the active pane; the daemon no longer rebuilds a global snapshot for wheel hit testing.
  - Targeted validation: rebuilt binaries; nonzero-public-numbering session regression; IPC tests (4 passed); client tests (41 passed); server tests (28 passed); `cargo check` and `git diff --check` passed.

- `5a1ec3f preserve copy selections across scrollback`
  - Copy mode records each endpoint as a distance from the live bottom of the pane rather than a row within one rendered page. Page movement therefore preserves selection identity, renderer highlights the visible slice, and helper extraction walks historical rows without leaving the pane at a different scrollback offset.
  - `g`/`G` now dispatch explicit top/bottom scrollback commands instead of only moving the cursor in the current page. Mouse selection uses the same history coordinates. Daemon and helper protocol versions were bumped for the new messages.
  - Targeted validation: rebuilt binaries; `cargo test copy_mode::tests` (8 passed); `cargo test pty::tests` (30 passed, history and live-PTY boundary regressions); client tests (41 passed); IPC tests (4 passed); session tests (18 passed); server tests (28 passed); `cargo check` and `git diff --check` passed.

- `20ef671 render configured pane labels`
  - `ui.show_pane_labels` now renders terminal-safe `<pane-id>:<title>` labels over pane origins using active/inactive UI styles. Disabling the setting omits the labels entirely.
  - Targeted validation: renderer tests (25 passed, including on/off label regression), `cargo check`, and `git diff --check` passed.

- `922104f surface interactive mouse command failures`
  - Direct pane mouse sends now use a shared checked daemon fallback. Rejected daemon responses for mouse presses/drags/releases, click focus, border resize, and selection copy become a status message and retain the interactive client rather than being ignored or propagated out of the event loop.
  - Targeted validation: client tests (41 passed), `cargo check`, and `git diff --check` passed.

- `019ab2a keep stale chooser previews interactive`
  - A `PreviewSession` daemon error for a stale or concurrently removed choose-tree item now becomes a cached unavailable placeholder rather than escaping the render loop and detaching the interactive client.
  - Targeted validation: client tests (42 passed, including stale-preview socket regression), `cargo check`, and `git diff --check` passed.

- `ea92521 collapse chooser tree IPC fan-out`
  - Added a versioned `ListChooseTree` response containing live or persisted session/window/pane summaries. Choose-tree rebuild and “expand all” now use one request each, avoiding N+1 daemon IPC and stale-session subrequest failures.
  - Targeted validation: rebuilt binaries; IPC tests (4 passed); server tests (29 passed, hierarchy regression); client tests (42 passed); `cargo check`, and `git diff --check` passed.

- `6b3b119 reap exited pane helpers`
  - After startup readiness succeeds, `PaneProcess` transfers its helper `Child` handle to a background waiter. Startup failure keeps its existing synchronous kill/reap path; ordinary helper exits can no longer accumulate zombies under `admuxd`.
  - Targeted validation: rebuilt binaries; PTY tests (30 passed); session tests (18 passed); `cargo check`, and `git diff --check` passed.

- `594c520 remove runtime command snapshot inference`
  - Removed unused helper foreground-command capture from the persistent snapshot wire protocol and bumped its version. Restore retains only VT state and the declarative pane command, eliminating incorrect process-group inference and lossy `ps` argv reconstruction.
  - Targeted validation: rebuilt binaries; PTY tests (30 passed); workspace tests (15 passed); persistence tests (7 passed); `cargo check`, and `git diff --check` passed.

- `b5275ed clean up daemon integration sessions`
  - The daemon CLI integration harness now owns a drop guard that lists and kills sessions through its isolated daemon before killing the daemon process. This covers normal exits and assertion unwinding, avoiding leaked helper/child processes from active workspace tests.
  - Targeted validation: rebuilt binaries; daemon integration tests (5 passed); `cargo check`, and `git diff --check` passed.

- `5d56d5a accept pasted choose-tree searches`
  - `Event::Paste` now starts or extends choose-tree search input, matching the existing prompt paste behavior instead of dropping text while the interactive chooser is open.
  - Targeted validation: client tests (43 passed, including chooser-paste regression); `cargo check`, and `git diff --check` passed.

- `a833218 bound historical copy selection coordinates`
  - Helper selection extraction now caps requested row distances at the terminal's actual retained history before iterating. Malicious or stale `u32` endpoints cannot trigger an enormous loop, and the existing viewport is restored afterward.
  - Targeted validation: rebuilt binaries; PTY tests (31 passed, including extreme-coordinate regression); copy-mode tests (8 passed); session tests (18 passed); `cargo check`, and `git diff --check` passed.

- `488ad7e clean up spawned PTY test helpers`
  - PTY tests now use a test-only `PaneProcess` wrapper that delegates normal APIs and calls helper shutdown on drop. This preserves production helper persistence while ensuring test-created helpers cannot outlive their temporary runtime directory.
  - Targeted validation: rebuilt binaries; PTY tests (32 passed, including drop-cleanup regression); `cargo check`, and `git diff --check` passed.

- `0d70696 correct runtime behavior documentation`
  - Updated public status/wiki docs for independent socket/config/state overrides, default `1`-based configurable public window/pane numbering, and the actual bounded-pruning helper cleanup behavior. The keybinding reference no longer promises an unavailable default `Ctrl-b 0` window selection.
  - Targeted validation: searched user-facing docs for stale numbering claims; `git diff --check` passed. The complete Rust validation before this documentation-only batch passed 270 unit tests, 2 CLI smoke tests, 5 daemon integration tests, doc tests, and `cargo check`.

- `6c218a6 implement minimal status style`
  - Added `ui.status_style = "minimal"` alongside the default `tmux-plus`. Minimal mode renders only current-session context and optional active-pane metadata, while tmux-plus preserves the multi-zone session/window/host/clock bar. The resolved configuration now carries the field to the renderer.
  - Targeted validation: config tests (20 passed); renderer tests (26 passed); `cargo check`, and `git diff --check` passed.

- `a3915d6 apply configured status themes`
  - Status segments now carry and apply configured styles. `theme.status` fills the bar; current/other session, active/inactive/last window, right status, message, prompt, and copy-mode styles each affect their documented output instead of relying solely on hard-coded reverse/bold attributes.
  - Targeted validation: renderer tests (27 passed, theme propagation regression); config tests (20 passed); `cargo check`, and `git diff --check` passed.

- `a805f04 apply configured selection theme`
  - Selection overlays now apply `ui.theme.selection` before their reverse-video attribute, so the configured selection foreground/background is visible rather than parsed but inert.
  - Targeted validation: renderer tests (27 passed, selection-theme regression); `cargo check`, and `git diff --check` passed.

- `c394beb harden persistence review paths`
  - State-file advisory locking retries interrupted `flock` calls instead of exposing transient signal delivery as a persistence failure.
  - Alias mutations now hold a private advisory lock across load/mutate/save, use unique 0600 temporary files, and preserve concurrent CLI updates rather than only avoiding a shared temporary-path collision.
  - Workspace save commits the manifest before its snapshot sidecar. A manifest-commit failure therefore retains the prior usable snapshot; a later snapshot failure leaves a clear error while the prior sidecar remains an intentional digest mismatch rather than being overwritten first.
  - Targeted validation: persistence tests (8 passed), alias tests (7 passed), workspace tests (16 passed), client tests (43 passed), plus a full rebuilt suite (277 unit tests, 2 CLI smoke tests, 5 daemon integration tests, doc tests, `cargo check`, and `git diff --check` all passed).

- `2b89104 clean workspace mappings with sessions`
  - Manifest-to-session mappings are removed whenever a session is unrecoverable, killed, naturally pruned, emptied, or replaced by rebuild.
  - Targeted validation: `cargo test server::tests` (22 passed), `cargo check`, and `git diff --check`.

- `8424cbd resolve new session cwd at the client`
  - Explicit relative `new --cwd` values are converted to client-relative absolute paths before IPC, so they no longer depend on the daemon startup directory.
  - Targeted validation: `cargo test client::tests::normalize_new_args` (3 passed), `cargo check`, and `git diff --check`.

- `f5f6f0c resolve buffer paths at the client`
  - CLI and prompt `save-buffer`/`load-buffer` paths are converted to client-relative absolute paths before daemon IPC.
  - Targeted validation: `cargo test client::tests::buffer_file_paths_are_resolved_at_the_client`, `cargo check`, and `git diff --check` (all passed).

- `b119746 avoid automatic paste buffer name collisions`
  - Automatic names skip explicit `bufferNNNN` names.

- `61a8b6d wrap paste buffer sequence allocation safely`
  - Paste-buffer allocation wraps safely rather than hanging at `u64::MAX`.

## Latest full validation

Latest full run:

```sh
cargo build --bins && cargo test && cargo check
```

Result: **277 unit tests**, 2 CLI smoke tests, 5 daemon integration tests, doc tests, compilation, and `git diff --check` all passed. The first full attempt in this effort exposed a pane-helper shutdown race; `7a0598d` fixed it and subsequent full runs pass.

## Current unresolved high-impact areas

The imported 151-item inventory has now been reconciled against this checkout.
Its source-level claims are either covered by the committed remediation batches
above or are no longer applicable because the risky behavior was removed (for
example, runtime foreground-command reconstruction). The fresh full suite
exercises the critical adversarial categories called out by the original audit:
malformed/stalled IPC, duplicate sessions, concurrent startup, mixed client
viewports, Unicode rendering/editing, persistence corruption, socket
permissions, lifecycle cleanup, and rollback paths.

The only remaining release activity is external review of the cumulative draft
PR and any newly reported regressions; no imported ISSUE.md item remains known
open in this worktree.

## Current worktree

Draft PR: https://github.com/abhiyandhakal/admux/pull/1 (`agent/audit-reliability-foundation` → `master`). It contains the committed remediation work; the local audit/progress artifacts below are intentionally excluded.

Expected untracked coordination/local files:

- `.admux/`
- `.codex`
- `ISSUE.md`
- `PROGRESS.md`
- `admux.toml`

Do not stage or commit those files.
