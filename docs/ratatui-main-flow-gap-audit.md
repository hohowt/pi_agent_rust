# Ratatui Main Flow Gap Audit

Date: 2026-06-05

Scope: this audit covers the current ratatui interactive main flow in
`src/interactive.rs` and `crates/pi-tui/src/chat.rs`. It does not claim that the
feature is absent from the whole CLI. Several items below still exist in
non-interactive or legacy-adjacent code paths, but are unavailable or degraded
from the current ratatui chat surface.

## Summary

The current ratatui main flow is functional for basic chat, slash parsing, model
switching, thinking-level changes, basic session status, codegraph
init/sync/status, basic tool/status event streaming, session resume, theme
selection, prompt template execution, language selection, file/image reference
expansion, assistant text streaming, and thinking delta streaming.

The highest-impact remaining regressions are credential flows, full tree/fork
flows, the editable settings overlay, and richer model/session picker parity.
Most affected features are not hard failures; they show compatibility messages,
open generic pickers without the full codex/pi-tui affordances, or display
read-only status where the previous UI performed a richer action.

## Now Available In Ratatui Main Flow

| Feature | Current behavior | Evidence |
|---|---|---|
| `/history`, `/resume`, double-Esc session picker | Opens a generic picker. Selecting a session submits `/resume <path>` and replaces active visible conversation and agent history. | `src/interactive.rs:1077-1142`, `tests/interactive_session_resume.rs:48-111` |
| `/theme` | Opens a generic picker. Selecting a theme applies it immediately and persists the project theme. | `src/interactive.rs:884-929` |
| `/template` | Opens a generic picker. Selecting a template expands it and submits the resulting prompt immediately. | `src/interactive.rs:938-988` |
| `/language` | Opens a generic picker or accepts an explicit language, persists the setting, and refreshes footer/options text. | `src/interactive.rs:1013-1075` |
| Assistant streaming | Assistant text deltas stream into the active assistant message; thinking deltas stream into a Thinking line. | `src/interactive.rs:361-436`, `crates/pi-tui/src/chat.rs:170-265` |
| File/image reference expansion | Submitted `@path`, single path, and `file://` references are expanded into text/image content blocks. | `src/interactive.rs:438-520`, `tests/interactive_session_resume.rs:177-223` |
| `/reload` | Reloads resources, auth, and model registry, refreshes footer/autocomplete state, and reports diagnostics. | `src/interactive.rs:542`, `src/interactive.rs:884-940` |
| `/compact` | Runs compaction, surfaces lifecycle events, shows failure/no-op status, and includes resulting summary/tokens/file counts when available. | `src/interactive.rs`, `tests/interactive_session_resume.rs` |
| `/export` | Exports the current session from the TUI with a default HTML path; `.json` paths export the current-path messages as JSON. | `src/interactive.rs`, `tests/interactive_session_resume.rs` |
| `/copy` | Copies the latest assistant text when built with `clipboard`; default builds show a clear feature-gated failure status. | `src/interactive.rs`, `tests/interactive_session_resume.rs` |
| `/share` | Creates a private/public GitHub gist through `gh`, uploads exported session HTML, and displays the Pi share viewer URL. | `src/interactive.rs`, `tests/interactive_session_resume.rs` |
| `/changelog` | Shows startup changelog status when appropriate and opens a version picker for current changelog entries. | `src/interactive.rs`, `tests/interactive_session_resume.rs` |
| `/logout` | Opens a provider picker for saved credentials; selecting a provider removes that credential immediately and refreshes auth/model state. | `src/interactive.rs`, `tests/interactive_session_resume.rs` |
| Startup auth hint | Shows startup credential guidance for the current model when auth is missing, while respecting quiet startup. | `src/interactive.rs`, `tests/interactive_session_resume.rs` |
| Markdown rendering | Assistant messages are rendered through the ratatui markdown path for headings, lists, links, code fences, and CJK text. | `crates/pi-tui/src/chat.rs:1144-1352`, `crates/pi-tui/src/chat.rs:1959-1989` |
| Editing history | Up/down navigation restores previous prompts and the current draft. | `crates/pi-tui/src/chat.rs:318-435`, `crates/pi-tui/src/chat.rs:1811-1855` |
| Mouse wheel | Mouse wheel routing is wired through the terminal mouse capture policy. | `crates/pi-tui/src/chat.rs:289-303`, `crates/pi-tui/src/terminal.rs:29-77` |

