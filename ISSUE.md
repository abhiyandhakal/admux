# admux issue inventory

> **Status:** imported audit notes; not independently re-verified in this checkout.
>
> **Source:** referenced ChatGPT conversation “Deep analysis of admux”, which
> reported an audit of revision `ae293cf1a4ad6db95e835f8a586275cc8cbbc6c6`.
> This checkout is currently at `3e94f74`; every item needs confirmation
> against current code and a reproducer before it is treated as an open bug.
>
> The source audit could not run builds, tests, sanitizers, or interactive PTY
> tests. Its findings are source-level claims, with several explicitly marked
> as risks. This document is deliberately an issue inventory, not a release
> assessment or a verified security advisory.

## Release blockers claimed by the imported audit

1. Attached clients share one global PTY viewport, so differently sized clients cannot coexist correctly.
2. Naturally exited panes leak their `admux-pane` helper and socket.
3. Malformed, incomplete, or failed IPC requests can terminate the daemon/helper listener.
4. A client that never closes its request can block the serial daemon; reads are unlimited and have no deadline.
5. Duplicate session creation overwrites a live session and can orphan its panes; automatic names can collide.
6. Concurrent autostart/manual starts can unlink each other's live socket and create competing daemons.
7. Runtime socket fallback can collapse across users and does not enforce ownership or private permissions.
8. Pane helper sockets expose unauthenticated control/screen access and are leaked in render snapshots.
9. Idle attachment sends roughly 62 requests/second and rewrites full state after each read-only request.
10. Each request probes every pane in every session, with no timeout.
11. `Attach` snapshots the active pane repeatedly (claimed four times per attachment).
12. Mutations are non-transactional and can leave partial model/process changes.
13. A failed split can leave an invisible live pane.
14. Failed pane/window/session cleanup can corrupt the model and orphan helpers.
15. Nested-session redirects are global to a session rather than client-specific.

## Full imported inventory

### State, recovery, and mutation

16. Recovery is all-or-nothing per session; one dead helper discards other recoverable panes.
17. Metadata persistence errors are ignored.
18. Corrupt state prevents daemon startup rather than being quarantined/recovered.
19. Persisted state has no schema version or migrations.
20. Missing `next_pane_id` can default to zero and overwrite pane zero during recovery.
21. Persisted IDs, ordering, layout, active targets, ratios, and counters are insufficiently validated.
22. State writes use one fixed temporary filename and race.
23. State writes lack fsync, backup, permission enforcement, and writer locking.
24. Paste buffers (potentially secrets) are persisted in plaintext.
25. Invalid explicit resize can set `layout.active` to a nonexistent pane.
26. Focus/resize report success when no action was applicable.
27. Selecting a pane in another window does not make that window active.
28. A failed split can still change active focus.
29. Killing an inactive window can change active-window selection.
30. PTY resize synchronization can partially apply.
31. Rendering silently omits panes when helper communication fails.
32. Default attachment can choose stale metadata before a live session.
33. Workspace rebuild ignores old-session cleanup failures.
34. Workspace construction has no rollback.
35. Session construction can leak the root helper on initial resize failure.

### IPC and daemon robustness

36. Client and helper socket reads/writes have no timeouts.
37. Every client connection failure triggers daemon autostart, including permission and path errors.
38. Autostart discards daemon diagnostics.
39. Client protocol validation is cached by socket path and can become stale after replacement.
40. Helper protocol has no version negotiation.
41. Helper readiness checks only path existence, not a live compatible socket.
42. Spawned helper `Child` handles are discarded and cannot reliably be reaped/terminated.

### Terminal input

43. `send-keys` writes key names literally rather than translating `C-l`, `Enter`, etc.
44. Function keys, paging, insert, and many modified keys cannot be forwarded.
45. Application cursor-key mode is ignored.
46. Pressing the leader twice cannot pass a literal leader key through.
47. Bracketed paste markers are stripped before input reaches the child.
48. Event-to-UTF-8 input is not byte-transparent.
49. Interactive code discards server-side `CommandResponse::Error` values.
50. Prompt switching updates local session state even after a rejected server response.
51. Nested `new`/`up` can suppress errors and exit successfully.

### Unicode and UI safety

52. Prompt cursor mixes byte and character offsets, causing non-ASCII editing panics.
53. Layout uses Unicode scalar counts instead of terminal display-cell widths.
54. Selection overlays use character indices rather than terminal cell columns.
55. Names/labels can inject terminal control sequences.
56. Empty, control-character, pathological, and unbounded session names are accepted.

### Mouse and direct-helper handling

57. Wheel coordinates are wrong in split panes.
58. Mouse encoding ignores the negotiated application protocol.
59. Chooser previews ignore selected window/pane targets.
60. Choosers lack scrolling after eight items.
61. Previewing a stale session can terminate the chooser/client.
62. Choosing a session does not verify attachment before changing local state.
63. Prompt/chooser session switches do not reset viewport state.
64. Building the tree chooser causes an N+1 request storm.
65. Chooser previews refetch while idle.
66. Status messages clear after one render frame.
67. Completion skips its first candidate and then cannot cycle reliably.
68. Pasting into prompt/chooser/help overlays does nothing.
69. Prompt syntax/request errors exit interactive mode rather than displaying feedback.
70. `choose-buffer`, `choose-tree`, and `detach-client` prompt commands are stubs.
71. Prompt `reload-config` does not reload current client configuration.
72. Rapid input after focus/window changes can reach the old pane.
73. Ordinary direct input makes an avoidable liveness connection plus send connection.
74. A failed direct send does not fall back through the daemon.
75. Terminal raw/alternate-screen restoration is not RAII-protected.
76. Interactive mode tests stdout but not stdin terminal capability.
77. Mouse capture is enabled even when mouse support is disabled.
78. Mouse press is not bound to a pane for drag/release routing.
79. Mouse-up outside a pane is dropped.
80. Only left mouse button events are supported.
81. Wheel over status/dividers can scroll the active pane.
82. Wheel coordinate forwarding remains terminal-global, not pane-local.
83. Mouse encoding does not honor protocol/motion mode negotiated by the child.
84. Border resize step is coarse/nonlinear and disconnected from pane geometry.

