//! One-row "Queued (N): preview" indicator rendered directly above the input
//! box whenever the current tab has pending prompts. See `App::drain_pending_prompts`
//! and the Enter / Esc handlers in `app.rs` for the producer side.

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::theme;

/// Height in rows the queued-hint occupies for the current tab. Zero when
/// there's nothing to show — the layout collapses to the existing geometry.
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
    //
    // Account for the literal 2-space left padding we prepend on the render
    // line below — otherwise long localized text gets clipped at the right
    // edge for no reason. `area.width` is the row budget; subtract the same
    // padding here so `truncate_to_width` lands on the actual visible width.
    let visible_width = area.width.saturating_sub(HORIZONTAL_PADDING) as usize;
    let preview_max = PREVIEW_MAX_CHARS.min(visible_width);
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
    let truncated = truncate_to_width(&text, visible_width);
    let line = Line::from(Span::styled(
        format!("  {}", truncated),
        theme::DIM,
    ));
    frame.render_widget(Paragraph::new(line), area);
}

fn truncate_to_width(text: &str, max_cells: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if max_cells == 0 {
        return String::new();
    }
    // Fast path: fits as-is. `UnicodeWidthStr::width` counts zero-width
    // chars as 0 cells (combining marks layered onto a base char don't add
    // visible columns), so a string of `a\u{0301}b` reports width 2 here.
    // That matches the actual rendered width, so the fast path is sound —
    // we only need the "zero-width → 1 cell" guard inside the truncation
    // loop to budget room for a per-char break decision once we know we
    // must clip.
    if UnicodeWidthStr::width(text) <= max_cells {
        return text.to_string();
    }
    // Reserve 1 cell up-front for the ellipsis so the displayed width
    // is provably ≤ max_cells without a post-trim that could chew off
    // the marker. When max_cells == 1, content_budget is 0 → we emit a
    // bare `…` (preferred over a blank row).
    let content_budget = max_cells.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        // Treat zero-width chars (combining marks, ZWJ) as 1 cell so they
        // can't slip past the budget; mirrors `ui/input.rs::char_display_width`.
        let w = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
        if used + w > content_budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
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
        // `truncate_to_width` reserves 1 cell for the ellipsis, so at most
        // 4 chars are emitted before `…` is appended.
        assert!(out.ends_with('…'), "got: {out}");
        assert!(out.chars().count() <= 5);
        assert_eq!(unicode_width::UnicodeWidthStr::width(out.as_str()), 5,
            "result must fill exactly the requested width when content overflows");
    }

    #[test]
    fn truncate_zero_width_returns_empty() {
        assert_eq!(truncate_to_width("anything", 0), "");
    }

    #[test]
    fn truncate_wide_char_with_narrow_budget_emits_ellipsis() {
        // CJK full-width glyph is 2 cells; with max_cells=1, we can't fit it
        // but we still want a visible truncation marker rather than a blank
        // row. Regression for the Copilot-review edge case.
        let out = truncate_to_width("中文", 1);
        assert_eq!(out, "…", "got: {out:?}");
    }

    #[test]
    fn truncate_respects_width_with_combining_marks() {
        // Zero-width combining marks can't push the visible width past
        // `max_cells`. Regression for the Copilot round-3 review case:
        // `a\u{0301}b` with `max_cells=1` previously emitted `a…` which
        // displays as 2 cells.
        let out = truncate_to_width("a\u{0301}bcdef", 1);
        let width = unicode_width::UnicodeWidthStr::width(out.as_str());
        assert!(
            width <= 1,
            "truncated string {out:?} has display width {width}, expected ≤ 1"
        );
        assert!(out.contains('…'), "must keep the ellipsis marker; got {out:?}");
    }

    #[test]
    fn truncate_wide_char_with_two_cell_budget_emits_ellipsis_only() {
        // Two cells is enough for either the wide char OR the ellipsis,
        // but not both. We choose the truncation marker over a partial
        // word so the user knows there is hidden content.
        let out = truncate_to_width("中文abc", 2);
        let width = unicode_width::UnicodeWidthStr::width(out.as_str());
        assert!(width <= 2, "got {out:?} width {width}");
        assert!(out.contains('…'));
    }
}
