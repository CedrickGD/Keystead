//! Vault picker, vault creation, unlock prompt, item editor and generator.

use keystead_core::generator::GeneratorKind;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use super::{bold, centered, dim, error_style, panel, reversed, sanitize, wrap};
use crate::app::form::FormFocus;
use crate::app::generator::GenRow;
use crate::app::{App, Screen};
use crate::i18n::{Lang, M};
use crate::input::{str_width, truncate, TextInput};

/// Width of the centred dialogs (vault picker, unlock, create vault).
const DIALOG_WIDTH: u16 = 64;

fn strength_style(score: u8) -> Style {
    match score {
        0 | 1 => Style::new().fg(Color::Red),
        2 => Style::new(),
        _ => Style::new().fg(Color::Green),
    }
}

/// A bordered single-line input; returns the cursor position (absolute).
fn draw_input(
    frame: &mut Frame,
    area: Rect,
    input: &TextInput,
    masked: bool,
    focused: bool,
) -> Option<(u16, u16)> {
    let border_style = if focused { bold() } else { dim() };
    let block = Block::bordered().border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let view = input.view(inner.width, 1, masked);
    let text = view.lines.first().cloned().unwrap_or_default();
    frame.render_widget(Paragraph::new(sanitize(&text)), inner);
    focused.then(|| (inner.x + view.cursor_x, inner.y))
}

pub fn draw_picker(frame: &mut Frame, body: Rect, app: &App) {
    let Screen::VaultPicker { selected } = &app.screen else {
        return;
    };
    let lang = app.lang;
    let height = (app.vaults.len() as u16).saturating_add(4).min(body.height);
    let area = centered(body, DIALOG_WIDTH, height.max(6));
    let block = panel(lang.t(M::PickVaultTitle), true);
    let inner = block.inner(area).inner(Margin::new(1, 1));
    frame.render_widget(block, area);
    let last = app.settings.last_vault_id.as_deref();
    let items: Vec<ListItem> = app
        .vaults
        .iter()
        .map(|v| {
            let mut spans = vec![Span::styled(sanitize(&v.name), bold())];
            if Some(v.id.as_str()) == last {
                spans.push(Span::styled(format!("  ({})", lang.t(M::LastUsed)), dim()));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(reversed())
        .highlight_symbol("> ");
    let mut state = ListState::default().with_selected(Some(*selected));
    frame.render_stateful_widget(list, inner, &mut state);
}

pub fn draw_unlock(frame: &mut Frame, body: Rect, app: &App) {
    let Screen::Unlock(u) = &app.screen else {
        return;
    };
    let lang = app.lang;
    let area = centered(body, DIALOG_WIDTH, 13);
    let block = panel("Keystead", true);
    let inner = block.inner(area).inner(Margin::new(2, 1));
    frame.render_widget(block, area);
    let [title, vault, _, label, input, message] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(Span::styled(lang.t(M::UnlockTitle), bold())),
        title,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{}: ", lang.t(M::VaultLabel)), dim()),
            Span::styled(sanitize(&u.vault.name), bold()),
        ])),
        vault,
    );
    frame.render_widget(Paragraph::new(lang.t(M::MasterPassword)), label);
    let busy = app.is_busy();
    if let Some((x, y)) = draw_input(frame, input, &u.password, true, !busy) {
        frame.set_cursor_position((x, y));
    }
    let msg = if busy {
        Some((lang.t(M::Unlocking).to_owned(), bold()))
    } else {
        u.error.as_ref().map(|e| (e.clone(), error_style()))
    };
    if let Some((text, style)) = msg {
        let lines: Vec<Line> = wrap(&text, usize::from(message.width))
            .into_iter()
            .map(|l| Line::from(Span::styled(l, style)))
            .collect();
        frame.render_widget(Paragraph::new(lines), message);
    }
}

