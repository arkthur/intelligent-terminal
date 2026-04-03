use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::{App, ConnectionState, WtEventSeverity};
use crate::theme;

fn agent_identity(app: &App) -> String {
    let agent_name = if app.agent_name.is_empty() {
        "agent"
    } else {
        &app.agent_name
    };

    let agent_identity = match app.agent_model.as_deref() {
        Some(model) if !model.is_empty() => format!("{} {}", agent_name, model),
        _ => agent_name.to_string(),
    };

    match app.prompt_name.as_deref() {
        Some(prompt_name) if !prompt_name.is_empty() => {
            format!("{} · {}", agent_identity, prompt_name)
        }
        _ => agent_identity,
    }
}

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let identity = agent_identity(app);
    let identity_style = match &app.state {
        ConnectionState::Failed(_) => theme::STATUS_FAILED,
        _ if app.progress_status.is_some() || app.prompt_in_flight => theme::IN_PROGRESS,
        ConnectionState::Connected => theme::STATUS_CONNECTED,
        ConnectionState::Connecting(_) => theme::STATUS_CONNECTING,
        ConnectionState::Disconnected => theme::STATUS_DISCONNECTED,
    };

    let mut spans = vec![Span::styled(identity, identity_style)];
    if let Some(note) = app.timing_note.as_deref().filter(|note| !note.is_empty()) {
        spans.push(Span::styled(" | ", theme::DIM));
        spans.push(Span::styled(note.to_string(), theme::SYSTEM_TEXT));
    }

    // WT event notification badge (right-aligned)
    if let Some((summary, severity)) = app.notification_badge() {
        let badge_style = match severity {
            WtEventSeverity::Critical => theme::BADGE_CRITICAL,
            WtEventSeverity::Actionable => theme::BADGE_ACTIONABLE,
            WtEventSeverity::Informational => theme::BADGE_INFO,
        };
        let icon = match severity {
            WtEventSeverity::Critical => "! ",
            WtEventSeverity::Actionable => "* ",
            WtEventSeverity::Informational => "",
        };
        let count = app.unacknowledged_count();
        let badge_text = if count > 1 {
            format!("{}{} (+{})", icon, summary, count - 1)
        } else {
            format!("{}{}", icon, summary)
        };

        // Calculate padding to right-align the badge
        let left_len: usize = spans.iter().map(|s| s.width()).sum();
        let badge_len = badge_text.len();
        let total_width = area.width as usize;
        if left_len + badge_len + 2 < total_width {
            let pad = total_width - left_len - badge_len;
            spans.push(Span::raw(" ".repeat(pad)));
        } else {
            spans.push(Span::styled(" | ", theme::DIM));
        }
        spans.push(Span::styled(badge_text, badge_style));
    }

    let p = Paragraph::new(Line::from(spans));
    frame.render_widget(p, area);
}
