use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};

use crate::app::App;
use crate::coordinator::{OpenTarget, RecommendedAction};
use crate::theme;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let Some(recommendations) = &app.current_tab().recommendations else {
        return;
    };

    let mut lines: Vec<Line> = Vec::new();
    let single_choice = recommendations.choices.len() == 1;

    for (idx, choice) in recommendations.choices.iter().enumerate() {
        let is_selected = idx == app.current_tab().selected_recommendation;
        let is_recommended = recommendations.recommended_choice == Some(choice.choice);

        // Skip the numbered title row when there is only one choice — the
        // agent's intro text in the chat already conveys what the action is,
        // and the Figma design omits this header for single-choice cards.
        if !single_choice {
            let title_style = if is_selected {
                theme::RECOMMENDATION_TITLE
            } else {
                theme::RECOMMENDATION_DETAIL
            };
            let mut title_spans: Vec<Span> = Vec::new();
            if is_recommended {
                title_spans.push(Span::styled("● ", theme::DOT_AGENT));
            } else {
                title_spans.push(Span::raw("  "));
            }
            title_spans.push(Span::styled(
                format!("{}. {}", choice.choice, choice.title),
                title_style,
            ));
            lines.push(Line::from(title_spans));
        }

        // Determine card content based on action type
        let (command_text, buttons, body_kind) = extract_card_content(choice, app, is_selected);
        let body_style = match body_kind {
            CardBodyKind::Code => theme::CARD_CODE,
            CardBodyKind::Description => theme::CARD_DESCRIPTION,
        };
        let divider_style = if is_selected {
            theme::CARD_BORDER_SELECTED
        } else {
            theme::CARD_BORDER
        };

        // Card width = full available width minus a 2-col indent on each side.
        // No outer border — the card is just a filled rectangle (CARD_BG)
        // with a single horizontal divider between command and buttons.
        let card_width = area.width.saturating_sub(4) as usize;
        // Inner content area = card minus 2 chars of left/right padding inside
        // the fill so text/buttons have breathing room from the card edge.
        let content_width = card_width.saturating_sub(4);

        // Helper to push an empty CARD_BG row that fills the full card width,
        // used for vertical padding so glyphs aren't flush with card edges.
        let push_pad_row = |lines: &mut Vec<Line>| {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(" ".repeat(card_width), theme::CARD_FILL),
            ]));
        };

        // Top padding inside the card.
        push_pad_row(&mut lines);

        // Command/content lines — full card_width painted with CARD_BG.
        for cmd_line in wrap_text(&command_text, content_width) {
            let padded = format!("  {}  ", pad_right(&cmd_line, content_width));
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(padded, body_style),
            ]));
        }

        // Divider line — `─` glyphs across the full card width, painted on
        // CARD_BG so it reads as a hairline inside the card.
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("─".repeat(card_width), divider_style),
        ]));

        // Button row — same card_width fill, buttons right-aligned.
        let button_spans = build_button_spans(
            &buttons,
            is_selected,
            app.current_tab().selected_button,
            card_width,
        );
        lines.push(Line::from(button_spans));

        // Bottom padding inside the card.
        push_pad_row(&mut lines);

        // Spacing between cards
        lines.push(Line::default());
    }

    // Hint line
    lines.push(Line::from(Span::styled(
        "Enter: activate | Esc: dismiss",
        theme::DIM,
    )));

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::NONE).padding(Padding::zero()))
        .wrap(Wrap { trim: false })
        .scroll((app.current_tab().rec_scroll as u16, 0));
    frame.render_widget(paragraph, area);
}

/// Visual style hint for the card body.
///
/// `Code` renders the body as the literal command/input that will be executed
/// (sharp, monospace-feeling). `Description` renders dimmed italic prose for
/// actions that don't have a literal command to show — e.g. `Open`, where the
/// body is metadata about the new destination, not something the user is about
/// to type.
enum CardBodyKind {
    Code,
    Description,
}

