use super::effects::Effect;
use super::events::{BackendEvent, StepStatus, TtsUiState, WorkerCommand};
use super::screens::review::ReviewState;
use super::screens::selection::SelectionState;
use super::state::{App, AppMode, Toast, summary_step_idx};

impl App {
    pub(super) fn handle_backend_event(&mut self, event: BackendEvent) -> Vec<Effect> {
        // SessionReady is always relevant
        if let BackendEvent::SessionReady(info) = event {
            let mut effects = Vec::new();
            // Lazy-init the audio player the first time we see a session
            // where TTS preview is live (frontmatter has a `tts:` block
            // AND a playback binary was found at startup).
            if info.tts_configured && self.player.is_none() {
                if let Some(bin) = self.player_binary.clone() {
                    effects.push(Effect::StartAudioPlayer(bin));
                } else {
                    self.logs
                        .push("Audio player not found — preview disabled".into());
                }
            }
            self.session_info = Some(info);
            return effects;
        }

        // Discard events from abandoned runs. Each cancelled run will eventually
        // produce a RunDone or RunError; decrement the counter when that arrives.
        if self.pending_cancels > 0 {
            if matches!(
                event,
                BackendEvent::RunDone { .. } | BackendEvent::RunError(_)
            ) {
                self.pending_cancels -= 1;
            }
            return Vec::new();
        }

        let mut effects = Vec::new();
        match event {
            BackendEvent::SessionReady(_) => unreachable!(),
            BackendEvent::ThinkingReset => {}
            BackendEvent::ThinkingDelta(delta) => {
                if matches!(self.mode, AppMode::Running) {
                    self.append_thinking(&delta);
                }
            }
            BackendEvent::ThinkingClear => {
                self.thinking.clear();
            }
            BackendEvent::Log(msg) => {
                if let Some(idx) = self.current_step_idx {
                    self.steps[idx].logs.push(msg.clone());
                }
                self.logs.push(msg);
                if self.log_auto_scroll {
                    self.log_scroll = self.logs.len().saturating_sub(1) as u16;
                }
            }
            BackendEvent::StepUpdate { step, status } => {
                if matches!(status, StepStatus::Running(_)) {
                    self.current_step_idx = self.step_index(step);
                }
                if let Some(st) = self.step_status_mut(step) {
                    *st = status;
                }
            }
            BackendEvent::RequestSelection(cards) => {
                if let AppMode::Selecting(ref mut state) = self.mode {
                    // Already selecting (model-change refresh): append new cards
                    state.cards.extend(cards);
                    state.refresh_in_flight = false;
                } else if self.batch_queue.is_empty() && self.batch_cards.is_empty() {
                    // Single term or last batch term (first result): go to selection
                    self.batch_progress = None;
                    self.mode = AppMode::Selecting(SelectionState::new(cards));
                } else if !self.batch_queue.is_empty() {
                    // Batch: accumulate cards, stay in Running, advance to next term
                    self.batch_cards.extend(cards);
                    let next_term = self.batch_queue.remove(0);
                    if let Some((ref mut current, _)) = self.batch_progress {
                        *current += 1;
                    }
                    self.last_term = Some(next_term.clone());
                    effects.push(Effect::SendWorker(WorkerCommand::RefreshWithTerm(
                        next_term,
                    )));
                } else {
                    // batch_queue empty but batch_cards non-empty: handle gracefully
                    let mut all_cards = std::mem::take(&mut self.batch_cards);
                    all_cards.extend(cards);
                    self.batch_progress = None;
                    self.mode = AppMode::Selecting(SelectionState::new(all_cards));
                }
            }
            BackendEvent::AppendCards(new_cards) => {
                if !self.batch_cards.is_empty() || !self.batch_queue.is_empty() {
                    // Still in batch processing (Running mode): accumulate
                    self.batch_cards.extend(new_cards);
                    if let Some(next_term) = self.batch_queue.first().cloned() {
                        self.batch_queue.remove(0);
                        if let Some((ref mut current, _)) = self.batch_progress {
                            *current += 1;
                        }
                        self.last_term = Some(next_term.clone());
                        effects.push(Effect::SendWorker(WorkerCommand::RefreshWithTerm(
                            next_term,
                        )));
                    } else {
                        // Last batch term done: enter selection with all cards
                        let all_cards = std::mem::take(&mut self.batch_cards);
                        self.batch_progress = None;
                        self.mode = AppMode::Selecting(SelectionState::new(all_cards));
                    }
                } else if let AppMode::Selecting(ref mut state) = self.mode {
                    // Non-batch refresh (manual 'r' or 't'): append as before
                    state.cards.extend(new_cards);
                    state.refresh_in_flight = false;
                }
            }
            BackendEvent::ReplaceCard {
                previous_card_id,
                card,
            } => {
                if let AppMode::Selecting(ref mut state) = self.mode {
                    // Look up the row by stable id, not index. If the
                    // user removed or edited the card while regen was
                    // in flight, the reply has nothing to attach to —
                    // drop it silently.
                    let Some(slot) = state
                        .cards
                        .iter_mut()
                        .find(|c| c.card_id == previous_card_id)
                    else {
                        if state.regen_in_flight == Some(previous_card_id) {
                            state.regen_in_flight = None;
                        }
                        return effects;
                    };
                    let was_selected = state.selected.remove(&previous_card_id);
                    state.tts_states.remove(&previous_card_id);
                    let new_id = card.card_id;
                    *slot = card;
                    if was_selected {
                        state.selected.insert(new_id);
                    }
                    if state.regen_in_flight == Some(previous_card_id) {
                        state.regen_in_flight = None;
                    }
                    self.toast = Some(Toast {
                        message: "Card regenerated".into(),
                        tick: self.tick,
                    });
                }
            }
            BackendEvent::RegenError { target_id, message } => {
                if let AppMode::Selecting(ref mut state) = self.mode {
                    // Only clear the spinner if THIS target is still
                    // the in-flight one. A late error for an orphaned
                    // (edited / removed) card must not stomp on a
                    // different card's regen-in-flight state.
                    if state.regen_in_flight == Some(target_id) {
                        state.regen_in_flight = None;
                    }
                }
                self.toast = Some(Toast {
                    message,
                    tick: self.tick,
                });
            }
            BackendEvent::TtsState { card_id, state } => {
                if let AppMode::Selecting(ref mut sel) = self.mode {
                    // Drop replies for cards that were removed or
                    // edited (and thus had their `card_id` re-minted)
                    // while synthesis was in flight. Without this
                    // gate, a stale `Ready` reply would auto-play
                    // pre-edit audio once and leak an orphaned entry
                    // into `tts_states`.
                    if !sel.cards.iter().any(|c| c.card_id == card_id) {
                        return effects;
                    }
                    if let TtsUiState::Ready { ref cache_path } = state {
                        effects.push(Effect::PlayAudio {
                            card_id,
                            path: cache_path.clone(),
                        });
                    }
                    sel.tts_states.insert(card_id, state);
                }
            }
            BackendEvent::RequestReview(flagged) => {
                self.mode = AppMode::Reviewing(ReviewState::new(flagged));
            }
            BackendEvent::CostUpdate {
                input_tokens,
                output_tokens,
                cost,
            } => {
                self.run_input_tokens += input_tokens;
                self.run_output_tokens += output_tokens;
                self.run_cost += cost;
            }
            BackendEvent::RunDone {
                message,
                cards,
                note_ids,
                failed,
            } => {
                let summary_idx = summary_step_idx();
                if let Some(record) = self.steps.get_mut(summary_idx) {
                    record.status = if failed {
                        StepStatus::Error(message.clone())
                    } else {
                        StepStatus::Done(None)
                    };
                }
                self.mode = AppMode::Done {
                    message,
                    cards,
                    note_ids,
                    failed,
                };
                self.thinking.clear();
                self.current_step_idx = None;
                self.browse_step = Some(summary_idx);
                self.browse_scroll = 0;
            }
            BackendEvent::RunError(msg) => {
                // During batch: log the error and try to continue with next term
                if !self.batch_queue.is_empty() {
                    let failed_term = self.last_term.as_deref().unwrap_or("?");
                    self.toast = Some(Toast {
                        message: format!("Failed: {failed_term}"),
                        tick: self.tick,
                    });
                    self.logs
                        .push(format!("Error for term \"{failed_term}\": {msg}"));

                    let next_term = self.batch_queue.remove(0);
                    if let Some((ref mut current, _)) = self.batch_progress {
                        *current += 1;
                    }
                    self.last_term = Some(next_term.clone());
                    // Reset step indicators but keep logs for batch continuity
                    for record in &mut self.steps {
                        record.status = StepStatus::Pending;
                        record.logs.clear();
                    }
                    self.current_step_idx = None;
                    self.mode = AppMode::Running;
                    self.thinking.clear();
                    effects.push(Effect::SendWorker(WorkerCommand::Start {
                        term: next_term,
                        enable_thinking_stream: false,
                    }));
                } else {
                    self.batch_progress = None;
                    // If we accumulated cards before this error, show selection
                    if !self.batch_cards.is_empty() {
                        let all_cards = std::mem::take(&mut self.batch_cards);
                        self.thinking.clear();
                        self.mode = AppMode::Selecting(SelectionState::new(all_cards));
                    } else {
                        let summary_idx = summary_step_idx();
                        if let Some(record) = self.steps.get_mut(summary_idx) {
                            record.status = StepStatus::Error(msg.clone());
                        }
                        self.mode = AppMode::Error(msg);
                        self.thinking.clear();
                        self.browse_step = Some(summary_idx);
                        self.browse_scroll = 0;
                    }
                }
            }
            BackendEvent::ModelChangeError(msg) => {
                self.logs.push(format!("Model change failed: {msg}"));
            }
            BackendEvent::Fatal(msg) => {
                // Mark the currently-running step as failed so the spinner
                // is replaced with the ✗ icon.
                for record in &mut self.steps {
                    if matches!(record.status, StepStatus::Running(_)) {
                        record.status = StepStatus::Error(msg.clone());
                        break;
                    }
                }
                let summary_idx = summary_step_idx();
                if let Some(record) = self.steps.get_mut(summary_idx) {
                    record.status = StepStatus::Error(msg.clone());
                }
                self.mode = AppMode::Error(msg);
                self.thinking.clear();
                self.browse_step = Some(summary_idx);
                self.browse_scroll = 0;
                self.is_fatal = true;
            }
        }
        effects
    }

    pub(super) fn handle_player_event(&mut self, ev: crate::audio::PlayerEvent) {
        match ev {
            crate::audio::PlayerEvent::Failed { card_id, message } => {
                // Clear the card's cached `Ready` state so the user's
                // next `p` press routes a fresh `PreviewTts` through the
                // worker instead of replaying the same failing path.
                if let AppMode::Selecting(ref mut sel) = self.mode {
                    sel.tts_states.remove(&card_id);
                }
                self.toast = Some(Toast {
                    message,
                    tick: self.tick,
                });
            }
        }
    }
}
