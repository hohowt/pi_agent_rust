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
- [x] Verify raw mode restore on panic/error path.

## 3. Interactive Chat Port

- [x] Move conversation state rendering from string output to ratatui widgets.
- [x] Replace `bubbles::Viewport` with internal scroll state.
- [ ] Add scrollback support for mouse wheel, PgUp/PgDown, Home/End, bottom
      anchoring, and preserved scroll position while output streams.
  - [x] PgUp/PgDown/Home/End keyboard scrollback.
  - [x] Bottom anchoring and preserved scroll position while output streams.
  - [ ] Mouse wheel routing with terminal mouse-mode/native selection policy.
- [x] Replace `bubbles::TextArea` with a local editor state.
- [ ] Restore multi-line editing, cursor left/right/up/down, word movement,
      delete, history navigation, and Shift+Enter newline insertion.
  - [x] Multi-line editing and Shift+Enter newline insertion.
  - [x] Cursor left/right/up/down movement.
  - [x] Word movement and delete/backspace behavior.
  - [ ] Editing history navigation.
- [ ] Restore paste, bracketed paste, drag/drop file paths, quoted paths, and
      file/image reference expansion.
  - [x] Paste and bracketed paste insertion through editor state.
  - [x] Drag/drop `file://` and quoted path normalization.
  - [ ] File/image reference expansion into submitted content.
- [x] Port keyboard bindings to crossterm key events.
- [x] Restore slash command prefix completion and Tab completion.
- [x] Restore generic picker overlay skeleton.
- [ ] Port streaming assistant updates to frame-request redraws.
  - [x] Stream agent status/tool/retry/compaction events through frame-request redraws.
  - [ ] Stream assistant text deltas into the active assistant message.
- [ ] Render assistant text, thinking deltas, retry events, and compaction
      events incrementally instead of after the turn finishes.
  - [x] Render retry and compaction events incrementally.
  - [ ] Render assistant text and thinking deltas incrementally.
- [ ] Port tool rendering and thinking visibility.
  - [x] Render live tool progress state as status lines.
- [ ] Restore collapsed tool previews, full tool output expansion, tool
      progress state, per-tool error styling, and Shift+Tab thinking visibility.
  - [x] Live tool progress state.
  - [ ] Collapsed previews, expansion, error styling, and Shift+Tab thinking visibility.
- [ ] Restore startup OAuth hint and startup changelog rendering.

## 4. Markdown Rendering

- [ ] Replace `glamour` rendering with `pulldown-cmark` to ratatui lines.
- [ ] Preserve code block indentation and wrapping settings.
- [ ] Add markdown fixtures for headings, lists, code blocks, links, and CJK text.
- [ ] Evaluate useful pieces from `markdown-tui-explorer` before adding new code.

## 5. Overlays And Secondary UIs

- [ ] Port model selector overlay.
  - [x] Open a generic model picker for `/model`.
  - [ ] Restore model details, auth/credential warnings, provider grouping,
        current-model marker, thinking suffix handling, and direct Enter
        selection parity.
- [ ] Port session picker overlay.
  - [x] Open a generic session picker from `/history`, `/resume`, and double-Esc.
  - [ ] Load selected sessions and replace active conversation state.
  - [ ] Restore search, delete confirmation, empty/filter states, status
        messages, branch metadata, and cwd/all-session toggles.
- [ ] Port settings UI.
  - [ ] Restore editable settings rows for theme, language, queue mode,
        compaction, double-Esc action, editor padding, and autocomplete size.
  - [ ] Persist project/global settings through existing config paths.
- [ ] Port theme picker.
  - [x] Open a generic theme picker.
  - [x] Apply selected themes immediately and persist project theme.
- [ ] Port prompt template picker.
  - [x] Open a generic prompt template picker.
  - [x] Apply selected template content immediately.
- [ ] Port language picker.
  - [x] Open a generic language picker.
  - [ ] Persist language changes and refresh all UI/prompt text consistently.
- [ ] Port tree/branch selector.
  - [ ] Restore tree navigation, branch selection, fork behavior, summary
        prompts, and custom prompt flow.
- [ ] Port standalone `src/session_picker.rs`.
- [ ] Port config UI in `src/main.rs`.

## 5.1 Slash Command Parity

- [x] `/help`: show localized help text.
- [x] `/clear`: clear visible chat buffer.
- [x] `/model`: open generic picker and switch model when a model is selected.
- [x] `/thinking`: set thinking level by argument.
- [x] `/session`: show basic session status.
- [x] `/codegraph init|sync|status`: call the codegraph index API.
- [ ] `/login`: restore OAuth/API-key interactive login flows.
- [ ] `/logout`: restore provider credential removal flow.
- [ ] `/settings`: restore full interactive settings overlay.
- [x] `/theme`: apply selected theme, not just show/select the item.
- [x] `/template`: apply selected prompt template content immediately.
- [ ] `/history` and `/resume`: load selected session.
- [ ] `/tree`: restore full tree UI instead of basic status text.
- [ ] `/fork`: restore branch picker/fork flow.
- [ ] `/compact`: show compaction progress/events and resulting summary.
- [ ] `/reload`: reload models/resources, update autocomplete catalog, and show
      diagnostics.
- [ ] `/export`: restore default export path, HTML/JSON export, and status.
- [ ] `/copy`: restore clipboard support behind the clipboard feature.
- [ ] `/share`: restore share flow.
- [ ] `/changelog`: restore startup/current changelog UI.
- [ ] `/name`: keep current naming behavior and add visible confirmation/error
      parity with the old status bar.

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
- [ ] Add regression tests for single Esc, double Esc, picker Esc, and Ctrl+C.
- [ ] Add regression tests for slash prefix filtering and Tab completion.
- [ ] Add regression tests for model/session/theme/template picker selection.
- [ ] Add regression tests for session load after picker selection.
- [ ] Add regression tests for live tool progress rendering.
- [ ] Run `cargo fmt --all --check`.
- [ ] Run `cargo check --all-targets`.
- [ ] Run `cargo clippy --all-targets -- -D warnings`.
