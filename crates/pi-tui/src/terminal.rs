use std::fmt;
use std::io::{self, Write};

use crossterm::Command;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

pub struct TerminalModeGuard {
    alternate_screen: bool,
    bracketed_paste: bool,
    cursor_hidden: bool,
    focus_change: bool,
    raw_mode: bool,
    ops: Box<dyn TerminalModeOps>,
}

impl TerminalModeGuard {
    pub(crate) fn enter() -> io::Result<Self> {
        Self::enter_with(Box::new(CrosstermTerminalModeOps))
    }

    fn enter_with(ops: Box<dyn TerminalModeOps>) -> io::Result<Self> {
        let mut guard = Self {
            alternate_screen: false,
            bracketed_paste: false,
            cursor_hidden: false,
            focus_change: false,
            raw_mode: false,
            ops,
        };

        guard.ops.enable_raw_mode()?;
        guard.raw_mode = true;
        guard.ops.enter_alternate_screen()?;
        guard.alternate_screen = true;
        guard.ops.hide_cursor()?;
        guard.cursor_hidden = true;
        guard.ops.enable_bracketed_paste()?;
        guard.bracketed_paste = true;
        guard.ops.enable_focus_change()?;
        guard.focus_change = true;
        Ok(guard)
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        if self.focus_change {
            self.ops.disable_focus_change();
        }
        if self.bracketed_paste {
            self.ops.disable_bracketed_paste();
        }
        if self.cursor_hidden {
            self.ops.show_cursor();
        }
        if self.alternate_screen {
            self.ops.leave_alternate_screen();
        }
        if self.raw_mode {
            self.ops.disable_raw_mode();
        }
    }
}

trait TerminalModeOps {
    fn enable_raw_mode(&mut self) -> io::Result<()>;
    fn enter_alternate_screen(&mut self) -> io::Result<()>;
    fn hide_cursor(&mut self) -> io::Result<()>;
    fn enable_bracketed_paste(&mut self) -> io::Result<()>;
    fn enable_focus_change(&mut self) -> io::Result<()>;
    fn disable_focus_change(&mut self);
    fn disable_bracketed_paste(&mut self);
    fn show_cursor(&mut self);
    fn leave_alternate_screen(&mut self);
    fn disable_raw_mode(&mut self);
}

struct CrosstermTerminalModeOps;

impl TerminalModeOps for CrosstermTerminalModeOps {
    fn enable_raw_mode(&mut self) -> io::Result<()> {
        enable_raw_mode()
    }

    fn enter_alternate_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnterAlternateScreen)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        execute!(io::stdout(), Hide)
    }

    fn enable_bracketed_paste(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnableBracketedPaste)
    }

    fn enable_focus_change(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnableFocusChange)
    }

    fn disable_focus_change(&mut self) {
        let _ = execute!(io::stdout(), DisableFocusChange);
    }

    fn disable_bracketed_paste(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste);
    }

    fn show_cursor(&mut self) {
        let _ = execute!(io::stdout(), Show);
    }

    fn leave_alternate_screen(&mut self) {
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }

    fn disable_raw_mode(&mut self) {
        let _ = disable_raw_mode();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnableAlternateScroll;

impl Command for EnableAlternateScroll {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[?1007h")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisableAlternateScroll;

impl Command for DisableAlternateScroll {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[?1007l")
    }
}

pub struct AlternateScrollGuard;

impl AlternateScrollGuard {
    pub(crate) fn enable() -> io::Result<Self> {
        let mut stdout = io::stdout();
        execute!(stdout, EnableAlternateScroll)?;
        stdout.flush()?;
        Ok(Self)
    }
}

impl Drop for AlternateScrollGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = execute!(stdout, DisableAlternateScroll);
        let _ = stdout.flush();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::panic::{self, AssertUnwindSafe};
    use std::rc::Rc;

    use super::{TerminalModeGuard, TerminalModeOps};

    #[derive(Default)]
    struct TestTerminalModeState {
        calls: Vec<&'static str>,
        fail_on: Option<&'static str>,
    }

    #[derive(Clone)]
    struct TestTerminalModeOps {
        state: Rc<RefCell<TestTerminalModeState>>,
    }

    impl TestTerminalModeOps {
        fn new(fail_on: Option<&'static str>) -> (Self, Rc<RefCell<TestTerminalModeState>>) {
            let state = Rc::new(RefCell::new(TestTerminalModeState {
                calls: Vec::new(),
                fail_on,
            }));
            (
                Self {
                    state: Rc::clone(&state),
                },
                state,
            )
        }

        fn step(&mut self, name: &'static str) -> std::io::Result<()> {
            let mut state = self.state.borrow_mut();
            state.calls.push(name);
            if state.fail_on == Some(name) {
                return Err(std::io::Error::other(name));
            }
            Ok(())
        }
    }

    impl TerminalModeOps for TestTerminalModeOps {
        fn enable_raw_mode(&mut self) -> std::io::Result<()> {
            self.step("enable_raw_mode")
        }

        fn enter_alternate_screen(&mut self) -> std::io::Result<()> {
            self.step("enter_alternate_screen")
        }

        fn hide_cursor(&mut self) -> std::io::Result<()> {
            self.step("hide_cursor")
        }

        fn enable_bracketed_paste(&mut self) -> std::io::Result<()> {
            self.step("enable_bracketed_paste")
        }

        fn enable_focus_change(&mut self) -> std::io::Result<()> {
            self.step("enable_focus_change")
        }

        fn disable_focus_change(&mut self) {
            let _ = self.step("disable_focus_change");
        }

        fn disable_bracketed_paste(&mut self) {
            let _ = self.step("disable_bracketed_paste");
        }

        fn show_cursor(&mut self) {
            let _ = self.step("show_cursor");
        }

        fn leave_alternate_screen(&mut self) {
            let _ = self.step("leave_alternate_screen");
        }

        fn disable_raw_mode(&mut self) {
            let _ = self.step("disable_raw_mode");
        }
    }

    #[test]
    fn restores_raw_mode_when_enter_fails_after_raw_mode() {
        let (ops, state) = TestTerminalModeOps::new(Some("enable_bracketed_paste"));

        let result = TerminalModeGuard::enter_with(Box::new(ops));

        assert!(result.is_err());
        assert_eq!(
            state.borrow().calls,
            vec![
                "enable_raw_mode",
                "enter_alternate_screen",
                "hide_cursor",
                "enable_bracketed_paste",
                "show_cursor",
                "leave_alternate_screen",
                "disable_raw_mode",
            ]
        );
    }

    #[test]
    fn restores_terminal_modes_during_panic_unwind() {
        let (ops, state) = TestTerminalModeOps::new(None);

        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = TerminalModeGuard::enter_with(Box::new(ops)).expect("enter terminal mode");
            panic!("simulate panic while raw mode is enabled");
        }));

        assert!(result.is_err());
        assert_eq!(
            state.borrow().calls,
            vec![
                "enable_raw_mode",
                "enter_alternate_screen",
                "hide_cursor",
                "enable_bracketed_paste",
                "enable_focus_change",
                "disable_focus_change",
                "disable_bracketed_paste",
                "show_cursor",
                "leave_alternate_screen",
                "disable_raw_mode",
            ]
        );
    }
}
