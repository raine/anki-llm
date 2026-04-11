use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use ratatui::DefaultTerminal;

pub fn init() -> DefaultTerminal {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        original_hook(info);
    }));
    let terminal = ratatui::init();
    execute!(std::io::stdout(), EnableBracketedPaste).ok();
    terminal
}

pub fn restore() {
    execute!(std::io::stdout(), DisableBracketedPaste).ok();
    ratatui::restore();
}
