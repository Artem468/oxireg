use std::io;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    symbols::Marker,
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, Paragraph, Wrap,
        canvas::{Canvas, Points},
    },
};

use crate::{app::App, file_view::TextLine, regex_debug::highlight_line};

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(frame.area());

    draw_regex_input(frame, root[0], app);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(100 - app.right_panel_percent),
            Constraint::Percentage(app.right_panel_percent),
        ])
        .split(root[1]);

    let text_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(body[0]);

    let visible_height = text_chunks[0].height.saturating_sub(2) as usize;
    let lines_result = if app.collapse_matches {
        let line_numbers = app
            .match_index
            .matching_lines_window(app.scroll_line, visible_height);
        app.file.read_lines_by_numbers(&line_numbers)
    } else {
        app.file.read_window(app.scroll_line, visible_height)
    };

    match lines_result {
        Ok(lines) => {
            draw_text(frame, text_chunks[0], app, &lines);
            draw_match_map(frame, text_chunks[1], app, &lines);
            draw_sidebar(frame, body[1], app, &lines);
        }
        Err(err) => {
            draw_file_error(frame, text_chunks[0], err);
            draw_match_map(frame, text_chunks[1], app, &[]);
            draw_sidebar(frame, body[1], app, &[]);
        }
    }
    draw_status(frame, root[2], app);
}

fn draw_regex_input(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let status_style = if app.regex_error.is_some() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Green)
    };
    let title = if app.regex_input.is_empty() {
        format!("Regex [{}]", app.flags.label())
    } else if app.regex_error.is_some() {
        format!("Regex - invalid [{}]", app.flags.label())
    } else {
        format!("Regex - ready [{}]", app.flags.label())
    };
    let paragraph = Paragraph::new(app.regex_input.as_str())
        .style(status_style)
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(paragraph, area);

    let cursor_x = area.x + 1 + app.cursor.min(area.width.saturating_sub(2) as usize) as u16;
    frame.set_cursor_position((cursor_x, area.y + 1));
}