pub fn draw_create_vault(frame: &mut Frame, body: Rect, app: &App) {
    let Screen::CreateVault(f) = &app.screen else {
        return;
    };
    let lang = app.lang;
    // Text width inside the border (2) and the margin (4).
    let text_w = usize::from(DIALOG_WIDTH.min(body.width).saturating_sub(6));
    let intro = if f.first_run {
        lang.t(M::NoVaultYet)
    } else {
        ""
    };
    let intro_lines = wrap(intro, text_w);
    let note_lines = wrap(lang.t(M::RememberMaster), text_w);
    let intro_h = if f.first_run {
        intro_lines.len() as u16 + 1
    } else {
        0
    };
    // Border (2), labels/inputs/strength/message (14), the note, plus a
    // blank margin row above and below if there is room.
    let content_h = 2 + intro_h + 14 + note_lines.len() as u16;
    let v_margin = u16::from(body.height >= content_h + 2);
    let area = centered(body, DIALOG_WIDTH, content_h + 2 * v_margin);
    let block = panel(lang.t(M::CreateVaultTitle), true);
    let inner = block.inner(area).inner(Margin::new(2, v_margin));
    frame.render_widget(block, area);
    let [intro_area, name_label, name_in, pw_label, pw_in, strength, confirm_label, confirm_in, message, note] =
        Layout::vertical([
            Constraint::Length(intro_h),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(inner);
    if f.first_run {
        let lines: Vec<Line> = intro_lines
            .into_iter()
            .map(|l| Line::from(Span::styled(l, bold())))
            .collect();
        frame.render_widget(Paragraph::new(lines), intro_area);
    }
    let busy = app.is_busy();
    frame.render_widget(Paragraph::new(lang.t(M::VaultName)), name_label);
    frame.render_widget(Paragraph::new(lang.t(M::MasterPassword)), pw_label);
    frame.render_widget(Paragraph::new(lang.t(M::ConfirmMaster)), confirm_label);
    let mut cursor = None;
    for (i, (area, input, masked)) in [
        (name_in, &f.name, false),
        (pw_in, &f.password, true),
        (confirm_in, &f.confirm, true),
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(c) = draw_input(frame, area, input, masked, !busy && f.focus == i) {
            cursor = Some(c);
        }
    }
    if let Some(score) = f.score {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{}: ", lang.t(M::StrengthLabel)), dim()),
                Span::styled(
                    lang.strength(score),
                    strength_style(score).add_modifier(Modifier::BOLD),
                ),
            ])),
            strength,
        );
    }
    let msg = if busy {
        Some(Span::styled(lang.t(M::Creating).to_owned(), bold()))
    } else {
        f.error
            .as_ref()
            .map(|e| Span::styled(e.clone(), error_style()))
    };
    if let Some(span) = msg {
        frame.render_widget(Paragraph::new(Line::from(span)), message);
    }
    let lines: Vec<Line> = note_lines
        .into_iter()
        .map(|l| Line::from(Span::styled(l, dim())))
        .collect();
    frame.render_widget(Paragraph::new(lines), note);
    if let Some(pos) = cursor {
        frame.set_cursor_position(pos);
    }
}

/// Rows the notes field occupies in the item editor.
const NOTES_ROWS: u16 = 5;

