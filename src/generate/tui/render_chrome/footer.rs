use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::super::runner::done_audio_cache_path;
use super::super::state::{App, AppMode};

use crate::tui::theme::{SPINNER_FRAMES, THEME, footer_cmd, footer_pipe};

pub(in crate::generate::tui) fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let mut s: Vec<Span<'static>> = vec![Span::raw(" ")];

    match &app.mode {
        AppMode::Input(_) => {
            s.extend(footer_cmd("Enter", "Generate"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Tab", "Queue term"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Ctrl+O", "Model"));
            s.push(footer_pipe());
            s.extend(footer_cmd("↑↓", "History"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Ctrl+C", "Quit"));
        }
        AppMode::Running => {
            if let Some((current, total)) = app.batch_progress {
                let spinner = SPINNER_FRAMES[app.tick as usize % SPINNER_FRAMES.len()];
                s.push(Span::styled(
                    format!("{spinner} Batch {current}/{total}"),
                    Style::default().fg(THEME.info),
                ));
                s.push(footer_pipe());
            }
            s.extend(footer_cmd("Esc", "Cancel"));
            s.push(footer_pipe());
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
        }
        AppMode::Selecting(state) => {
            let n = state.selected.len();
            let focused_is_dup = state
                .cards
                .get(state.cursor)
                .map(|c| c.is_duplicate)
                .unwrap_or(false);
            s.extend(footer_cmd("Space", "Toggle"));
            s.push(footer_pipe());
            if focused_is_dup {
                s.extend(footer_cmd("f", "Force"));
                s.push(footer_pipe());
            }
            s.extend(footer_cmd("a", "All"));
            s.push(footer_pipe());
            s.extend(footer_cmd("n", "None"));
            s.push(footer_pipe());
            s.extend(footer_cmd("c", "Copy"));
            s.push(footer_pipe());
            s.extend(footer_cmd("d", "Remove"));
            s.push(footer_pipe());
            s.extend(footer_cmd("e", "Edit"));
            s.push(footer_pipe());
            let has_post_select = app
                .session_info
                .as_ref()
                .map(|info| info.post_select_configured)
                .unwrap_or(false);
            if has_post_select {
                s.extend(footer_cmd("z", "Skip post"));
                s.push(footer_pipe());
            }
            if app
                .session_info
                .as_ref()
                .map(|info| info.tts_configured)
                .unwrap_or(false)
                && app.audio_ready()
            {
                s.extend(footer_cmd("p", "Play"));
                s.push(footer_pipe());
            }
            if state.refresh_in_flight || state.regen_in_flight.is_some() {
                let spinner = SPINNER_FRAMES[app.tick as usize % SPINNER_FRAMES.len()];
                let loading_text = if let Some((current, total)) = app.batch_progress {
                    format!("{spinner} Batch {current}/{total}...")
                } else {
                    format!("{spinner} Loading...")
                };
                s.push(Span::styled(loading_text, Style::default().fg(THEME.info)));
            } else if state.term_input.is_some() || state.feedback_input.is_some() {
                s.extend(footer_cmd("Enter", "Submit"));
                s.push(footer_pipe());
                s.extend(footer_cmd("Esc", "Cancel"));
            } else {
                s.extend(footer_cmd("r", "More"));
                s.push(footer_pipe());
                s.extend(footer_cmd("t", "New term"));
                s.push(footer_pipe());
                s.extend(footer_cmd("R", "Regen"));
            }
            s.push(footer_pipe());
            s.extend(footer_cmd("Ctrl+O", "Model"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Enter", "Confirm"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Esc", "Back"));
            s.push(footer_pipe());
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
            s.push(Span::styled(
                format!("  ({n} selected)"),
                Style::default().fg(THEME.dimmed),
            ));
        }
        AppMode::Reviewing(state) => {
            let cur = (state.cursor + 1).min(state.flagged.len());
            let total = state.flagged.len();
            s.push(Span::styled(
                format!("Flagged {cur}/{total}"),
                Style::default().fg(THEME.warning),
            ));
            s.push(footer_pipe());
            s.extend(footer_cmd("k", "Keep"));
            s.push(footer_pipe());
            s.extend(footer_cmd("d", "Discard"));
            s.push(footer_pipe());
            s.extend(footer_cmd("u", "Back"));
            s.push(footer_pipe());
            s.extend(footer_cmd("a", "Keep all"));
            s.push(footer_pipe());
            s.extend(footer_cmd("x", "Discard all"));
            s.push(footer_pipe());
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
        }
        AppMode::Done {
            note_ids, cards, ..
        } => {
            s.extend(footer_cmd("j/k", "Steps"));
            s.push(footer_pipe());
            if !app.is_fatal {
                s.extend(footer_cmd("n", "New term"));
                if app.last_term.is_some() {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("r", "Retry"));
                }
                s.push(footer_pipe());
                s.extend(footer_cmd("Ctrl+O", "Model"));
                if !cards.is_empty() {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("c", "Copy"));
                }
                if app
                    .session_info
                    .as_ref()
                    .map(|info| info.tts_configured)
                    .unwrap_or(false)
                    && app.audio_ready()
                    && cards.first().and_then(done_audio_cache_path).is_some()
                {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("p", "Play"));
                }
                if !note_ids.is_empty() {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("d", "Delete"));
                }
                s.push(footer_pipe());
            }
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
        }
        AppMode::Error(_) => {
            s.extend(footer_cmd("j/k", "Steps"));
            s.push(footer_pipe());
            if !app.is_fatal {
                s.extend(footer_cmd("n", "New term"));
                if app.last_term.is_some() {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("r", "Retry"));
                }
                s.push(footer_pipe());
                s.extend(footer_cmd("Ctrl+O", "Model"));
                s.push(footer_pipe());
            }
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
        }
    }

    frame.render_widget(Paragraph::new(Line::from(s)), area);
}
