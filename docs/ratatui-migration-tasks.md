# Ratatui Migration Tasks

## 1. Dependency And Skeleton

- [x] Add ratatui migration dependencies to `Cargo.toml`.
- [x] Switch the project toolchain to stable Rust 1.86 and `ratatui` 0.30.
- [x] Split ratatui runtime and console helpers into `crates/pi-tui`.
- [x] Add a `crates/pi-tui/` module skeleton.
- [x] Implement terminal setup/restore guard.
- [x] Implement Codex-style alternate scroll guard.
- [x] Implement event stream abstraction for key, paste, resize, focus, and draw.
- [x] Implement frame requester and frame rate limiter.
- [x] Add unit tests for frame request coalescing.

## 2. Minimal Ratatui App

- [x] Add a minimal ratatui app state and renderer.
- [x] Render header, conversation area, input area, and footer placeholders.
- [x] Wire interactive mode to the ratatui skeleton.
- [ ] Verify raw mode restore on panic/error path.

## 3. Interactive Chat Port

- [ ] Move conversation state rendering from string output to ratatui widgets.
- [ ] Replace `bubbles::Viewport` with internal scroll state.
- [ ] Replace `bubbles::TextArea` with a local editor state.
- [ ] Port keyboard bindings to crossterm key events.
- [ ] Port streaming assistant updates to frame-request redraws.
- [ ] Port tool rendering and thinking visibility.

## 4. Markdown Rendering

- [ ] Replace `glamour` rendering with `pulldown-cmark` to ratatui lines.
- [ ] Preserve code block indentation and wrapping settings.
- [ ] Add markdown fixtures for headings, lists, code blocks, links, and CJK text.
- [ ] Evaluate useful pieces from `markdown-tui-explorer` before adding new code.

## 5. Overlays And Secondary UIs

- [ ] Port model selector overlay.
- [ ] Port session picker overlay.
- [ ] Port settings UI.
- [ ] Port theme picker.
- [ ] Port tree/branch selector.
- [ ] Port standalone `src/session_picker.rs`.
- [ ] Port config UI in `src/main.rs`.

## 6. Theme Migration

- [ ] Replace `lipgloss::Style` fields in `pi-theme`.
- [ ] Replace `glamour::StyleConfig` fields in `pi-theme`.
- [ ] Expose ratatui-compatible style structs.
- [ ] Update docs for themes.

## 7. Console Output Migration

- [ ] Replace `rich_rust` usage in `src/tui.rs`.
- [ ] Replace Markdown output path outside interactive TUI.
- [ ] Replace syntax-highlighted output path or gate it through `syntect`.

## 8. Removal And Cleanup

- [x] Remove `bubbletea` imports from `src/`.
- [x] Remove `bubbles` imports from `src/`.
- [x] Remove `lipgloss` imports from `src/` and `crates/`.
- [x] Remove `glamour` imports from `src/` and `crates/`.
- [x] Remove `rich_rust` imports from `src/`.
- [x] Remove old dependencies from `Cargo.toml`.
- [x] Remove local patch comments for retired `charmed_*` crates.

## 9. Verification

- [ ] Add vt100 snapshot tests for main chat layout.
- [ ] Add resize tests for compact terminals.
- [ ] Add scroll/input fairness tests.
- [ ] Add paste and IME-adjacent input tests where possible.
- [ ] Run `cargo fmt --all --check`.
- [ ] Run `cargo check --all-targets`.
- [ ] Run `cargo clippy --all-targets -- -D warnings`.
