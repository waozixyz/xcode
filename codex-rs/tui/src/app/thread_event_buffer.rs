//! Bounded replay-buffer policy for per-thread app-server events.

use super::ServerNotification;
use super::ThreadBufferedEvent;
use super::ThreadEventStore;
use codex_app_server_protocol::ThreadItemPayload;
use std::borrow::Cow;

// Keep merged text finite so continued streaming still reaches bounded replay eviction.
const MAX_COALESCED_AGENT_MESSAGE_DELTA_BYTES: usize = 4 * 1024;
const MAX_BUFFERED_AGENT_MESSAGE_DELTA_BYTES: usize = 256 * 1024;
// The live command cell owns the full output. The replay copy only rebuilds a
// transcript after a thread switch, so it keeps the start and end of large output
// instead of doubling every command's output for the life of the session.
const MAX_BUFFERED_COMMAND_OUTPUT_BYTES: usize = 16 * 1024;

/// The start and end of `output` around a count of the lines left out, or `None`
/// when it already fits the replay budget.
fn shorten_command_output(output: &str) -> Option<String> {
    if output.len() <= MAX_BUFFERED_COMMAND_OUTPUT_BYTES {
        return None;
    }
    let half = MAX_BUFFERED_COMMAND_OUTPUT_BYTES / 2;
    let mut head_end = half;
    while !output.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = output.len() - half;
    while !output.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    // Prefer whole lines; one very long line is cut at the byte budget.
    if let Some(newline) = output[..head_end].rfind('\n') {
        head_end = newline + 1;
    }
    if let Some(newline) = output[tail_start..].find('\n') {
        tail_start += newline + 1;
    }
    let omitted = output[head_end..tail_start].lines().count();
    let noun = if omitted == 1 { "line" } else { "lines" };
    let mut shortened = String::with_capacity(head_end + (output.len() - tail_start) + 32);
    shortened.push_str(&output[..head_end]);
    if !shortened.is_empty() && !shortened.ends_with('\n') {
        shortened.push('\n');
    }
    shortened.push_str(&format!("… +{omitted} {noun}\n"));
    shortened.push_str(&output[tail_start..]);
    Some(shortened)
}

fn shorten_buffered_command_output(notification: &mut ServerNotification) {
    if let ServerNotification::ItemCompleted(completed) = notification
        && let ThreadItemPayload::CommandExecution {
            aggregated_output: Some(output),
            ..
        } = &mut completed.item.payload
        && let Some(shortened) = shorten_command_output(output)
    {
        *output = shortened;
    }
}

impl ThreadEventStore {
    pub(super) fn push_replay_notification(&mut self, notification: Cow<'_, ServerNotification>) {
        if let ServerNotification::AgentMessageDelta(delta) = notification.as_ref()
            && delta.delta.len() > MAX_BUFFERED_AGENT_MESSAGE_DELTA_BYTES
        {
            return;
        }

        if let ServerNotification::AgentMessageDelta(delta) = notification.as_ref()
            && self.attachment_prefix != Some(self.buffer.len())
            && let Some(ThreadBufferedEvent::Notification(previous)) = self.buffer.back_mut()
            && let ServerNotification::AgentMessageDelta(previous) = previous.as_mut()
            && previous.thread_id == delta.thread_id
            && previous.turn_id == delta.turn_id
            && previous.item_id == delta.item_id
            && previous.delta.len().saturating_add(delta.delta.len())
                <= MAX_COALESCED_AGENT_MESSAGE_DELTA_BYTES
        {
            previous.delta.push_str(&delta.delta);
            self.buffered_agent_message_delta_bytes = self
                .buffered_agent_message_delta_bytes
                .saturating_add(delta.delta.len());
            self.evict_overflowing_events();
            return;
        }

        let mut notification = notification.into_owned();
        shorten_buffered_command_output(&mut notification);
        self.push_buffered_event(ThreadBufferedEvent::Notification(Box::new(notification)));
    }

    pub(super) fn push_buffered_event(&mut self, event: ThreadBufferedEvent) {
        if let ThreadBufferedEvent::Notification(notification) = &event
            && let ServerNotification::AgentMessageDelta(delta) = notification.as_ref()
        {
            self.buffered_agent_message_delta_bytes = self
                .buffered_agent_message_delta_bytes
                .saturating_add(delta.delta.len());
        }
        self.buffer.push_back(event);
        self.evict_overflowing_events();
    }

    fn evict_overflowing_events(&mut self) {
        while self.buffer.len() > self.capacity
            || self.buffered_agent_message_delta_bytes > MAX_BUFFERED_AGENT_MESSAGE_DELTA_BYTES
        {
            let Some(removed) = self.buffer.pop_front() else {
                break;
            };
            if let Some(prefix) = &mut self.attachment_prefix {
                *prefix = prefix.saturating_sub(1);
            }
            match removed {
                ThreadBufferedEvent::Notification(notification) => {
                    if let ServerNotification::AgentMessageDelta(delta) = notification.as_ref() {
                        self.buffered_agent_message_delta_bytes = self
                            .buffered_agent_message_delta_bytes
                            .saturating_sub(delta.delta.len());
                    } else if let ServerNotification::TurnStarted(started) = *notification
                        && self.active_turn_id() == Some(&started.turn.id)
                    {
                        self.evicted_active_turn = Some(Box::new(started.turn));
                    }
                }
                ThreadBufferedEvent::Request(request) => self
                    .pending_interactive_replay
                    .note_evicted_server_request(request.as_ref()),
                ThreadBufferedEvent::HistoryEntryResponse(_)
                | ThreadBufferedEvent::FeedbackSubmission(_) => {}
            }
        }
    }
}

#[cfg(test)]
#[path = "thread_event_buffer_tests.rs"]
mod tests;
