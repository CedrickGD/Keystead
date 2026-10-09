//! Main screen: item list with search (left) and details (right).

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};
use ratatui::Frame;
use keystead_core::model::{now_ms, FieldKind, ItemType, VaultItem};
use keystead_core::totp;

use super::{bold, dim, error_style, panel, reversed, sanitize, set_cursor, wrap};
use crate::app::{App, Focus};
use crate::i18n::{Lang, M};
use crate::input::{str_width, truncate, MASK_CHAR};

/// Below this width only one pane is shown (list, or details after Enter).
const SPLIT_MIN_WIDTH: u16 = 72;

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width >= SPLIT_MIN_WIDTH {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)])
                .areas(area);
        draw_list(frame, left, app);
        draw_detail(frame, right, app);
    } else if app.focus == Focus::Detail {
        draw_detail(frame, area, app);
    } else {
        draw_list(frame, area, app);
    }
}

pub(crate) fn type_letter(lang: Lang, t: ItemType) -> &'static str {
    lang.t(match t {
        ItemType::Login => M::LetterLogin,
        ItemType::Card => M::LetterCard,
        ItemType::Identity => M::LetterIdentity,
        ItemType::Note => M::LetterNote,
    })
}

pub(crate) fn type_name(lang: Lang, t: ItemType) -> &'static str {
    lang.t(match t {
        ItemType::Login => M::TypeLogin,
        ItemType::Card => M::TypeCard,
        ItemType::Identity => M::TypeIdentity,
        ItemType::Note => M::TypeNote,
    })
}

