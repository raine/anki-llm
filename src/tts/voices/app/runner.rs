use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::DefaultTerminal;

use crate::tts::cache::TtsCache;
use crate::tts::voices::catalog::load_snapshot;
use crate::tts::voices::credentials::probe_all;
use crate::tts::voices::preview::spawn_worker;

use super::draw::draw;
use super::state::{App, AppDependencies, InitialFilters};

struct Controller {
    app: App,
}

impl Controller {
    fn initialize(filters: InitialFilters, cache: Arc<TtsCache>) -> Self {
        let deps = AppDependencies {
            entries: load_snapshot(),
            provider_states: probe_all(),
            cache,
            worker: spawn_worker(),
        };
        Self {
            app: App::new(filters, deps),
        }
    }

    fn should_quit(&self) -> bool {
        self.app.should_quit
    }

    fn draw(&mut self, terminal: &mut DefaultTerminal) {
        terminal.draw(|f| draw(f, &mut self.app)).ok();
    }

    fn drain_preview_results(&mut self) {
        while let Ok(result) = self.app.worker.rx.try_recv() {
            self.app.handle_preview_result(result);
        }
    }

    fn reap_player(&mut self) {
        self.app.reap_player();
    }

    fn handle_event(&mut self, evt: Event) {
        match evt {
            Event::Key(key) if is_press_or_repeat(&key) => self.app.handle_key(key),
            Event::Paste(text) => self.app.handle_paste(text),
            _ => {}
        }
    }

    fn tick(&mut self) {
        self.app.tick = self.app.tick.wrapping_add(1);
    }

    fn shutdown(&mut self) {
        self.app.stop_player();
        self.app.worker.shutdown();
    }
}

pub fn run(mut terminal: DefaultTerminal, filters: InitialFilters, cache: Arc<TtsCache>) {
    let mut controller = Controller::initialize(filters, cache);

    while !controller.should_quit() {
        controller.draw(&mut terminal);
        controller.drain_preview_results();
        controller.reap_player();

        if event::poll(Duration::from_millis(50)).unwrap_or(false)
            && let Ok(evt) = event::read()
        {
            controller.handle_event(evt);
        }

        controller.tick();
    }

    controller.shutdown();
}

fn is_press_or_repeat(key: &KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}
