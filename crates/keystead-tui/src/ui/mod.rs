//! Rendering of the interactive UI with ratatui.
//!
//! Colours are used sparingly and never as the only signal: selection and
//! key hints use reverse video, which is readable on dark and light
//! terminals alike; borders are ratatui's plain default borders.

mod main_screen;
mod screens;

use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

use crate::app::form::FormFocus;
use crate::app::{App, Focus, Overlay, Screen, StatusKind};
use crate::i18n::{Lang, M};
use crate::input::str_width;
use keystead_core::generator::GeneratorKind;

/// Smallest usable terminal size.
const MIN_WIDTH: u16 = 30;
const MIN_HEIGHT: u16 = 8;

pub(crate) fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

pub(crate) fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

pub(crate) fn reversed() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

pub(crate) fn error_style() -> Style {
    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
}

pub(crate) fn success_style() -> Style {
    Style::new().fg(Color::Green)
}

/// A block with the default border; the title is shown in reverse video
/// when the block has the keyboard focus.
pub(crate) fn panel(title: &str, focused: bool) -> Block<'_> {
    let title_style = if focused {
        reversed().add_modifier(Modifier::BOLD)
    } else {
        bold()
    };
    Block::bordered().title(Span::styled(format!(" {title} "), title_style))
}

/// `area` shrunk to at most `width` × `height`, centred.
pub(crate) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Replaces tabs and other control characters for display.
pub(crate) fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\t' => ' ',
            c if c.is_control() && c != '\n' => '?',
            c => c,
        })
        .collect()
}

/// Word-wraps `s` to `width` columns (long words are split).
pub(crate) fn wrap(s: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for line in sanitize(s).split('\n') {
        let mut cur = String::new();
        let mut cur_w = 0;
        for word in line.split_inclusive(' ') {
            let w = str_width(word);
            if cur_w + w <= width || str_width(word.trim_end()) + cur_w <= width {
                cur.push_str(word);
                cur_w += w;
                continue;
            }
            if !cur.is_empty() {
                out.push(cur.trim_end().to_owned());
                cur.clear();
                cur_w = 0;
            }
            if w <= width {
                cur.push_str(word);
                cur_w = w;
                continue;
            }
            for c in word.chars() {
                let cw = c.width().unwrap_or(0);
                if cur_w + cw > width && !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                cur.push(c);
                cur_w += cw;
            }
        }
        out.push(cur.trim_end().to_owned());
    }
    out
}

/// Renders the whole UI.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(Paragraph::new(app.lang.t(M::TooSmall)).style(bold()), area);
        return;
    }
    // Key hints wrap onto a second line rather than hiding functions.
    let hint_rows = hint_lines(&hints_for(app), area.width, 2);
    let [header, body, status, hints] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(hint_rows.len().max(1) as u16),
    ])
    .areas(area);

    draw_header(frame, header, app);
    match app.screen {
        Screen::VaultPicker { .. } => screens::draw_picker(frame, body, app),
        Screen::CreateVault(_) => screens::draw_create_vault(frame, body, app),
        Screen::Unlock(_) => screens::draw_unlock(frame, body, app),
        Screen::Main => main_screen::draw(frame, body, app),
        Screen::Form(_) => screens::draw_form(frame, body, app),
        Screen::Generator => screens::draw_generator(frame, body, app),
    }
    draw_status(frame, status, app);
    frame.render_widget(Paragraph::new(hint_rows), hints);

    match &app.overlay {
        Some(Overlay::Help) => draw_help(frame, body, app),
        Some(Overlay::ConfirmTrash { name, .. }) => {
            let text = app.lang.tf(M::ConfirmTrash, &[("name", &sanitize(name))]);
            draw_confirm(frame, body, app.lang, &text);
        }
        Some(Overlay::ConfirmDiscard) => {
            draw_confirm(frame, body, app.lang, app.lang.t(M::DiscardChanges));
        }
        None => {}
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![Span::styled(
        " Keystead ",
        reversed().add_modifier(Modifier::BOLD),
    )];
    if let Some(name) = app.vault_name() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(sanitize(name), bold()));
    }
    let left = Line::from(spans);
    frame.render_widget(Paragraph::new(left), area);
    if matches!(app.screen, Screen::Main) && app.vault_name().is_some() {
        let total = app.total_items();
        let text = if app.filter.is_empty() {
            app.lang.item_count(total)
        } else {
            app.lang.tf(
                M::MatchCount,
                &[
                    ("n", &app.items.len().to_string()),
                    ("total", &total.to_string()),
                ],
            )
        };
        let right = Paragraph::new(format!("{text} ")).alignment(ratatui::layout::Alignment::Right);
        frame.render_widget(right, area);
    }
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let Some(status) = &app.status else {
        return;
    };
    let style = match status.kind {
        StatusKind::Info => Style::new(),
        StatusKind::Success => success_style(),
        StatusKind::Error => error_style(),
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(format!(" {}", status.text), style))),
        area,
    );
}

