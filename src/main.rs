mod editor;
mod file_io;
mod input;
mod syntax;
mod ui;

use anyhow::{Context, Result};
use crossterm::event::{
    self, Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use editor::Editor;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::Terminal;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use syntax::Highlighter;

/// Tracks whether we pushed the Kitty keyboard protocol flags, so the
/// panic hook knows whether it needs to pop them too.
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let path = match args.next() {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("Usage: PC <path-to-file>");
            std::process::exit(1);
        }
    };

    let mut editor = Editor::open(path).context("failed to open file")?;
    let highlighter = Highlighter::new();

    install_panic_restore_hook();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    // The base terminal protocol can't distinguish e.g. Ctrl+Backspace
    // from plain Backspace at all — both send the same byte. Terminals
    // that support the Kitty keyboard protocol (Ghostty, Kitty, WezTerm,
    // foot, ...) can report modifiers on such keys if we ask for it.
    // Best-effort: older/unsupported terminals just ignore the query.
    let keyboard_enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if keyboard_enhanced {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        KEYBOARD_ENHANCED.store(true, Ordering::Relaxed);
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, &mut editor, &highlighter);

    if keyboard_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    restore_terminal(&mut terminal)?;

    result
}

/// Redraws only when something actually changed, instead of on a fixed
/// tick — an idle editor should burn ~0% CPU, not repaint several times a
/// second forever. `event::read()` blocks in the OS until an event
/// actually arrives, so there is no polling loop to tune.
fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    editor: &mut Editor,
    highlighter: &Highlighter,
) -> Result<()> {
    terminal.draw(|f| ui::draw(f, editor, highlighter))?;

    loop {
        let mut needs_redraw = false;
        match event::read()? {
            Event::Key(key) => {
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat {
                    match input::handle_key(editor, key) {
                        input::Action::Quit => return Ok(()),
                        input::Action::Continue => {}
                    }
                    needs_redraw = true;
                }
            }
            Event::Resize(_, _) => needs_redraw = true,
            _ => {}
        }

        if needs_redraw {
            terminal.draw(|f| ui::draw(f, editor, highlighter))?;
        }
    }
}

fn restore_terminal<B: Backend + io::Write>(terminal: &mut Terminal<B>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Make sure the terminal isn't left in raw/alternate-screen mode (or with
/// the keyboard protocol still pushed) if we panic while the TUI is
/// active.
fn install_panic_restore_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if KEYBOARD_ENHANCED.load(Ordering::Relaxed) {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));
}
