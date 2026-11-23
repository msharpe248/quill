//! Taking over the terminal, and giving it back.
//!
//! Standalone, Quill puts the terminal into raw mode itself. Inside the app
//! shell it cannot: the macOS App Sandbox lets a program read its terminal's
//! modes but not change them. There the shell, which owns the other end of
//! the PTY, sets raw mode before it starts the core, and the core accepts a
//! terminal that is already raw.

use std::io::{self, Stdout, stdin, stdout};
use std::panic;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use rustix::termios::LocalModes;

use crate::anchored::Anchored;

/// The terminal Quill draws on: the real one, through a backend that does
/// not take its word for how wide a character is.
pub type Screen = Terminal<Anchored<CrosstermBackend<Stdout>>>;

/// We switched raw mode on, so we switch it off.
static OWNS_RAW_MODE: AtomicBool = AtomicBool::new(false);
static ENHANCED_KEYBOARD: AtomicBool = AtomicBool::new(false);
static BRACKETED_PASTE: AtomicBool = AtomicBool::new(false);
static MOUSE: AtomicBool = AtomicBool::new(false);
static ON_ALTERNATE_SCREEN: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK: Once = Once::new();

/// Raw mode, the alternate screen and, where offered, the Kitty keyboard
/// protocol. Undo with [`leave`]; a panic undoes it too.
pub fn enter(mouse: bool) -> io::Result<Screen> {
    let owns_raw_mode = match enable_raw_mode() {
        Ok(()) => true,
        Err(_) if host_made_it_raw() => false,
        Err(err) => return Err(err),
    };
    OWNS_RAW_MODE.store(owns_raw_mode, Ordering::SeqCst);
    PANIC_HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            leave();
            previous(info);
        }));
    });

    let result = (|| {
        execute!(stdout(), EnterAlternateScreen)?;
        ON_ALTERNATE_SCREEN.store(true, Ordering::SeqCst);
        // The Kitty keyboard protocol tells ctrl-i from tab and reports cmd,
        // which the app shell forwards. Asking whether the terminal has it
        // needs raw mode that crossterm set itself. A terminal the host made
        // raw is Quill's own shell, which does have it; and a terminal
        // without it ignores the request.
        let enhanced = !owns_raw_mode || supports_keyboard_enhancement().unwrap_or(false);
        if enhanced {
            execute!(
                stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
            ENHANCED_KEYBOARD.store(true, Ordering::SeqCst);
        }
        // Pasted text arrives as one event, not as keys to act on.
        execute!(stdout(), EnableBracketedPaste)?;
        BRACKETED_PASTE.store(true, Ordering::SeqCst);
        // Clicks, drags and the wheel are reported instead of being the
        // terminal's own to select text and scroll back with.
        if mouse {
            execute!(stdout(), EnableMouseCapture)?;
            MOUSE.store(true, Ordering::SeqCst);
        }
        Terminal::new(Anchored::new(CrosstermBackend::new(stdout())))
    })();
    if result.is_err() {
        leave();
    }
    result
}

/// Puts the terminal back as [`enter`] found it. Safe to call twice.
pub fn leave() {
    if MOUSE.swap(false, Ordering::SeqCst) {
        let _ = execute!(stdout(), DisableMouseCapture);
    }
    if BRACKETED_PASTE.swap(false, Ordering::SeqCst) {
        let _ = execute!(stdout(), DisableBracketedPaste);
    }
    if ENHANCED_KEYBOARD.swap(false, Ordering::SeqCst) {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    if ON_ALTERNATE_SCREEN.swap(false, Ordering::SeqCst) {
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
    if OWNS_RAW_MODE.swap(false, Ordering::SeqCst) {
        let _ = disable_raw_mode();
    }
}

/// Whether the terminal on stdin is already in raw mode.
fn host_made_it_raw() -> bool {
    rustix::termios::tcgetattr(stdin()).is_ok_and(|modes| is_raw(modes.local_modes))
}

/// Raw enough to run a full-screen UI: no line buffering and no echo.
fn is_raw(modes: LocalModes) -> bool {
    !modes.intersects(LocalModes::ICANON | LocalModes::ECHO)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_means_neither_line_buffering_nor_echo() {
        assert!(is_raw(LocalModes::empty()));
        assert!(is_raw(LocalModes::ISIG));
        assert!(!is_raw(LocalModes::ICANON));
        assert!(!is_raw(LocalModes::ECHO));
        assert!(!is_raw(
            LocalModes::ICANON | LocalModes::ECHO | LocalModes::ISIG
        ));
    }

    #[test]
    fn leave_without_enter_does_nothing() {
        leave();
        leave();
        assert!(!OWNS_RAW_MODE.load(Ordering::SeqCst));
    }
}
