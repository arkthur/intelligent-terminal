use std::borrow::Cow;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, ChatMessage, PlanEntryStatus};
use crate::theme;
use crate::ui_trace;

const MAX_RENDER_LINE_CHARS: usize = 4096;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let render_started = std::time::Instant::now();
    let inner = Block::default().borders(Borders::NONE);
    let inner_area = inner.inner(area);
    let visible_height = inner_area.height as usize;
    let requested_lines = visible_height
        .saturating_add(app.scroll_offset)
        .saturating_add(32);

    let mut reversed_lines: Vec<Line> = Vec::new();

    if app.agent_streaming {
        reversed_lines.push(Line::from(Span::styled("...", theme::DIM)));
        if !app.pending_agent_response.is_empty() {
            let mut pending: Vec<Line> = app
                .pending_agent_response
                .lines()
                .map(|line_text| {
                    Line::from(Span::styled(
                        truncate_render_text(line_text),
                        theme::AGENT_TEXT,
                    ))
                })
                .collect();
            reversed_lines.extend(pending.drain(..).rev());
        }
    }

    for (idx, msg) in app.messages.iter().enumerate().rev() {
        let is_last_message = idx + 1 == app.messages.len();
        let mut message_lines = build_message_lines(msg, is_last_message, app.agent_streaming);
        reversed_lines.extend(message_lines.drain(..).rev());
        if reversed_lines.len() >= requested_lines {
            break;
        }
    }

    let mut lines: Vec<Line> = reversed_lines.into_iter().rev().collect();

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Type a message and press Enter to begin.",
            theme::DIM,
        )));
    }

    let total_lines = lines.len();
    let scroll = total_lines.saturating_sub(visible_height.saturating_add(app.scroll_offset));

    let paragraph = Paragraph::new(lines).block(inner).scroll((scroll as u16, 0));

    frame.render_widget(paragraph, area);

    ui_trace::log_slow("chat_render", render_started.elapsed(), || {
        format!(
            "messages={} pending_chars={} requested_lines={} visible_height={} area={}x{}",
            app.messages.len(),
            app.pending_agent_response.chars().count(),
            requested_lines,
            visible_height,
            area.width,
            area.height
        )
    });
}

fn build_message_lines<'a>(
    msg: &'a ChatMessage,
    is_last_message: bool,
    agent_streaming: bool,
) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    match msg {
        ChatMessage::User(text) => {
            lines.push(Line::from(vec![
                Span::styled("> ", theme::USER_PROMPT),
                Span::styled(truncate_render_text(text), theme::USER_PROMPT),
            ]));
            lines.push(Line::default());
        }
        ChatMessage::Agent(text) => {
            for line_text in text.lines() {
                lines.push(Line::from(Span::styled(
                    truncate_render_text(line_text),
                    theme::AGENT_TEXT,
                )));
            }
            if !agent_streaming || !is_last_message {
                lines.push(Line::default());
            }
        }
        ChatMessage::System(text) => {
            for line_text in text.lines() {
                lines.push(Line::from(Span::styled(
                    truncate_render_text(line_text),
                    theme::SYSTEM_TEXT,
                )));
            }
            lines.push(Line::default());
        }
        ChatMessage::ToolCall { title, status, .. } => {
            lines.push(Line::from(Span::styled(
                format!(
                    "[{}] {}",
                    truncate_render_text(title),
                    truncate_render_text(status)
                ),
                theme::TOOL_CALL,
            )));
        }
        ChatMessage::Plan(entries) => {
            lines.push(Line::from(Span::styled("Plan:", theme::PLAN_STYLE)));
            for entry in entries {
                let marker = match entry.status {
                    PlanEntryStatus::Completed => "[x]",
                    PlanEntryStatus::InProgress => "[>]",
                    PlanEntryStatus::Pending => "[ ]",
                };
                lines.push(Line::from(Span::styled(
                    format!("  {} {}", marker, truncate_render_text(&entry.content)),
                    theme::PLAN_STYLE,
                )));
            }
            lines.push(Line::default());
        }
        ChatMessage::Error(text) => {
            lines.push(Line::from(Span::styled(
                format!("Error: {}", truncate_render_text(text)),
                theme::ERROR_STYLE,
            )));
            lines.push(Line::default());
        }
    }
    lines
}

fn truncate_render_text(text: &str) -> Cow<'_, str> {
    let char_count = text.chars().count();
    if char_count <= MAX_RENDER_LINE_CHARS {
        return Cow::Borrowed(text);
    }

    let head_chars = MAX_RENDER_LINE_CHARS * 3 / 4;
    let tail_chars = MAX_RENDER_LINE_CHARS / 4;
    let omitted = char_count.saturating_sub(head_chars + tail_chars);
    let head: String = text.chars().take(head_chars).collect();
    let tail: String = text
        .chars()
        .skip(char_count.saturating_sub(tail_chars))
        .collect();

    Cow::Owned(format!("{head} ...<{omitted} chars omitted>... {tail}"))
}
