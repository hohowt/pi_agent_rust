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
