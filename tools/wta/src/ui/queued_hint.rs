//! One-row "Queued (N): preview" indicator rendered directly above the input
//! box whenever the current tab has pending prompts. See `App::drain_pending_prompts`
//! and the Enter / Esc handlers in `app.rs` for the producer side.

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::theme;

/// Height in rows the queued-hint occupies for `tab`. Zero when there's
/// nothing to show — the layout collapses to the existing geometry.
pub(crate) fn queue_hint_height(app: &App) -> u16 {
    if app.current_tab().pending_prompts.is_empty() {
        0
    } else {
        1
    }
}

/// Width budget for the preview text inside the hint row. Mirrors the
/// layout's left/right horizontal padding (1 cell each).
const HORIZONTAL_PADDING: u16 = 2;
/// Maximum chars of the preview displayed; the rest is replaced with `…`.
/// Independent of terminal width so the indicator stays compact even in wide
/// terminals — long prompts don't dominate the row.
const PREVIEW_MAX_CHARS: usize = 60;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let tab = app.current_tab();
    if tab.pending_prompts.is_empty() || area.height == 0 {
        return;
    }
    let count = tab.pending_prompts.len();
    // The next Esc would pop the BACK of the deque (LIFO undo), so the
    // preview shows the most-recently queued prompt to match what Esc
    // affects. FIFO dispatch order (next to send is the front) is conveyed
    // by the count alone — the user sees the queue shrink as the agent
    // works through it.
    let preview_max = PREVIEW_MAX_CHARS.min(
        area.width.saturating_sub(HORIZONTAL_PADDING) as usize,
    );
    let preview = tab
        .pending_prompts
        .back()
        .map(|p| p.preview(preview_max.max(1)))
        .unwrap_or_default();
    let text = t!(
        "input.queue.indicator",
        count = count,
        preview = preview
    )
    .into_owned();
    // Truncate again at the line level just in case the localized template
    // expands beyond the available width (e.g. RTL or longer translations).
    let truncated = truncate_to_width(&text, area.width as usize);
    let line = Line::from(Span::styled(
        format!("  {}", truncated),
        theme::DIM,
    ));
    frame.render_widget(Paragraph::new(line), area);
}

fn truncate_to_width(text: &str, max_cells: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if max_cells == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > max_cells {
            // Replace the last char with an ellipsis if there's room.
            if max_cells >= 1 && !out.is_empty() {
                out.pop();
                out.push('…');
            }
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::truncate_to_width;

    #[test]
    fn truncate_under_width_keeps_string() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
    }

    #[test]
    fn truncate_over_width_inserts_ellipsis() {
        let out = truncate_to_width("abcdefghij", 5);
        // We push 5 chars then the next overflow triggers ellipsis swap.
        assert!(out.ends_with('…'), "got: {out}");
        assert!(out.chars().count() <= 5);
    }

    #[test]
    fn truncate_zero_width_returns_empty() {
        assert_eq!(truncate_to_width("anything", 0), "");
    }
}