fn draw_text(frame: &mut Frame<'_>, area: Rect, app: &App, lines: &[TextLine]) {
    let number_width = lines
        .last()
        .map(|line| line.number.to_string().len())
        .unwrap_or(1)
        .max(4);
    let text_width = area.width.saturating_sub(number_width as u16 + 4).max(1) as usize;

    let rendered: Vec<Line<'_>> = lines
        .iter()
        .map(|line| {
            let mut spans = vec![Span::styled(
                format!("{:>width$} | ", line.number, width = number_width),
                Style::default().fg(Color::DarkGray),
            )];
            spans.extend(highlight_line(
                &line.text,
                app.compiled.as_ref(),
                text_width,
                line.truncated,
            ));
            Line::from(spans)
        })
        .collect();

    let title = if app.collapse_matches {
        format!("Text - {} - matching lines only", app.file.path.display())
    } else {
        format!("Text - {}", app.file.path.display())
    };
    let paragraph = Paragraph::new(rendered)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn draw_file_error(frame: &mut Frame<'_>, area: Rect, err: io::Error) {
    let paragraph = Paragraph::new(err.to_string())
        .style(Style::default().fg(Color::Red))
        .block(Block::default().borders(Borders::ALL).title("Text"));
    frame.render_widget(paragraph, area);
}

fn draw_match_map(frame: &mut Frame<'_>, area: Rect, app: &App, lines: &[TextLine]) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let rows = area.height as usize;
    let mut marks = vec![' '; rows];
    let file_size = app.match_index.file_size.max(1);

    for hit in &app.match_index.hits {
        let row = ((hit.byte_offset.saturating_mul(rows as u64)) / file_size)
            .min(rows.saturating_sub(1) as u64) as usize;
        marks[row] = '\u{2588}';
    }

    let current_line = if app.collapse_matches {
        lines
            .first()
            .map(|line| line.number.saturating_sub(1))
            .unwrap_or(0)
    } else {
        app.scroll_line
    };

    if let Some(offset) = app.file.byte_offset_for_line(current_line) {
        let row = ((offset.saturating_mul(rows as u64)) / file_size)
            .min(rows.saturating_sub(1) as u64) as usize;
        if marks[row] == ' ' {
            marks[row] = '|';
        }
    }

    let rendered: Vec<Line<'_>> = marks
        .into_iter()
        .map(|mark| {
            let style = match mark {
                '\u{2588}' => Style::default().fg(Color::Yellow),
                '|' => Style::default().fg(Color::LightBlue),
                _ => Style::default().fg(Color::DarkGray),
            };
            Line::from(Span::styled(mark.to_string(), style))
        })
        .collect();

    frame.render_widget(Paragraph::new(rendered), area);
}

fn draw_sidebar(frame: &mut Frame<'_>, area: Rect, app: &mut App, lines: &[TextLine]) {
    let constraints = match (app.frequency_collapsed, app.status_collapsed) {
        (true, true) => vec![
            Constraint::Length(3),
            Constraint::Percentage(40),
            Constraint::Percentage(40),
            Constraint::Length(3),
        ],
        (true, false) => vec![
            Constraint::Length(3),
            Constraint::Percentage(32),
            Constraint::Percentage(28),
            Constraint::Percentage(40),
        ],
        (false, true) => vec![
            Constraint::Percentage(40),
            Constraint::Percentage(34),
            Constraint::Percentage(26),
            Constraint::Length(3),
        ],
        (false, false) => vec![
            Constraint::Percentage(34),
            Constraint::Percentage(24),
            Constraint::Percentage(28),
            Constraint::Percentage(14),
        ],
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    app.frequency_area = chunks[0];
    app.status_area = chunks[3];

    draw_frequency(frame, chunks[0], app);
    draw_explain(frame, chunks[1], app);
    draw_groups(frame, chunks[2], app, lines);
    draw_regex_status(frame, chunks[3], app);
}

fn draw_frequency(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.frequency_collapsed {
        frame.render_widget(
            Paragraph::new("collapsed")
                .style(Style::default().fg(Color::DarkGray))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Group Frequency (F2/click)"),
                ),
            area,
        );
        return;
    }

    let limit = area.height.saturating_sub(4).max(1) as usize;
    let Some((group, values)) = app.match_index.top_frequency_bars(limit) else {
        frame.render_widget(
            Paragraph::new("No named group frequencies")
                .style(Style::default().fg(Color::DarkGray))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Group Frequency (F2/click)"),
                ),
            area,
        );
        return;
    };

    let title = format!("Group Frequency - {group} (F2/click)");
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(14), Constraint::Min(1)])
        .split(inner);

    draw_donut(frame, chunks[0], &values);
    draw_frequency_legend(frame, chunks[1], values);
}

fn compact_label(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value;
    }
    let mut label: String = value.chars().take(max_chars.saturating_sub(2)).collect();
    label.push_str("..");
    label
}

const DONUT_COLORS: [Color; 8] = [
    Color::Yellow,
    Color::Cyan,
    Color::Green,
    Color::Magenta,
    Color::LightBlue,
    Color::LightGreen,
    Color::LightMagenta,
    Color::LightCyan,
];

fn draw_donut(frame: &mut Frame<'_>, area: Rect, values: &[(String, u64)]) {
    let total: u64 = values.iter().map(|(_, count)| *count).sum();
    if total == 0 {
        return;
    }

    let segments = donut_segments(values);
    let points = donut_points(&segments);
    let chart = Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([-1.2, 1.2])
        .y_bounds([-1.2, 1.2])
        .paint(|ctx| {
            for (idx, coords) in points.iter().enumerate() {
                ctx.draw(&Points {
                    coords,
                    color: DONUT_COLORS[idx % DONUT_COLORS.len()],
                });
            }
        });

    frame.render_widget(chart, area);
}

fn donut_segments(values: &[(String, u64)]) -> Vec<f64> {
    let total = values.iter().map(|(_, count)| *count).sum::<u64>() as f64;
    let mut acc = 0.0;
    values
        .iter()
        .map(|(_, count)| {
            acc += *count as f64 / total;
            acc
        })
        .collect()
}