/// Extracts the display text, button labels, and body style from a choice's actions.
fn extract_card_content(
    choice: &crate::coordinator::RecommendationChoice,
    _app: &App,
    _is_selected: bool,
) -> (String, Vec<String>, CardBodyKind) {
    // Find the primary action
    for action in &choice.actions {
        match action {
            RecommendedAction::Send { input, .. } => {
                return (
                    input.clone(),
                    vec!["[ Run ]".into(), "Insert in Terminal".into()],
                    CardBodyKind::Code,
                );
            }
            RecommendedAction::OpenAndSend {
                target,
                input,
                agent,
                ..
            } => {
                let agent_label = agent.as_deref().unwrap_or("agent");
                let display = format!("{}: {}", agent_label, input);
                let target_label = match target {
                    OpenTarget::Tab => "Open in New Tab ↵",
                    OpenTarget::Panel => "Open in New Panel ↵",
                };
                return (display, vec![target_label.into()], CardBodyKind::Code);
            }
            RecommendedAction::Open {
                target,
                cwd,
                title,
                direction,
                ..
            } => {
                let kind = match target {
                    OpenTarget::Tab => "tab".to_string(),
                    OpenTarget::Panel => match direction.as_deref() {
                        Some(d) if !d.is_empty() => format!("panel ({})", d),
                        _ => "panel".to_string(),
                    },
                };
                let display = match (title.as_deref(), cwd.as_deref()) {
                    (Some(t), Some(c)) if !t.is_empty() && !c.is_empty() => {
                        format!("New {} ({}) in {}", kind, t, c)
                    }
                    (Some(t), _) if !t.is_empty() => format!("New {} ({})", kind, t),
                    (_, Some(c)) if !c.is_empty() => format!("New {} in {}", kind, c),
                    _ => format!("New empty {}", kind),
                };
                let button = match target {
                    OpenTarget::Tab => "Open Tab ↵",
                    OpenTarget::Panel => "Open Panel ↵",
                };
                return (display, vec![button.into()], CardBodyKind::Description);
            }
        }
    }

    // Fallback: just show the title
    (
        choice.title.clone(),
        vec!["Execute ↵".into()],
        CardBodyKind::Description,
    )
}

/// Builds styled spans for the button row inside a card.
///
/// The whole `card_width` is painted with `CARD_FILL` so the row reads as
/// part of the same filled card. Buttons are right-aligned; each button is
/// padded with one space on either side so that its own bg paints a
/// button-shaped pill within the card fill.
fn build_button_spans<'a>(
    buttons: &[String],
    is_selected: bool,
    focused_button: usize,
    card_width: usize,
) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    spans.push(Span::raw("  "));

    let mut button_pieces: Vec<(String, Style)> = Vec::new();
    for (i, label) in buttons.iter().enumerate() {
        if i > 0 {
            // Wider gap between buttons (~Figma's gap-[24px]) takes the card
            // fill, not the button bg.
            button_pieces.push(("   ".into(), theme::CARD_FILL));
        }
        // Focused button: tight white pill (label rendered as-is, no extra
        // padding — `[ Run ]` already carries its own brackets).
        // Non-focused button: plain white text, no pill — matches Figma's
        // secondary-action look.
        let style = if is_selected && i == focused_button {
            theme::BUTTON_FOCUSED
        } else {
            theme::BUTTON_PLAIN
        };
        button_pieces.push((label.clone(), style));
    }

    let buttons_width: usize = button_pieces.iter().map(|(t, _)| t.chars().count()).sum();
    // Right-align with two cells of right padding before the card edge,
    // matching the 2-cell horizontal inset used by command lines.
    let right_pad = 2usize.min(card_width.saturating_sub(buttons_width));
    let pad_left = card_width.saturating_sub(buttons_width + right_pad);
    spans.push(Span::styled(" ".repeat(pad_left), theme::CARD_FILL));

    for (text, style) in button_pieces {
        spans.push(Span::styled(text, style));
    }

    let used: usize = pad_left + buttons_width;
    if used < card_width {
        spans.push(Span::styled(" ".repeat(card_width - used), theme::CARD_FILL));
    }

    spans
}

/// Simple text wrapping.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    for raw_line in text.lines() {
        if raw_line.is_empty() {
            lines.push(String::new());
            continue;
        }
        let chars: Vec<char> = raw_line.chars().collect();
        for chunk in chars.chunks(width) {
            lines.push(chunk.iter().collect());
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Pads a string with spaces to the right to reach the target width.
fn pad_right(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - len))
    }
}
