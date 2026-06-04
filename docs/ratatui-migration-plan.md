# Ratatui Migration Plan

## Goal

Replace the current TUI stack built on `charmed-bubbletea`, `charmed-bubbles`,
`charmed-lipgloss`, `charmed-glamour`, and `rich_rust` with a smaller, reliable
stack based on `ratatui` and `crossterm`, following the event/render separation
used by Codex Rust TUI.

## Why

The current TUI stack is not a mature ecosystem dependency and has already shown
event-loop and rendering coupling problems: scrolling, typing, and redraws all
compete inside the same synchronous `bubbletea::Program` loop. Codex avoids this
by separating terminal input, application events, and frame drawing.

## Target Stack

- `ratatui`: terminal rendering and widgets.
- `crossterm` with `bracketed-paste` and `event-stream`: terminal modes and input.
- `tokio-stream` with `sync`: async stream adapters for input/draw events.
- `tokio-util` with `time`: timeout helpers and testable timing.
- `pulldown-cmark`: Markdown parsing.
- `textwrap`: text wrapping.
- `unicode-width` and `unicode-segmentation`: terminal text measurement.
- `supports-color`: terminal color capability detection.
- `arboard`: clipboard support, retained behind the existing feature.
- `vt100`: terminal rendering tests.

## Architecture

The replacement TUI should use a Codex-style split:

1. Terminal setup/restore owns raw mode, alternate screen, bracketed paste, focus,
   and alternate scroll.
2. Event stream maps crossterm input to internal TUI events. Mouse events are not
   part of the default path.
3. Frame requester coalesces redraw requests and enforces a frame budget.
4. App state updates are cheap and request redraws instead of rendering directly.
5. Renderer consumes app state and writes a ratatui frame.
6. Markdown rendering is converted to `ratatui::text::{Line, Span}` using
   `pulldown-cmark` or a small markdown renderer inspired by
   `markdown-tui-explorer`.

## Migration Strategy

The old TUI stack has been removed instead of kept behind a compatibility
feature, because the charmed crates do not compile on stable Rust 1.86.

The first milestone is a standalone `pi-tui` crate that can initialize terminal
modes, process input events, coalesce draw requests, and render a minimal chat
frame. The second milestone ports the full interactive chat surface.

## Current Regression Gaps

The ratatui port has the dependency and skeleton work in place, but the current
interactive surface is still behind the pre-migration TUI in these areas:

1. Conversation viewport: no full scrollback model, mouse-wheel routing,
   PgUp/PgDown/Home/End behavior, scroll anchoring, or preserved scroll position
   while output streams.
2. Editor: no full multi-line textarea, cursor movement, editing history,
   newline insertion, selection-aware paste handling, file drag/drop parsing, or
   file/image reference expansion.
3. Streaming: assistant deltas, tool progress, auto-retry, and compaction events
   are not rendered live with incremental redraws.
4. Tool display: tool calls/results are not rendered with the old collapsed
   preview, progress state, detailed output formatting, or thinking visibility
   toggle behavior.
5. Markdown: assistant output is plain text; headings, lists, code fences,
   links, wrapping, CJK width, and syntax-like code presentation are not yet
   equivalent to the old glamour path.
6. Slash commands: command completion is partially restored, but command-specific
   interactive flows are still missing or degraded for login/logout, settings,
   theme apply, template insertion, export, copy, share, fork, compact status,
   changelog, and resource reload.
7. Pickers and overlays: the generic picker exists, but the old model selector,
   session picker, settings UI, theme picker, branch selector, and tree UI have
   not been fully ported with their previous filtering, details, deletion,
   confirmation, persistence, and navigation behavior.
8. Session switching: `/history` and double-Esc can show a picker, but selecting
   a session does not yet load and replace the active session state.
9. Settings persistence: language/theme/settings changes are not all persisted
   through the same project/global settings paths as before.
10. Status line: the footer is back, but token usage, VCS information, queue
    state, spinner/progress hints, thinking/model status, and contextual key
    hints are not yet at parity.
11. Mouse and terminal behavior: native selection/copy, tmux wheel forwarding,
    alternate-scroll behavior, IME placement, resize handling, and bracketed
    paste need regression coverage against the ratatui implementation.
12. Startup and secondary UI flows: OAuth hint, changelog, session recovery,
    standalone session picker, and non-interactive config UI parity are still
    incomplete.

## Completion Criteria

- Interactive chat runs on ratatui without `bubbletea` or `bubbles`.
- Main session picker and config UI no longer use `bubbletea`.
- Markdown rendering no longer uses `glamour`.
- The general console path no longer uses `rich_rust`.
- `crates/pi-theme` no longer exposes `lipgloss` or `glamour` types.
- `Cargo.toml` no longer depends on `bubbletea`, `bubbles`, `lipgloss`,
  `glamour`, or `rich_rust`.
- Keyboard input, IME behavior, scrolling, selection/copy, paste, resize, and
  streaming output have regression coverage.
