# Schema diff: herdr 0.8.2 / protocol 20 → 0.9.0 / protocol 22

Source: `herdr --session test090 api schema --json` (protocol 22, herdr 0.9.0) vs `crates/board-herdr/tests/fixtures/schema.json` (protocol 20, herdr 0.8.2). Dump saved to `docs/herdr-0.9.0-schema.json` (269 KB, `herdr api schema --output`).

## Header

| field | 0.8.2 | 0.9.0 |
|---|---|---|
| `protocol` | 20 | **22** |
| `schema_version` | 1 | 1 |
| `version` (via `ping`) | 0.8.2 | **0.9.0** |
| `capabilities` (ping) | `live_handoff`, `detached_server_daemon` | adds `endpoint_protocol_generation: 1`, `surface_interest: true`, `health_check: true` |

`crates/board-herdr/src/lib.rs` updated: `SUPPORTED_HERDR_VERSION="0.9.0"`, `SUPPORTED_HERDR_PROTOCOL=22`.

## Request methods

91 methods → 102 methods. **No removals, no renames.**

New (all additive, not used by board-herdr):

- `client_shell.surface.set`
- `command.invoke`
- `integration.list`
- `pane.copy_motion`
- `pane.copy_search`
- `pane.edit_scrollback`
- `pane.link.activate`
- `pane.scroll`
- `pane.selection.read`
- `product_announcement.dismiss`
- `release_notes.dismiss`

## Parameter type diffs for board-relevant methods

All methods board-herdr calls are unchanged except for **additive optional fields** — old wire shapes remain valid on 0.9.0.

| method | old `$ref` | change |
|---|---|---|
| `workspace.create` | `WorkspaceCreateParams` `{cwd, env, focus, label}` | adds optional `source_workspace_id: string|null` (“workspace whose focused pane supplies the `follow` cwd policy”) |
| `workspace.close` | `WorkspaceTarget {workspace_id}` | now `WorkspaceCloseParams {workspace_id, close_group?: bool}` — old `{workspace_id}` still valid (`close_group` omitted) |
| `worktree.create` | `WorktreeCreateParams` | adds optional `trust_repository: bool` |
| `worktree.list` | `WorktreeListParams` | adds optional `trust_repository: bool` |
| `worktree.open` | `WorktreeOpenParams` | adds optional `trust_repository: bool` |
| `worktree.remove` | `WorktreeRemoveParams` | adds optional `trust_repository: bool` |

All other board-used methods byte-identical between 20 and 22:

`workspace.list`, `tab.create`, `tab.list`, `tab.rename`, `agent.start`, `agent.get`, `agent.prompt`, `agent.wait`, `pane.list`, `pane.split`, `pane.read`, `pane.send_text`, `pane.send_keys`, `pane.close`, `pane.get`, `pane.focus`, `pane.rename`, `pane.layout`, `notification.show`, `session.snapshot`, `ping`, `events.subscribe` — verified via `extract_methods` diff (JSON-equal).

New request `$defs` (not board-used): `ClientShellSurfaceSetParams`, `CommandInvokeParams`, `PaneCopyMotion`, `PaneCopyMotionParams`, `PaneCopySearchDirection`, `PaneCopySearchParams`, `PaneLinkActivateParams`, `PaneScrollParams`, `PaneSelectionReadParams`, `PaneTextPoint`, `PaneTextRange`, `ProductAnnouncementDismissParams`, `ReleaseNotesDismissParams`, `WorkspaceCloseParams` (see above).

## Success response (`ResponseResult` `oneOf`)

58 `oneOf` types → 64. Additions only:

- `client_shell_surface_set`
- `integration_list`
- `pane_copy_motion`
- `pane_copy_search`
- `pane_link_activated`
- `pane_selection`

No removed or renamed result type. All board-decoded result types (`pong`, `session_snapshot`, `workspace_created`, `workspace_list`, `tab_created`, `tab_list`, `tab_info`, `pane_info`, `pane_list`, `pane_layout`, `pane_read`, `agent_started`, `agent_info`, `notification_show`, `ok`, etc.) are structurally identical.

### Definition changes

- `ServerCapabilities` (inside `pong`): adds optional `endpoint_protocol_generation: uint32|null`, `health_check: bool (default false)`, `surface_interest: bool (default false)` — all `#[serde(default)]` tolerant.
- New `$defs`: `IntegrationInfo`, `IntegrationState`, `PaneTextPoint`, `PaneTextRange` (payload for the new methods).

**Board-relevant payload `$defs` unchanged (JSON-equal):** `WorkspaceInfo`, `TabInfo`, `PaneInfo`, `AgentInfo`, `AgentSessionInfo`, `SessionSnapshot`, `PaneLayoutSnapshot`, `PaneLayoutPane`, `PaneLayoutRect`, `PaneLayoutSplit`, `PaneReadResult`, etc.

## Event schemas

`event` and `subscription_event` are JSON-equal between 20 and 22 (no def added/removed/changed). Wire shape `{"event":"<kind>","data":{"type":"<kind>",...}}` and subscription kinds (`pane.agent_status_changed` requires `pane_id`, global `pane.exited`/`pane.closed`) are unchanged.

## Error response

Identical.

## Impact on board-herdr

- **Must change:** `SUPPORTED_HERDR_VERSION` / `SUPPORTED_HERDR_PROTOCOL` and doc comments referencing `0.8.2/20` → `0.9.0/22`.
- **Compatible as-is** (due to `#[serde(default)]` + `skip_serializing_if` + `#[serde(default)]` on caps): `PaneInfo`, `AgentInfo`, `WorkspaceInfo`, `TabInfo`, `SessionSnapshot`, `Pong`, `Layout`, `PaneReadResult`, etc. Live-verified: `ping`, `workspace.list`, `pane.list`, `pane.read`, `pane.split`, `agent.start`, `tab.create`, `workspace.create`, `notification.show`, `session.snapshot`, `pane.layout` all succeed on 0.9.0 with old param shapes.
- **Trivial compat:** `workspace.close` now accepts optional `close_group`; existing `json!({"workspace_id": id})` call remains valid.
- **New params** (`source_workspace_id`, `trust_repository`, etc.) are board-opt-in; omitted by current code and accepted by server.

## Files changed (board-herdr)

- `crates/board-herdr/src/lib.rs` — bump constants + header comment
- `crates/board-herdr/src/client.rs` — update compat-adapter docs + `tab.rename`/`pane.get` doc refs (19→22)
- `crates/board-herdr/src/types.rs` — header + `AgentSession`/`agent_session` docs + `AgentStarted` doc
- `crates/board-herdr/src/params.rs` — header (protocol 19→22) + `AgentStartParams`/`PaneSplitParams`/`TabRenameParams` doc tags
- `crates/board-herdr/src/events/mod.rs` — subscription quirk comment protocol 19→22
- `crates/board-herdr/tests/socket.rs` — fixtures for `protocol_gate_accepts_exact` + `deprecated_protocol_adapter_accepts` + `is_live_true_on_pong` now expect `0.9.0/22`
- `docs/herdr-0.9.0-schema.json` — fresh dump (new)