/// One key hint: key label and description.
pub(crate) type Hint = (String, &'static str);

fn h(key: &str, label: &'static str) -> Hint {
    (key.to_owned(), label)
}

/// Key hints for the current state, most important first.
fn hints_for(app: &App) -> Vec<Hint> {
    let l = app.lang;
    let t = |m| l.t(m);
    let ctrl = |c: char| format!("{}+{c}", l.t(M::KeyCtrl));
    if app.is_busy() {
        return Vec::new();
    }
    if let Some(overlay) = &app.overlay {
        return match overlay {
            Overlay::Help => vec![h("Esc", t(M::HintBack))],
            _ => vec![h(t(M::YesKey), t(M::Yes)), h(t(M::NoKey), t(M::No))],
        };
    }
    match &app.screen {
        Screen::VaultPicker { .. } => vec![
            h("Enter", t(M::HintOpenVault)),
            h("↑↓", t(M::HintSelect)),
            h("n", t(M::HintNewVault)),
            h("q", t(M::HintQuit)),
        ],
        Screen::CreateVault(_) => vec![
            h("Tab", t(M::HintNextField)),
            h("Enter", t(M::HintSave)),
            h(
                "Esc",
                t(if app.vaults.is_empty() {
                    M::HintQuit
                } else {
                    M::HintCancel
                }),
            ),
        ],
        Screen::Unlock(_) => vec![
            h("Enter", t(M::HintUnlock)),
            h(
                "Esc",
                t(if app.vaults.len() > 1 {
                    M::HintBack
                } else {
                    M::HintQuit
                }),
            ),
        ],
        Screen::Main => match app.focus {
            Focus::Filter => vec![
                h("Enter", t(M::HintDone)),
                h("Esc", t(M::HintClear)),
                h("↑↓", t(M::HintSelect)),
            ],
            Focus::List | Focus::Detail => {
                let mut v = vec![h("?", t(M::HintHelp))];
                if app.focus == Focus::Detail {
                    v.push(h("Esc", t(M::HintBack)));
                } else {
                    v.push(h("/", t(M::HintSearch)));
                }
                // Only what the selected item actually offers. "s" is
                // hinted inline next to the first masked field instead.
                let actions = app.selected_actions();
                if let Some(a) = actions {
                    for (on, key, m) in [
                        (a.password, "p", M::HintPassword),
                        (a.username, "u", M::HintUsername),
                        (a.totp, "t", M::HintTotp),
                        (a.url, "o", M::HintOpen),
                    ] {
                        if on {
                            v.push(h(key, t(m)));
                        }
                    }
                }
                v.push(h("n", t(M::HintNew)));
                if actions.is_some() {
                    v.push(h("e", t(M::HintEdit)));
                    v.push(h("d", t(M::HintTrash)));
                }
                v.extend([
                    h("g", t(M::HintGenerator)),
                    (ctrl('L'), t(M::HintLock)),
                    h("q", t(M::HintQuit)),
                ]);
                v
            }
        },
        Screen::Form(form) => {
            let mut v = vec![
                (ctrl('S'), t(M::HintSave)),
                h("Esc", t(M::HintCancel)),
                h("Tab", t(M::HintNextField)),
            ];
            if let FormFocus::Field(i) = form.focus_target() {
                if form.fields.get(i).is_some_and(|f| f.input.is_multiline()) {
                    v.push(h("Enter", t(M::HintNewline)));
                }
            }
            if form.has_password() {
                v.push((ctrl('G'), t(M::HintGenerate)));
            }
            if form.fields.iter().any(|f| f.key.is_secret()) {
                v.push((ctrl('R'), t(M::HintReveal)));
            }
            v
        }
        Screen::Generator => {
            let size = match app.generator.options.kind {
                GeneratorKind::Password => M::GenLength,
                GeneratorKind::Passphrase => M::GenWords,
            };
            vec![
                h("c", t(M::HintCopy)),
                h("Enter", t(M::HintRegenerate)),
                h("↑↓", t(M::HintOption)),
                h("←→", t(M::HintChange)),
                (t(M::KeySpace).to_owned(), t(M::HintToggle)),
                h("+/-", t(size)),
                h("Esc", t(M::HintBack)),
            ]
        }
    }
}

/// Renders hints as `[key] label` pairs, wrapped onto at most `max_lines`
/// lines of `width` columns (hints that do not fit are dropped, so the
/// most important ones go first).
fn hint_lines(hints: &[Hint], width: u16, max_lines: usize) -> Vec<Line<'static>> {
    const GAP: usize = 1;
    let width = usize::from(width);
    let mut lines = Vec::new();
    let mut spans = Vec::new();
    let mut used = 0usize;
    for (key, label) in hints {
        let key_text = format!(" {key} ");
        let label_text = format!(" {label}");
        let w = str_width(&key_text) + str_width(&label_text);
        if used > 0 && used + GAP + w > width {
            lines.push(Line::from(std::mem::take(&mut spans)));
            used = 0;
            if lines.len() == max_lines {
                return lines;
            }
        }
        if w > width {
            break;
        }
        if used > 0 {
            spans.push(Span::raw(" ".repeat(GAP)));
            used += GAP;
        }
        used += w;
        spans.push(Span::styled(key_text, reversed()));
        spans.push(Span::raw(label_text));
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    lines
}

fn draw_confirm(frame: &mut Frame, body: Rect, lang: Lang, text: &str) {
    let width = (str_width(text) as u16 + 6).clamp(30, body.width.saturating_sub(4).max(30));
    let lines = wrap(text, usize::from(width.saturating_sub(4)));
    let height = lines.len() as u16 + 4;
    let area = centered(body, width, height);
    frame.render_widget(Clear, area);
    let block = panel(lang.t(M::ConfirmTitle), true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut content: Vec<Line> = lines
        .into_iter()
        .map(|l| Line::from(Span::styled(l, bold())))
        .collect();
    content.push(Line::default());
    content.push(Line::from(vec![
        Span::styled(format!(" {} ", lang.t(M::YesKey)), reversed()),
        Span::raw(format!(" {}    ", lang.t(M::Yes))),
        Span::styled(format!(" {} ", lang.t(M::NoKey)), reversed()),
        Span::raw(format!(" {}", lang.t(M::No))),
    ]));
    frame.render_widget(
        Paragraph::new(content),
        inner.inner(ratatui::layout::Margin::new(1, 0)),
    );
}

fn draw_help(frame: &mut Frame, body: Rect, app: &App) {
    let l = app.lang;
    let ctrl = |c: char| format!("{}+{c}", l.t(M::KeyCtrl));
    let rows: Vec<(String, M)> = vec![
        ("↑↓ PgUp PgDn Home End".into(), M::HelpNavigate),
        ("Enter".into(), M::HelpEnter),
        ("/".into(), M::HelpSearch),
        ("Esc".into(), M::HelpEsc),
        ("u".into(), M::HelpCopyUser),
        ("p".into(), M::HelpCopyPassword),
        ("t".into(), M::HelpCopyTotp),
        ("o".into(), M::HelpOpen),
        ("s".into(), M::HelpShow),
        ("n".into(), M::HelpNew),
        ("e".into(), M::HelpEdit),
        ("d".into(), M::HelpTrash),
        ("g".into(), M::HelpGenerator),
        (ctrl('L'), M::HelpLock),
        ("q".into(), M::HelpQuit),
    ];
    let key_w = rows.iter().map(|(k, _)| str_width(k)).max().unwrap_or(0) + 2;
    let mut lines: Vec<Line> = rows
        .iter()
        .map(|(k, m)| {
            Line::from(vec![
                Span::styled(format!("{k:<key_w$}"), bold()),
                Span::raw(l.t(*m)),
            ])
        })
        .collect();
    let secs = app.settings.clipboard_clear_seconds;
    let clip = if secs > 0 {
        l.tf(M::HelpClipboard, &[("s", &secs.to_string())])
    } else {
        l.t(M::HelpClipboardNever).to_owned()
    };
    let width = 74u16.min(body.width.saturating_sub(2));
    let text_w = usize::from(width.saturating_sub(4));
    let mut notes: Vec<String> = wrap(&clip, text_w);
    let minutes = app.settings.auto_lock_minutes;
    if minutes > 0 {
        notes.extend(wrap(
            &l.tf(M::HelpAutoLock, &[("m", &minutes.to_string())]),
            text_w,
        ));
    }
    // Short terminals: drop the spacing and the "any key" line first.
    let roomy = usize::from(body.height) >= lines.len() + notes.len() + 5;
    if roomy {
        lines.push(Line::default());
    }
    lines.extend(
        notes
            .into_iter()
            .map(|n| Line::from(Span::styled(n, dim()))),
    );
    if roomy {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(l.t(M::HelpClose), dim())));
    }
    let height = lines.len() as u16 + 2;
    let area = centered(body, width, height);
    frame.render_widget(Clear, area);
    let block = panel(l.t(M::HelpTitle), true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(lines),
        inner.inner(ratatui::layout::Margin::new(1, 0)),
    );
}

/// Places the terminal cursor if the position lies inside `area`.
pub(crate) fn set_cursor(frame: &mut Frame, area: Rect, x: u16, y: u16) {
    let pos = Position::new(area.x.saturating_add(x), area.y.saturating_add(y));
    if area.contains(pos) {
        frame.set_cursor_position(pos);
    }
}

#[cfg(test)]
mod tests;
