use crate::app::{App, InputMode};
use crate::model::LinkHealth;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
const GOOD: Color = Color::Green;
const WARN: Color = Color::Yellow;
const BAD: Color = Color::Red;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(area);

    draw_title(frame, chunks[0], app);
    draw_filters(frame, chunks[1], app);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(chunks[2]);

    let visible = body[0].height.saturating_sub(2) as usize;
    app.clamp_offset(visible);
    draw_list(frame, body[0], app, visible);
    draw_detail(frame, body[1], app);
    draw_footer(frame, chunks[3], app);

    match app.mode {
        InputMode::Help => draw_help(frame, area),
        InputMode::DomainBrowser => draw_domain_browser(frame, area, app),
        InputMode::YearBrowser => draw_year_browser(frame, area, app),
        InputMode::Digest => draw_digest(frame, area, app),
        _ => {}
    }
}

fn draw_title(frame: &mut Frame, area: Rect, app: &App) {
    let stats = app.library.stats();
    let title = Line::from(vec![
        Span::styled(
            " drip ",
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!(
                "{} shown · {} total · {} unread · dead {} · unchecked {}",
                app.filtered.len(),
                stats.total,
                stats.unread,
                stats.link_dead,
                stats.link_unknown
            ),
            Style::default().fg(DIM),
        ),
    ]);
    frame.render_widget(Paragraph::new(title), area);
}

