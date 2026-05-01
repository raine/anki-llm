use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::DefaultTerminal;

use crate::tts::cache::TtsCache;

use super::draw::draw;
use super::state::{App, InitialFilters};

pub fn run(mut terminal: DefaultTerminal, filters: InitialFilters, cache: Arc<TtsCache>) {
    let mut app = App::new(filters, cache);

    while !app.should_quit {
        terminal.draw(|f| draw(f, &mut app)).ok();

        while let Ok(result) = app.worker.rx.try_recv() {
            app.handle_preview_result(result);
        }
        app.reap_player();

        if event::poll(Duration::from_millis(50)).unwrap_or(false)
            && let Ok(evt) = event::read()
        {
            match evt {
                Event::Key(key) if is_press_or_repeat(&key) => app.handle_key(key),
                Event::Paste(text) => app.handle_paste(text),
                _ => {}
            }
        }

        app.tick = app.tick.wrapping_add(1);
    }

    app.stop_player();
    app.worker.shutdown();
}

fn is_press_or_repeat(key: &KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}