pub fn draw_form(frame: &mut Frame, body: Rect, app: &mut App) {
    let lang = app.lang;
    let Screen::Form(form) = &mut app.screen else {
        return;
    };
    let title = if form.is_new {
        lang.t(M::NewLoginTitle).to_owned()
    } else {
        lang.tf(
            M::EditTitle,
            &[("name", &truncate(&sanitize(form.original_name()), 40))],
        )
    };
    let block = panel(&title, true);
    let inner = block.inner(body).inner(Margin::new(1, 0));
    frame.render_widget(block, body);
    if inner.width < 10 || inner.height < 3 {
        return;
    }
    let [content, error_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);

    let label_w = form
        .fields
        .iter()
        .map(|f| str_width(lang.t(f.key.label())))
        .max()
        .unwrap_or(10) as u16
        + 4;
    let value_w = content.width.saturating_sub(label_w).max(1);

    // Row offsets of every field and of the button row.
    let mut tops = Vec::with_capacity(form.fields.len() + 1);
    let mut y = 0u16;
    for f in &form.fields {
        tops.push(y);
        y += if f.input.is_multiline() {
            NOTES_ROWS
        } else {
            1
        } + 1;
    }
    tops.push(y);
    let total = y + 1;

    // Keep the focused element visible.
    let focus_top = tops
        .get(form.focus.min(form.fields.len()))
        .copied()
        .unwrap_or(0);
    let focus_h = match form.fields.get(form.focus) {
        Some(f) if f.input.is_multiline() => NOTES_ROWS,
        _ => 1,
    };
    if focus_top < form.scroll {
        form.scroll = focus_top;
    } else if focus_top + focus_h > form.scroll + content.height {
        form.scroll = (focus_top + focus_h).saturating_sub(content.height);
    }
    form.scroll = form.scroll.min(total.saturating_sub(content.height));
    let scroll = form.scroll;

    let mut cursor = None;
    for (i, field) in form.fields.iter().enumerate() {
        let height = if field.input.is_multiline() {
            NOTES_ROWS
        } else {
            1
        };
        let Some(top) = tops[i].checked_sub(scroll) else {
            continue;
        };
        if top >= content.height {
            continue;
        }
        let height = height.min(content.height - top);
        let focused = form.focus == i;
        let label_area = Rect {
            x: content.x,
            y: content.y + top,
            width: label_w,
            height: 1,
        };
        let marker = if focused { "> " } else { "  " };
        let label_style = if focused {
            reversed().add_modifier(Modifier::BOLD)
        } else {
            bold()
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(marker),
                Span::styled(lang.t(field.key.label()), label_style),
            ])),
            label_area,
        );
        let value_area = Rect {
            x: content.x + label_w,
            y: content.y + top,
            width: value_w,
            height,
        };
        let masked = field.key.is_secret() && !form.reveal;
        let view = field.input.view(value_w, height, masked);
        let style = if focused {
            Style::new().add_modifier(Modifier::UNDERLINED)
        } else {
            Style::new()
        };
        let mut lines: Vec<Line> = view
            .lines
            .iter()
            .map(|l| Line::from(Span::styled(sanitize(l), style)))
            .collect();
        if field.input.is_empty() && !focused && field.key == crate::app::form::FieldKey::Totp {
            lines = vec![Line::from(Span::styled(lang.t(M::TotpHint), dim()))];
        }
        if focused && field.input.is_multiline() {
            // Mark the whole notes area so empty lines are visible too.
            frame.render_widget(
                Block::new().style(Style::new().add_modifier(Modifier::UNDERLINED)),
                value_area,
            );
        }
        frame.render_widget(Paragraph::new(lines), value_area);
        if focused {
            cursor = Some((value_area.x + view.cursor_x, value_area.y + view.cursor_y));
        }
    }

    // Buttons.
    if let Some(top) = tops[form.fields.len()].checked_sub(scroll) {
        if top < content.height {
            let focus = form.focus_target();
            let button = |label: &str, on: bool| {
                Span::styled(
                    format!("[ {label} ]"),
                    if on {
                        reversed().add_modifier(Modifier::BOLD)
                    } else {
                        bold()
                    },
                )
            };
            let line = Line::from(vec![
                Span::raw(" ".repeat(usize::from(label_w))),
                button(lang.t(M::Save), focus == FormFocus::Save),
                Span::raw("  "),
                button(lang.t(M::Cancel), focus == FormFocus::Cancel),
            ]);
            frame.render_widget(
                Paragraph::new(line),
                Rect {
                    y: content.y + top,
                    height: 1,
                    ..content
                },
            );
        }
    }
    if let Some(err) = &form.error {
        frame.render_widget(
            Paragraph::new(Span::styled(err.clone(), error_style())),
            error_area,
        );
    }
    if let Some((x, y)) = cursor {
        if x < content.x + content.width && y < content.y + content.height {
            frame.set_cursor_position((x, y));
        }
    }
}