fn draw_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let lang = app.lang;
    let focused = matches!(app.focus, Focus::List | Focus::Filter);
    let block = panel(lang.t(M::Items), focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let [search, list] = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);

    // Search line ("Suche: …" + separator).
    let label = format!(" {}: ", lang.t(M::Search));
    let label_w = str_width(&label) as u16;
    let search_line = Rect {
        height: 1,
        ..search
    };
    frame.render_widget(
        Paragraph::new(Span::styled(label.clone(), bold())),
        search_line,
    );
    let field = Rect {
        x: search_line.x + label_w,
        width: search_line.width.saturating_sub(label_w + 1),
        ..search_line
    };
    if app.filter.is_empty() && app.focus != Focus::Filter {
        frame.render_widget(
            Paragraph::new(Span::styled(
                truncate(lang.t(M::SearchPlaceholder), usize::from(field.width)),
                dim(),
            )),
            field,
        );
    } else {
        let view = app.filter.view(field.width.max(1), 1, false);
        let text = view.lines.first().cloned().unwrap_or_default();
        let style = if app.focus == Focus::Filter {
            Style::new().add_modifier(ratatui::style::Modifier::UNDERLINED)
        } else {
            Style::new()
        };
        frame.render_widget(Paragraph::new(Span::styled(sanitize(&text), style)), field);
        if app.focus == Focus::Filter {
            set_cursor(frame, field, view.cursor_x, 0);
        }
    }
    if search.height > 1 {
        let sep = Rect {
            y: search.y + 1,
            height: 1,
            ..search
        };
        frame.render_widget(
            Paragraph::new(Span::styled("─".repeat(usize::from(sep.width)), dim())),
            sep,
        );
    }

    app.page_size = usize::from(list.height.max(1));
    if app.items.is_empty() {
        let msg = if app.filter.is_empty() {
            M::NoItems
        } else {
            M::NoMatches
        };
        let lines: Vec<Line> = wrap(lang.t(msg), usize::from(list.width.saturating_sub(2)))
            .into_iter()
            .map(|l| Line::from(Span::styled(format!(" {l}"), dim())))
            .collect();
        frame.render_widget(Paragraph::new(lines), list);
        return;
    }

    // Columns: type letter, name, username/subtitle.
    let avail = usize::from(list.width.saturating_sub(2 + 1 + 2));
    let name_w = (avail * 11 / 20).max(4);
    let sub_w = avail.saturating_sub(name_w + 1);
    let rows: Vec<Row> = app
        .items
        .iter()
        .map(|s| {
            let mut name = sanitize(&s.name);
            if s.favorite {
                name.push_str(" *");
            }
            Row::new(vec![
                Cell::from(Span::styled(type_letter(lang, s.item_type), bold())),
                Cell::from(Span::styled(truncate(&name, name_w), bold())),
                Cell::from(Span::styled(truncate(&sanitize(&s.subtitle), sub_w), dim())),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(1),
        Constraint::Length(name_w as u16),
        Constraint::Fill(1),
    ];
    let highlight = if focused { reversed() } else { bold() };
    let table = Table::new(rows, widths)
        .column_spacing(1)
        .row_highlight_style(highlight)
        .highlight_symbol("> ");
    frame.render_stateful_widget(table, list, &mut app.list_state);
}

/// Label/value rows of the detail panel.
struct DetailBuilder {
    lang: Lang,
    show: bool,
    label_w: usize,
    width: usize,
    lines: Vec<Line<'static>>,
    hinted: bool,
}

impl DetailBuilder {
    fn blank(&mut self) {
        if self.lines.last().is_some_and(|l| l.width() > 0) {
            self.lines.push(Line::default());
        }
    }

    fn heading(&mut self, m: M) {
        self.blank();
        self.lines
            .push(Line::from(Span::styled(self.lang.t(m).to_owned(), bold())));
    }

    /// A row; long values wrap and continue under the value column.
    fn row(&mut self, label: &str, value: &str, style: Style) {
        if value.trim().is_empty() {
            return;
        }
        let value_w = self.width.saturating_sub(self.label_w).max(8);
        for (i, part) in wrap(value, value_w).into_iter().enumerate() {
            let label_text = if i == 0 {
                truncate(label, self.label_w.saturating_sub(1))
            } else {
                String::new()
            };
            let pad = self.label_w.saturating_sub(str_width(&label_text));
            self.lines.push(Line::from(vec![
                Span::styled(format!("{label_text}{}", " ".repeat(pad)), dim()),
                Span::styled(part, style),
            ]));
        }
    }

    fn secret(&mut self, label: &str, value: &str, masked: String) {
        if value.trim().is_empty() {
            return;
        }
        if self.show {
            self.row(label, value, bold());
        } else {
            self.row(label, &masked, Style::new());
            if !self.hinted {
                self.hinted = true;
                let hint = format!("   ({})", self.lang.t(M::ShowSecretsHint));
                if let Some(last) = self.lines.last_mut() {
                    if last.width() + str_width(&hint) <= self.width {
                        last.spans.push(Span::styled(hint, dim()));
                    }
                }
            }
        }
    }

    fn text_block(&mut self, text: &str) {
        for l in wrap(text.trim_end(), self.width.saturating_sub(2)) {
            self.lines.push(Line::from(format!("  {l}")));
        }
    }
}

fn mask() -> String {
    std::iter::repeat_n(MASK_CHAR, 8).collect()
}

fn group_code(code: &str) -> String {
    if code.len() >= 6 && code.is_ascii() {
        let mid = code.len() / 2;
        format!("{} {}", &code[..mid], &code[mid..])
    } else {
        code.to_owned()
    }
}

fn detail_lines(app: &App, item: &VaultItem, width: usize) -> Vec<Line<'static>> {
    let lang = app.lang;
    let labels: &[M] = match item.item_type {
        ItemType::Login => &[
            M::FieldUsername,
            M::FieldPassword,
            M::FieldTotp,
            M::FieldWebsite,
            M::FieldUpdated,
        ],
        ItemType::Card => &[
            M::FieldCardholder,
            M::FieldBrand,
            M::FieldCardNumber,
            M::FieldExpiry,
            M::FieldSecurityCode,
            M::FieldUpdated,
        ],
        ItemType::Identity => &[
            M::FieldName,
            M::FieldEmail,
            M::FieldPhone,
            M::FieldCompany,
            M::FieldAddress,
            M::FieldUsername,
            M::FieldUpdated,
        ],
        ItemType::Note => &[M::FieldUpdated],
    };
    let custom_w = item
        .fields
        .iter()
        .map(|f| str_width(&f.name))
        .max()
        .unwrap_or(0);
    let label_w = labels
        .iter()
        .map(|m| str_width(lang.t(*m)))
        .chain(std::iter::once(custom_w))
        .max()
        .unwrap_or(10)
        .min(width / 2)
        + 2;
    let mut b = DetailBuilder {
        lang,
        show: app.show_secrets,
        label_w,
        width,
        lines: Vec::new(),
        hinted: false,
    };

    // Title + type/folder line.
    for l in wrap(&item.name, width) {
        b.lines.push(Line::from(Span::styled(l, bold())));
    }
    let mut meta = type_name(lang, item.item_type).to_owned();
    if let Some(folder) = item.folder_id.as_deref().and_then(|id| app.folder_name(id)) {
        meta.push_str(&format!(
            " · {}: {}",
            lang.t(M::FieldFolder),
            sanitize(folder)
        ));
    }
    if item.favorite {
        meta.push_str(&format!(" · * {}", lang.t(M::FieldFavorite)));
    }
    b.lines.push(Line::from(Span::styled(meta, dim())));
    b.lines.push(Line::default());

    match item.item_type {
        ItemType::Login => {
            if let Some(login) = &item.login {
                b.row(lang.t(M::FieldUsername), &login.username, bold());
                b.secret(lang.t(M::FieldPassword), &login.password, mask());
                let seed = login.totp.trim();
                if !seed.is_empty() {
                    match totp::totp_at(seed, app.unix_time()) {
                        Ok(code) => {
                            let code_text = group_code(&code.code);
                            let remaining =
                                lang.tf(M::TotpRemaining, &[("s", &code.remaining.to_string())]);
                            b.lines.push(Line::from(vec![
                                Span::styled(
                                    format!("{:<w$}", lang.t(M::FieldTotp), w = label_w),
                                    dim(),
                                ),
                                Span::styled(code_text, bold()),
                                Span::styled(format!("   {remaining}"), dim()),
                            ]));
                        }
                        Err(_) => {
                            b.row(lang.t(M::FieldTotp), lang.t(M::TotpInvalid), error_style())
                        }
                    }
                }
                for (i, uri) in login
                    .uris
                    .iter()
                    .filter(|u| !u.uri.trim().is_empty())
                    .enumerate()
                {
                    let label = if i == 0 { lang.t(M::FieldWebsite) } else { "" };
                    b.row(label, &uri.uri, Style::new());
                }
            }
        }
        ItemType::Card => {
            if let Some(card) = &item.card {
                b.row(lang.t(M::FieldCardholder), &card.cardholder_name, bold());
                b.row(lang.t(M::FieldBrand), &card.brand, Style::new());
                let digits: String = card.number.chars().filter(char::is_ascii_digit).collect();
                let masked = if digits.len() >= 4 {
                    format!(
                        "{} {}",
                        std::iter::repeat_n(MASK_CHAR, 4).collect::<String>(),
                        &digits[digits.len() - 4..]
                    )
                } else {
                    mask()
                };
                b.secret(lang.t(M::FieldCardNumber), &card.number, masked);
                let expiry = match (card.exp_month.trim(), card.exp_year.trim()) {
                    ("", "") => String::new(),
                    (m, "") => m.to_owned(),
                    ("", y) => y.to_owned(),
                    (m, y) => format!("{m}/{y}"),
                };
                b.row(lang.t(M::FieldExpiry), &expiry, Style::new());
                b.secret(
                    lang.t(M::FieldSecurityCode),
                    &card.code,
                    std::iter::repeat_n(MASK_CHAR, 3).collect(),
                );
            }
        }
        ItemType::Identity => {
            if let Some(id) = &item.identity {
                let full = [id.title.trim(), id.first_name.trim(), id.last_name.trim()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" ");
                b.row(lang.t(M::FieldName), &full, bold());
                b.row(lang.t(M::FieldEmail), &id.email, Style::new());
                b.row(lang.t(M::FieldPhone), &id.phone, Style::new());
                b.row(lang.t(M::FieldCompany), &id.company, Style::new());
                let city = format!("{} {}", id.postal_code.trim(), id.city.trim());
                let address: Vec<&str> = [
                    id.address1.trim(),
                    id.address2.trim(),
                    city.trim(),
                    id.state.trim(),
                    id.country.trim(),
                ]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect();
                for (i, part) in address.iter().enumerate() {
                    let label = if i == 0 { lang.t(M::FieldAddress) } else { "" };
                    b.row(label, part, Style::new());
                }
                b.row(lang.t(M::FieldUsername), &id.username, Style::new());
            }
        }
        ItemType::Note => {}
    }

    if !item.fields.is_empty() {
        b.heading(M::CustomFields);
        for f in &item.fields {
            let name = sanitize(&f.name);
            match f.kind {
                FieldKind::Text => b.row(&name, &f.value, Style::new()),
                FieldKind::Hidden => b.secret(&name, &f.value, mask()),
                FieldKind::Boolean => {
                    let v = if f.value.trim().eq_ignore_ascii_case("true") {
                        M::Yes
                    } else {
                        M::No
                    };
                    b.row(&name, lang.t(v), Style::new());
                }
            }
        }
    }

    if !item.notes.trim().is_empty() {
        b.heading(M::FieldNotes);
        b.text_block(&item.notes);
    }

    b.blank();
    b.row(
        lang.t(M::FieldUpdated),
        &lang.relative_time(item.updated_at, now_ms()),
        dim(),
    );
    b.lines
}

fn draw_detail(frame: &mut Frame, area: Rect, app: &mut App) {
    let lang = app.lang;
    let block = panel(lang.t(M::Details), app.focus == Focus::Detail);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let content = inner.inner(ratatui::layout::Margin::new(1, 0));
    if content.width == 0 || content.height == 0 {
        return;
    }
    let lines = match app.selected_item() {
        Some(item) => detail_lines(app, item, usize::from(content.width)),
        None => vec![Line::from(Span::styled(
            lang.t(if app.items.is_empty() {
                M::NoItems
            } else {
                M::NoSelection
            })
            .to_owned(),
            dim(),
        ))],
    };
    let max_scroll =
        u16::try_from(lines.len().saturating_sub(usize::from(content.height))).unwrap_or(u16::MAX);
    app.detail_max_scroll = max_scroll;
    app.detail_scroll = app.detail_scroll.min(max_scroll);
    frame.render_widget(
        Paragraph::new(lines).scroll((app.detail_scroll, 0)),
        content,
    );
}
