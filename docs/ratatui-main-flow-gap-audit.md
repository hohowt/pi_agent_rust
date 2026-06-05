# Ratatui Main Flow Gap Audit

Date: 2026-06-05

Scope: this audit covers the current ratatui interactive main flow in
`src/interactive.rs` and `crates/pi-tui/src/chat.rs`. It does not claim that the
feature is absent from the whole CLI. Several items below still exist in
non-interactive or legacy-adjacent code paths, but are unavailable or degraded
from the current ratatui chat surface.

## Summary

The current ratatui main flow is functional for basic chat, slash parsing, model
switching by explicit argument, thinking-level changes, basic session status,
codegraph init/sync/status, and basic tool/status event streaming.

The highest-impact regressions are session switching, settings/theme/template
application, credential flows, export/copy/share, full tree/fork flows, and live
assistant/thinking rendering. Most affected features are not hard failures; they
show compatibility messages, open generic pickers without applying the selected
object, or display read-only status where the previous UI performed an action.

## Unavailable In Ratatui Main Flow

| Feature | Current behavior | Evidence | Impact |
|---|---|---|---|
| `/login` | Returns a status message telling the user to use non-interactive setup. No OAuth/API-key interactive flow runs. | `src/interactive.rs:400-406`, `src/interactive.rs:863-867` | Users cannot authenticate from the interactive TUI. |
| `/logout` | Returns a status message. No provider credential removal or confirmation flow runs. | `src/interactive.rs:400-406`, `src/interactive.rs:868-870` | Users cannot remove credentials from the interactive TUI. |
| `/export` | Returns a status message. Interactive default export path and HTML/JSON selection are not wired. | `src/interactive.rs:400-406`, `src/interactive.rs:871` | Export exists elsewhere, but not from the ratatui main flow. |
| `/copy` | Returns a status message. Clipboard integration is not wired. | `src/interactive.rs:400-406`, `src/interactive.rs:872` | Users cannot copy the current transcript from the TUI command. |
| `/share` | Returns a status message. No upload/share flow runs. | `src/interactive.rs:400-406`, `src/interactive.rs:876-878` | Interactive sharing is unavailable. |
| `/fork` | Returns a status message pointing users to `/tree`. No branch picker or fork is performed. | `src/interactive.rs:400-406`, `src/interactive.rs:873-875` | Conversation branching from TUI is unavailable. |

## Present But Degraded

| Feature | Current behavior | Evidence | Missing behavior |
|---|---|---|---|
| `/history`, `/resume`, double-Esc session picker | Opens a generic picker. Selecting a session submits `/resume <path>`, but the handler only prints “session load will run after ratatui session switch is wired.” | `crates/pi-tui/src/chat.rs:247-255`, `src/interactive.rs:826-852` | Load selected session, replace active conversation state, restore branch metadata and search/delete/filter behavior. |
| `/settings` | Displays a read-only settings summary. | `src/interactive.rs:384`, `src/interactive.rs:625-633` | Editable settings overlay, queue/compaction/double-Esc/editor/autocomplete settings, project/global persistence. |
| `/theme` | Opens a generic picker. Selecting a theme submits `/theme <name>`, but the handler returns “theme switch pending.” | `src/interactive.rs:395`, `src/interactive.rs:711-727` | Immediate theme application and project theme persistence. |
| `/template` | Opens a generic picker. Selecting a template submits `/template <name>`, but the handler returns “prompt template pending insertion.” | `src/interactive.rs:396`, `src/interactive.rs:740-764` | Insert/apply selected template content into the editor. |
| `/language` | Picker exists and explicit `zh|en` changes in-memory UI labels/status. | `src/interactive.rs:399`, `src/interactive.rs:789-823` | Persist language changes and refresh all UI/prompt text consistently through config paths. |
| `/tree` | Displays basic leaf/path counts. | `src/interactive.rs:389`, `src/interactive.rs:610-622` | Full tree navigation, branch selection, summary prompts, and custom fork prompt flow. |
| `/compact` | Runs compaction and returns only “completed”; event callback is discarded. | `src/interactive.rs:397`, `src/interactive.rs:766-768` | Progress/events and resulting summary in the TUI. |
| `/reload` | Displays current resource counts. It does not reload resources. | `src/interactive.rs:385`, `src/interactive.rs:636-642` | Reload models/resources, refresh autocomplete catalog, show diagnostics. |
| `/model` picker | Generic picker can switch model by selected value. | `src/interactive.rs:381`, `src/interactive.rs:438-484` | Provider grouping, auth warnings, current marker, richer details, thinking suffix parity, direct Enter selection parity. |
| `/changelog` | Prints first 80 changelog lines via unavailable-command path. | `src/interactive.rs:400-406`, `src/interactive.rs:879-883` | Startup/current changelog UI parity. |
| `/name` | Sets name and prints confirmation. | `src/interactive.rs:398`, `src/interactive.rs:771-786` | Old status-bar style confirmation/error parity. |