fn donut_points(segments: &[f64]) -> Vec<Vec<(f64, f64)>> {
    let mut points = vec![Vec::new(); segments.len()];
    let outer = 1.0_f64;
    let inner = 0.48_f64;
    let step = 0.035_f64;
    let mut y = -outer;
    while y <= outer {
        let mut x = -outer;
        while x <= outer {
            let radius = (x * x + y * y).sqrt();
            if radius >= inner && radius <= outer {
                let mut angle = y.atan2(x) + std::f64::consts::FRAC_PI_2;
                if angle < 0.0 {
                    angle += std::f64::consts::TAU;
                }
                let ratio = angle / std::f64::consts::TAU;
                let idx = segments
                    .iter()
                    .position(|end| ratio <= *end)
                    .unwrap_or(segments.len().saturating_sub(1));
                points[idx].push((x, y));
            }
            x += step;
        }
        y += step;
    }
    points
}

fn draw_frequency_legend(frame: &mut Frame<'_>, area: Rect, values: Vec<(String, u64)>) {
    let items = values
        .into_iter()
        .enumerate()
        .take(area.height as usize)
        .map(|(idx, (value, count))| {
            let color = DONUT_COLORS[idx % DONUT_COLORS.len()];
            ListItem::new(Line::from(vec![
                Span::styled("\u{2588} ", Style::default().fg(color)),
                Span::raw(compact_label(value, 18)),
                Span::styled(format!(" {count}"), Style::default().fg(Color::DarkGray)),
            ]))
        });
    frame.render_widget(List::new(items), area);
}

fn draw_groups(frame: &mut Frame<'_>, area: Rect, app: &App, lines: &[TextLine]) {
    let groups = group_lines(app, lines);
    let capture_items = if groups.is_empty() {
        vec![ListItem::new("No capture groups")]
    } else {
        groups.into_iter().map(ListItem::new).collect()
    };
    frame.render_widget(
        List::new(capture_items).block(Block::default().borders(Borders::ALL).title("Groups")),
        area,
    );
}

fn draw_explain(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items = app
        .explanation
        .iter()
        .map(|line| ListItem::new(line.as_str()));
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("Explain")),
        area,
    );
}

fn draw_regex_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.status_collapsed {
        frame.render_widget(
            Paragraph::new("collapsed")
                .style(Style::default().fg(Color::DarkGray))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Status (F3/click)"),
                ),
            area,
        );
    } else {
        let error = app.regex_error.as_deref().unwrap_or("OK");
        let style = if app.regex_error.is_some() {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        };
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(error)
                .style(style)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Status (F3/click)"),
                )
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}

fn group_lines(app: &App, lines: &[TextLine]) -> Vec<String> {
    let mut out = app.captures.clone();
    let Some(regex) = &app.compiled else {
        return out;
    };

    if regex.captures_len() <= 1 {
        return out;
    }

    let names: Vec<Option<&str>> = regex.capture_names().collect();
    let mut found = 0;
    for line in lines {
        for captures in regex.captures_iter(&line.text) {
            out.push(format!("line {}", line.number));
            for idx in 0..captures.len() {
                if let Some(matched) = captures.get(idx) {
                    let name = names.get(idx).and_then(|name| *name).unwrap_or("");
                    if name.is_empty() {
                        out.push(format!("  ${idx}: {:?}", matched.as_str()));
                    } else {
                        out.push(format!("  ${idx} {name}: {:?}", matched.as_str()));
                    }
                }
            }
            found += 1;
            if found >= 8 {
                return out;
            }
        }
    }

    out
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let tail = app
        .export_status
        .clone()
        .unwrap_or_else(|| format!("loaded line index: {}", app.file.known_lines()));
    let shortcuts = format!(
        "Esc/Ctrl-C quit | Enter history | Ctrl-Up/Down hist | Alt-Left/Right resize | F4 grep:{} | F8/F9 match",
        if app.collapse_matches { "on" } else { "off" },
    );
    let state = format!(
        "Alt-E export:{} | Ctrl-E export | flags {} | {} | {}",
        app.export_format.label(),
        app.flags.label(),
        app.match_index.progress_label(),
        tail
    );
    frame.render_widget(
        Paragraph::new(vec![Line::from(shortcuts), Line::from(state)])
            .style(Style::default().fg(Color::DarkGray)),
        area,
    );
}