## Unavailable In Ratatui Main Flow

| Feature | Current behavior | Evidence | Impact |
|---|---|---|---|
| `/login` | Returns a status message telling the user to use non-interactive setup. No OAuth/API-key interactive flow runs. | `src/interactive.rs:400-406`, `src/interactive.rs:863-867` | Users cannot authenticate from the interactive TUI. |
| `/fork` | Returns a status message pointing users to `/tree`. No branch picker or fork is performed. | `src/interactive.rs:400-406`, `src/interactive.rs:873-875` | Conversation branching from TUI is unavailable. |

## Present But Degraded

| Feature | Current behavior | Evidence | Missing behavior |
|---|---|---|---|
| `/settings` | Displays a read-only settings summary. | `src/interactive.rs:384`, `src/interactive.rs:625-633` | Editable settings overlay, queue/compaction/double-Esc/editor/autocomplete settings, project/global persistence. |
| `/tree` | Displays basic leaf/path counts. | `src/interactive.rs:389`, `src/interactive.rs:610-622` | Full tree navigation, branch selection, summary prompts, and custom fork prompt flow. |
| `/model` picker | Generic picker can switch model by selected value. | `src/interactive.rs:381`, `src/interactive.rs:438-484` | Provider grouping, auth warnings, current marker, richer details, thinking suffix parity, direct Enter selection parity. |
| `/name` | Sets name and prints confirmation. | `src/interactive.rs:398`, `src/interactive.rs:771-786` | Old status-bar style confirmation/error parity. |

## Input And Rendering Gaps

| Area | Current behavior | Evidence | Missing behavior |
|---|---|---|---|
| Tool rendering | Live tool progress appears as status lines. | `src/interactive.rs:888-966`, `docs/ratatui-migration-tasks.md:53-58` | Collapsed previews, full output expansion, per-tool error styling, thinking visibility toggle. |

## Secondary UI Gaps

| Area | Current behavior | Evidence | Missing behavior |
|---|---|---|---|
| Standalone session picker | No `src/session_picker.rs` exists in the current tree. | `docs/ratatui-migration-tasks.md:96` | Port standalone picker entry point. |
| Config UI in `src/main.rs` | Existing config TUI path still needs ratatui parity audit/port. | `docs/ratatui-migration-tasks.md:97`, `src/main.rs:2394-2395` | Replace old/non-ratatui config UI behavior with ratatui flow. |
| Theme system migration | `pi-theme` migration remains open. | `docs/ratatui-migration-tasks.md:125-130` | Ratatui-compatible style structs and theme docs. |
| Console output migration | Non-interactive console markdown/syntax paths remain open. | `docs/ratatui-migration-tasks.md:132-136` | Remove/replace `rich_rust` console rendering paths. |

## Priority Order

1. Credential flow: `/login` is an explicit unavailable-command path.
2. Tree/fork parity: `/tree` is read-only summary; `/fork` is unavailable.
3. Settings parity: `/settings` exists but still lacks the full editable
   interactive flow and persistence controls.
4. Picker richness: model/session pickers work, but still lack grouping,
   metadata, delete confirmation, filter states, and cwd/all-session toggles.
5. Tool presentation: live progress exists, but collapsed previews, expansion,
   per-tool error styling, and Shift+Tab thinking visibility remain open.

## Audit Commands

The gap list above was built from these local checks:

```bash
rg -n "format_command_unavailable|会话加载将在|当前不可用|需要 .*流程|not yet|unavailable|将在 .*接入后|当前请" src/interactive.rs crates/pi-tui/src/chat.rs src/main.rs docs/ratatui-migration*.md -S
rg -n "login|logout|history|resume|export|copy|share|fork|theme|template|language|settings|tree|compact|reload|changelog" src/interactive.rs crates/pi-tui/src/chat.rs src/main.rs -S
```