fn draw_filters(frame: &mut Frame, area: Rect, app: &App) {
    let folder = app.folder_filter.as_deref().unwrap_or("all");
    let domain = app.domain_filter.as_deref().unwrap_or("all");
    let year = app
        .year_filter
        .map(|y| y.to_string())
        .unwrap_or_else(|| "all".into());

    let line = Line::from(vec![
        Span::styled(" folder:", Style::default().fg(DIM)),
        Span::styled(format!(" {folder} "), Style::default().fg(WARN)),
        Span::styled(" domain:", Style::default().fg(DIM)),
        Span::styled(format!(" {domain} "), Style::default().fg(WARN)),
        Span::styled(" year:", Style::default().fg(DIM)),
        Span::styled(format!(" {year} "), Style::default().fg(WARN)),
        Span::styled(" status:", Style::default().fg(DIM)),
        Span::styled(
            format!(" {} ", app.status_filter.label()),
            Style::default().fg(WARN),
        ),
        Span::styled(" links:", Style::default().fg(DIM)),
        Span::styled(
            format!(" {} ", app.link_filter.label()),
            Style::default().fg(WARN),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn link_style(health: LinkHealth, selected: bool) -> Style {
    let fg = match health {
        LinkHealth::Unknown => DIM,
        LinkHealth::Alive => GOOD,
        LinkHealth::Redirect => WARN,
        LinkHealth::Dead => BAD,
        LinkHealth::Error => Color::Magenta,
    };
    if selected {
        Style::default().fg(fg).bg(Color::Rgb(30, 40, 50))
    } else {
        Style::default().fg(fg)
    }
}

fn draw_list(frame: &mut Frame, area: Rect, app: &App, visible: usize) {
    let end = (app.list_offset + visible).min(app.filtered.len());
    let items: Vec<ListItem> = app.filtered[app.list_offset..end]
        .iter()
        .enumerate()
        .map(|(row, &idx)| {
            let b = &app.library.bookmarks[idx];
            let abs = app.list_offset + row;
            let selected = abs == app.selected;
            let style = if selected {
                Style::default().bg(Color::Rgb(30, 40, 50)).fg(Color::White)
            } else {
                Style::default()
            };

            let created = b
                .created
                .map(|c| c.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "????-??-??".into());

            let title = if b.title.is_empty() {
                b.url.clone()
            } else {
                truncate(&b.title, area.width.saturating_sub(26) as usize)
            };

            let line = Line::from(vec![
                Span::styled(
                    format!(" {} ", b.status.glyph()),
                    if selected {
                        Style::default().fg(GOOD).bg(Color::Rgb(30, 40, 50))
                    } else {
                        Style::default().fg(GOOD)
                    },
                ),
                Span::styled(
                    format!("{} ", b.link_health.glyph()),
                    link_style(b.link_health, selected),
                ),
                Span::styled(format!("{created} "), Style::default().fg(DIM).patch(style)),
                Span::styled(
                    title,
                    style.add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
                ),
            ]);
            ListItem::new(line)
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .title(Span::styled(" bookmarks ", Style::default().fg(ACCENT)));
    frame.render_widget(List::new(items).block(block), area);
}

fn draw_detail(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .title(Span::styled(" detail ", Style::default().fg(ACCENT)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(b) = app.selected_bookmark() else {
        frame.render_widget(
            Paragraph::new(Span::styled("no selection", Style::default().fg(DIM))),
            inner,
        );
        return;
    };

    let tags = if b.tags.is_empty() {
        "—".into()
    } else {
        b.tags.join(", ")
    };
    let created = b
        .created
        .map(|c| c.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "—".into());
    let last = b
        .last_opened
        .map(|c| c.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "never".into());
    let domain = b.domain();
    let opens = b.open_count.to_string();
    let status = b.status.as_str();
    let link = b.link_summary();
    let excerpt = if b.excerpt.is_empty() {
        "—".to_string()
    } else {
        b.excerpt.clone()
    };
    let note = if b.note.is_empty() {
        "—".to_string()
    } else {
        b.note.clone()
    };

    let text = vec![
        Line::from(Span::styled(
            b.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        line_kv("url", &b.url),
        line_kv("domain", &domain),
        line_kv("folder", &b.folder),
        line_kv("tags", &tags),
        line_kv("created", &created),
        line_kv("status", status),
        line_kv("link", &link),
        line_kv("opens", &opens),
        line_kv("last open", &last),
        Line::from(""),
        Line::from(Span::styled("excerpt", Style::default().fg(DIM))),
        Line::from(excerpt),
        Line::from(""),
        Line::from(Span::styled("note", Style::default().fg(DIM))),
        Line::from(note),
    ];

    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), inner);
}

fn line_kv<'a>(k: &'a str, v: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{k:10}"), Style::default().fg(DIM)),
        Span::raw(v.to_string()),
    ])
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let content = match app.mode {
        InputMode::Search => Line::from(vec![
            Span::styled(" / ", Style::default().fg(Color::Black).bg(ACCENT)),
            Span::raw(format!("{}█", app.query)),
            Span::styled("  esc clear · enter apply", Style::default().fg(DIM)),
        ]),
        InputMode::DomainBrowser => {
            if app.domain_query.is_empty() {
                Line::from(Span::styled(
                    "↑↓/ctrl-j/k move · type to filter · enter apply · esc close",
                    Style::default().fg(DIM),
                ))
            } else {
                Line::from(vec![
                    Span::styled(" domain/ ", Style::default().fg(Color::Black).bg(ACCENT)),
                    Span::raw(format!("{}█", app.domain_query)),
                    Span::styled(
                        format!("  ({} matches)", app.domain_list.len()),
                        Style::default().fg(DIM),
                    ),
                ])
            }
        }
        InputMode::YearBrowser => Line::from(Span::styled(
            "j/k year · enter scrub that year · 0 clear · esc",
            Style::default().fg(DIM),
        )),
        InputMode::Digest => Line::from(Span::styled(
            "today's dig · enter open · j/k · n skip · z reshuffle · esc dismiss",
            Style::default().fg(WARN),
        )),
        InputMode::Normal => {
            let msg = if app.message.is_empty() {
                "j/k · / search · D domains · Y years · z digest · c check · ? help · q quit".into()
            } else {
                app.message.clone()
            };
            let color = if app.is_checking() {
                WARN
            } else if app.message.is_empty() {
                DIM
            } else {
                WARN
            };
            Line::from(Span::styled(msg, Style::default().fg(color)))
        }
        InputMode::Help => Line::from(Span::styled(
            " press any key to close help ",
            Style::default().fg(DIM),
        )),
    };
    frame.render_widget(Paragraph::new(content), area);
}

fn draw_domain_browser(frame: &mut Frame, area: Rect, app: &mut App) {
    let popup = centered_rect(60, 75, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " domains (by count) ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let visible = inner.height as usize;
    if app.domain_selected < app.domain_offset {
        app.domain_offset = app.domain_selected;
    } else if app.domain_selected >= app.domain_offset + visible {
        app.domain_offset = app.domain_selected + 1 - visible;
    }
    let end = (app.domain_offset + visible).min(app.domain_list.len());

    let items: Vec<ListItem> = app.domain_list[app.domain_offset..end]
        .iter()
        .enumerate()
        .map(|(row, (name, count))| {
            let abs = app.domain_offset + row;
            let selected = abs == app.domain_selected;
            let style = if selected {
                Style::default()
                    .bg(Color::Rgb(30, 40, 50))
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let active = app.domain_filter.as_deref() == Some(name.as_str());
            let marker = if active { "▸ " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(marker, Style::default().fg(GOOD).patch(style)),
                Span::styled(
                    format!("{count:>5}  "),
                    Style::default().fg(DIM).patch(style),
                ),
                Span::styled(name.clone(), style),
            ]))
        })
        .collect();

    frame.render_widget(List::new(items), inner);
}

fn draw_year_browser(frame: &mut Frame, area: Rect, app: &App) {
    let popup = centered_rect(40, 70, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " year scrub ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "pick a year to review never-opened links",
            Style::default().fg(DIM),
        )),
        Line::from(""),
    ];

    for (i, (year, count)) in app.year_list.iter().enumerate() {
        let selected = i == app.year_selected;
        let active = app.year_filter == Some(*year);
        let marker = if active {
            "▸"
        } else if selected {
            "•"
        } else {
            " "
        };
        let style = if selected {
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(30, 40, 50))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {marker} "),
                Style::default().fg(GOOD).patch(style),
            ),
            Span::styled(format!("{year}  "), style),
            Span::styled(
                format!("{count} bookmarks"),
                Style::default().fg(DIM).patch(style),
            ),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_digest(frame: &mut Frame, area: Rect, app: &App) {
    let popup = centered_rect(72, 55, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " today's dig — 3 to revisit ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "forgotten saves, never opened, not dead — diverse domains",
            Style::default().fg(DIM),
        )),
        Line::from(""),
    ];

    if app.digest_indices.is_empty() {
        lines.push(Line::from(Span::styled(
            "nothing to resurface. nice?",
            Style::default().fg(GOOD),
        )));
    } else {
        for (i, &idx) in app.digest_indices.iter().enumerate() {
            let b = &app.library.bookmarks[idx];
            let selected = i == app.digest_selected;
            let style = if selected {
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(30, 40, 50))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let year = b
                .created
                .map(|c| c.format("%Y").to_string())
                .unwrap_or_else(|| "????".into());
            let marker = if selected { "▸" } else { " " };
            let title = if b.title.is_empty() {
                b.url.clone()
            } else {
                truncate(&b.title, 56)
            };

            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {marker} "),
                    Style::default().fg(GOOD).patch(style),
                ),
                Span::styled(
                    format!("{}. ", i + 1),
                    Style::default().fg(DIM).patch(style),
                ),
                Span::styled(title, style),
            ]));
            lines.push(Line::from(vec![
                Span::raw("     "),
                Span::styled(
                    format!("{} · {} · {}", year, b.domain(), b.link_health.glyph()),
                    Style::default().fg(DIM),
                ),
            ]));
            lines.push(Line::from(""));
        }
    }

    lines.push(Line::from(Span::styled(
        "enter open · n skip · z reshuffle · esc start browsing",
        Style::default().fg(DIM),
    )));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(74, 82, area);
    frame.render_widget(Clear, popup);

    let help = r#"
 drip — raindrop bookmarks, local-first

 NAVIGATION
   j / ↓       move down          k / ↑    move up
   ctrl-d/u    page               g / G    top / bottom
   enter       open URL           y        copy URL

 SEARCH & FILTER
   /           fuzzy search
   f           cycle folder
   s           cycle status       l        cycle link health
   D           domain browser     .        filter domain of selected
   Y           year scrub picker  cycle year with shift-y feel via Y menu
   0           clear ALL filters
   esc         clear search / close overlays

 REVISIT
   z           show / reshuffle today's dig (3 resurfaced links)
   r           random never-opened pick
   n           skip (mark skipped) — great in year scrub

 SYNC
   S           pull from Raindrop API (needs: drip auth)

 DEAD LINKS
   c           check selected     C        recheck all in view
   ctrl-c      check unchecked in view     x  cancel

 STATUS
   space       cycle status       d        mark done

 GENERAL
   w save   ? help   q quit
"#;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " help ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    frame.render_widget(
        Paragraph::new(help.trim_start())
            .block(block)
            .style(Style::default().fg(Color::White)),
        popup,
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else if max <= 1 {
        "…".into()
    } else {
        let t: String = s.chars().take(max - 1).collect();
        format!("{t}…")
    }
}
