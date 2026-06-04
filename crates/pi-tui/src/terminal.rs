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
}

impl TerminalModeGuard {
    pub(crate) fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            Hide,
            EnableBracketedPaste,
            EnableFocusChange
        )?;
        Ok(Self {
            alternate_screen: true,
        })
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = execute!(stdout, DisableFocusChange, DisableBracketedPaste, Show);
        if self.alternate_screen {
            let _ = execute!(stdout, LeaveAlternateScreen);
        }
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