fn gen_row_text(lang: Lang, app: &App, row: GenRow) -> (String, String) {
    let o = &app.generator.options;
    let check = |b: bool| {
        if b {
            "[x]".to_owned()
        } else {
            "[ ]".to_owned()
        }
    };
    match row {
        GenRow::Mode => (
            lang.t(M::GenMode).to_owned(),
            match o.kind {
                GeneratorKind::Password => format!("< {} >", lang.t(M::GenModePassword)),
                GeneratorKind::Passphrase => format!("< {} >", lang.t(M::GenModePassphrase)),
            },
        ),
        GenRow::Length => (lang.t(M::GenLength).to_owned(), format!("< {} >", o.length)),
        GenRow::Uppercase => (lang.t(M::GenUppercase).to_owned(), check(o.uppercase)),
        GenRow::Lowercase => (lang.t(M::GenLowercase).to_owned(), check(o.lowercase)),
        GenRow::Digits => (lang.t(M::GenDigits).to_owned(), check(o.digits)),
        GenRow::Symbols => (lang.t(M::GenSymbols).to_owned(), check(o.symbols)),
        GenRow::AvoidAmbiguous => (
            lang.t(M::GenAvoidAmbiguous).to_owned(),
            check(o.avoid_ambiguous),
        ),
        GenRow::Words => (lang.t(M::GenWords).to_owned(), format!("< {} >", o.words)),
        GenRow::Separator => {
            let sep = if o.separator == " " {
                lang.t(M::GenSeparatorSpace).to_owned()
            } else {
                format!("\"{}\"", o.separator)
            };
            (lang.t(M::GenSeparator).to_owned(), format!("< {sep} >"))
        }
        GenRow::Capitalize => (lang.t(M::GenCapitalize).to_owned(), check(o.capitalize)),
        GenRow::IncludeNumber => (
            lang.t(M::GenIncludeNumber).to_owned(),
            check(o.include_number),
        ),
    }
}

pub fn draw_generator(frame: &mut Frame, body: Rect, app: &App) {
    let lang = app.lang;
    let width = 76.min(body.width);
    let g = &app.generator;
    let mut lines: Vec<Line> = Vec::new();
    match &g.error {
        Some(e) => lines.push(Line::from(Span::styled(lang.error(e), error_style()))),
        None => {
            for l in wrap(&g.value, usize::from(width.saturating_sub(6))) {
                lines.push(Line::from(Span::styled(l, bold())));
            }
        }
    }
    if let Some(score) = g.score {
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", lang.t(M::StrengthLabel)), dim()),
            Span::styled(
                lang.strength(score),
                strength_style(score).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    lines.push(Line::default());
    let rows = g.rows();
    let label_w = rows
        .iter()
        .map(|r| str_width(&gen_row_text(lang, app, *r).0))
        .max()
        .unwrap_or(10)
        + 3;
    for (i, row) in rows.iter().enumerate() {
        let (label, value) = gen_row_text(lang, app, *row);
        let selected = i == g.row;
        let marker = if selected { "> " } else { "  " };
        let pad = label_w.saturating_sub(str_width(&label));
        let style = if selected { reversed() } else { Style::new() };
        lines.push(Line::from(vec![
            Span::raw(marker),
            Span::styled(
                format!("{label}{}", " ".repeat(pad)),
                style.add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {value} "), style),
        ]));
    }
    // Sized to the content: border + margin (4) plus the lines.
    let height = (lines.len() as u16).saturating_add(4);
    let area = centered(body, width, height);
    let block = panel(lang.t(M::GeneratorTitle), true);
    let inner = block.inner(area).inner(Margin::new(2, 1));
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), inner);
}
