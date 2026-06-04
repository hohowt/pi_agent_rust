//! Ratatui-based terminal UI primitives for Pi.

mod chat;
mod console;
mod event;
mod frame;
mod terminal;

pub use chat::{ChatAction, ChatLine, ChatOptions, run_minimal_chat_loop};
pub use console::{PiConsole, SpinnerStyle};
