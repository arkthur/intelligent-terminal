use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::{App, WtEventSeverity};
use crate::theme;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    if !app.show_notification_banner {
        return;
    }

    let Some(notification) = app.active_notification() else {
        return;
    };

    let icon_style = match notification.severity {
        WtEventSeverity::Critical => theme::BADGE_CRITICAL,
        WtEventSeverity::Actionable => theme::BADGE_ACTIONABLE,
        WtEventSeverity::Informational => theme::BADGE_INFO,
    };

    let icon = match notification.severity {
        WtEventSeverity::Critical => "! ",
        WtEventSeverity::Actionable => "* ",
        WtEventSeverity::Informational => "- ",
    };

    let lines = vec![
        Line::from(vec![
            Span::styled(icon, icon_style),
            Span::styled(&notification.summary, icon_style),
        ]),
        Line::from(vec![Span::styled(
            "  Esc: dismiss",
            theme::BANNER_HINT,
        )]),
    ];

    let p = Paragraph::new(lines);
    frame.render_widget(p, area);
}

/// Returns the height needed for the notification banner (0 if hidden).
pub fn banner_height(app: &App) -> u16 {
    if app.show_notification_banner && app.active_notification().is_some() {
        2
    } else {
        0
    }
}