## Input And Rendering Gaps

| Area | Current behavior | Evidence | Missing behavior |
|---|---|---|---|
| Assistant streaming | Tool/status/retry/compaction lines can stream via frame redraws, but assistant text is only appended after the provider call completes. | `src/interactive.rs:345-368`, `docs/ratatui-migration-tasks.md:46-52` | Stream assistant text deltas into the active assistant message; render thinking deltas incrementally. |
| Tool rendering | Live tool progress appears as status lines. | `src/interactive.rs:888-966`, `docs/ratatui-migration-tasks.md:53-58` | Collapsed previews, full output expansion, per-tool error styling, thinking visibility toggle. |
| Markdown rendering | Conversation lines are ratatui spans/paragraphs, but assistant markdown is still plain text. | `docs/ratatui-migration-tasks.md:61-66` | Headings, lists, code fences, links, CJK wrapping, code-like presentation via pulldown-cmark. |
| File/image reference expansion | Paste and quoted `file://` path normalization exist, but submitted content is still sent as a single text block. | `src/interactive.rs:347-349`, `docs/ratatui-migration-tasks.md:38-42` | Expand file references and attach image content blocks. |
| Editing history | Local editor supports multiline/cursor/word/delete editing, but there is no input history source. | `docs/ratatui-migration-tasks.md:31-37` | Up/down history navigation for prior prompts. |
| Mouse wheel | Keyboard scrollback exists. Mouse wheel policy is not fully restored because native selection/copy and mouse-mode routing remain unresolved. | `docs/ratatui-migration-tasks.md:26-30` | Mouse wheel routing with terminal mouse-mode/native selection policy. |

## Secondary UI Gaps

| Area | Current behavior | Evidence | Missing behavior |
|---|---|---|---|
| Standalone session picker | No `src/session_picker.rs` exists in the current tree. | `docs/ratatui-migration-tasks.md:96` | Port standalone picker entry point. |
| Config UI in `src/main.rs` | Existing config TUI path still needs ratatui parity audit/port. | `docs/ratatui-migration-tasks.md:97`, `src/main.rs:2394-2395` | Replace old/non-ratatui config UI behavior with ratatui flow. |
| Theme system migration | `pi-theme` migration remains open. | `docs/ratatui-migration-tasks.md:125-130` | Ratatui-compatible style structs and theme docs. |
| Console output migration | Non-interactive console markdown/syntax paths remain open. | `docs/ratatui-migration-tasks.md:132-136` | Remove/replace `rich_rust` console rendering paths. |

## Priority Order

1. Session switching: `/history`, `/resume`, and double-Esc currently look
   usable but do not load the selected session.
2. Settings/theme/template application: pickers create a strong expectation of
   an applied change, but selection currently returns a placeholder/status.
3. Credential and data egress flows: `/login`, `/logout`, `/export`, `/copy`,
   `/share` are explicit unavailable-command paths.
4. Tree/fork parity: `/tree` is read-only summary; `/fork` is unavailable.
5. Live assistant/thinking rendering and markdown: affects the core chat
   experience even when commands are not used.

## Audit Commands

The gap list above was built from these local checks:

```bash
rg -n "format_command_unavailable|会话加载将在|当前不可用|需要 .*流程|not yet|unavailable|将在 .*接入后|当前请" src/interactive.rs crates/pi-tui/src/chat.rs src/main.rs docs/ratatui-migration*.md -S
rg -n "login|logout|history|resume|export|copy|share|fork|theme|template|language|settings|tree|compact|reload|changelog" src/interactive.rs crates/pi-tui/src/chat.rs src/main.rs -S
```