### Cross-session dispatch and paths

85. Directional focus resolves through global `last_session`, not requesting client's session.
86. Explicit attach does not update `last_session`.
87. Window-local pane IDs are used in requests without a window identifier.
88. Splits inherit session cwd instead of the target pane/window cwd.
89. `kill-session` reports success despite failed helper cleanup.
90. `up --rebuild` destroys the old workspace before the replacement succeeds.
91. `--rebuild` still restores split-pane snapshot state.
92. Window IDs are consumed before validation/construction succeeds.
93. Killing the last pane returns an unrelated/quiet response type.
94. Relative `--cwd` depends on daemon startup directory.
95. Relative buffer paths depend on daemon startup directory.
96. Buffers/filesystem requests have no meaningful size limits.
97. Viewport dimensions commit before resize succeeds.
98. Geometry changes resize panes in inactive windows too.
99. `Hello` and list requests trigger global liveness/persistence work.
100. Workspace mappings are not consistently removed with sessions.
101. `ADMUX_CONFIG` and `ADMUX_STATE` are ignored unless `ADMUX_SOCKET` is set.
102. `ADMUX_SOCKET` alone changes config/state defaults to cwd-relative paths.
103. Missing HOME/XDG writes state into the current directory.

### Configuration and rendering

104. A `[keys]` table can erase all default copy-mode bindings.
105. Partial copy-mode configuration replaces rather than extends defaults.
106. Advertised `page-up`/`page-down` aliases cannot parse.
107. Character modifier matching treats Shift inconsistently.
108. Unknown config keys are silently ignored.
109. Numeric config values are unbounded/unvalidated.
110. Several exposed UI/theme/config options are not applied at runtime.
111. Idle renderer clears/redraws the entire terminal at frame rate.
112. Cursor can remain shown at stale location after focused pane loses its cursor.
113. Buffer overlays render raw terminal control sequences.
114. Unbounded user lengths are narrowed to `u16` in renderer calculations.

### PTY helper and workspace persistence

115. Helper failures are converted to fake successful/empty values.
116. Invalid helper persistent-snapshot data can panic via `expect`.
117. Helper shutdown ignores child kill failure.
118. Child command starts before helper listener binding succeeds.
119. Helper readiness accepts any existing path.
120. Full restore snapshots are passed as one command-line argument and can exceed argv limits.
121. Raw-history truncation may cut UTF-8/escape sequences.
122. Mixed-direction resize skips necessary reconstruction on one expanded axis.
123. Fixed 2 MiB history undermines configured scrollback after expansion.
124. Long session names can exceed Unix socket pathname limits.
125. Saved foreground command may be the shell, not actual foreground program (risk).
126. Non-Linux command reconstruction reparses lossy `ps` output (risk).
127. Corrupt snapshot blocks `up --rebuild` before rebuild behavior applies.
128. Snapshot structure is not validated against manifest before process creation.
129. Workspace save is not atomic across manifest and sidecar.
130. `admux save` rewrites manifests and destroys comments/formatting/unknown content.
131. Unknown manifest fields are silently discarded on save.
132. `DefaultHasher` is not a defined durable manifest-digest format.
133. Accepted manifest ratios are silently clamped.
134. Saving runtime foreground commands can write secrets into shareable manifests.
135. Existing `.admux/.gitignore` is not repaired to protect snapshots.
136. Workspace cwd paths are not validated before process creation.

### Copy buffers, tests, and documentation

137. Copy-mode `g`/`G` operate only on visible snapshot, not full scrollback.
138. Copy-mode `$` moves to pane width, not actual line end.
139. Page scrolling does not preserve absolute selection coordinates.
140. Copy cursor arithmetic can overflow `u16`.
141. Automatic paste-buffer names can collide with explicit names.
142. Explicit buffers bypass count limits.
143. Buffer sequence arithmetic can overflow.
144. External clipboard backend configuration/API is unimplemented; OSC52 is always used.
145. Integration tests can read/write a user's real state/config when only socket is isolated.
146. Client-side test `ADMUX_CONFIG` does not configure an already-running daemon.
147. Integration tests can kill daemons while leaving helpers/children alive.
148. Unit tests can leak helpers because `PaneProcess` lacks cleanup on drop.
149. Tests lack adversarial coverage for duplicate sessions, malformed/stalled IPC, concurrent startup, multi-client dimensions, Unicode, cleanup failures, corrupt persistence, permissions, and rollback.
150. README engineering-document links are machine-local absolute paths.
151. Documentation overstates behavior in several areas (input forwarding, rebuild, config, corruption recovery, and cleanup).

## Suggested validation order

1. Establish test isolation (separate socket, state, config, runtime directory) before any full test run.
2. Reproduce lifecycle, IPC-stall, duplicate-session, and multi-client viewport claims in dedicated temporary runtime directories.
3. Verify state/workspace corruption and `--rebuild` behavior without touching user state.
4. Validate terminal input, Unicode, rendering, and mouse behavior interactively or with PTY tests.
5. Convert confirmed items into scoped GitHub issues with exact revision, reproduction, expected/actual behavior, security impact, and regression test.

