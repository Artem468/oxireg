use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};
use regex::{Regex, RegexBuilder};

const GROUP_COLORS: [Color; 8] = [
    Color::Yellow,
    Color::Cyan,
    Color::Green,
    Color::Magenta,
    Color::LightBlue,
    Color::LightGreen,
    Color::LightMagenta,
    Color::LightCyan,
];

#[derive(Clone, Copy, Default)]
pub struct RegexFlags {
    pub case_insensitive: bool,
    pub multi_line: bool,
    pub dot_matches_new_line: bool,
}

impl RegexFlags {
    pub fn toggle_case_insensitive(&mut self) {
        self.case_insensitive = !self.case_insensitive;
    }

    pub fn toggle_multi_line(&mut self) {
        self.multi_line = !self.multi_line;
    }

    pub fn toggle_dot_matches_new_line(&mut self) {
        self.dot_matches_new_line = !self.dot_matches_new_line;
    }

    pub fn label(self) -> String {
        format!(
            "i:{} m:{} s:{}",
            flag_state(self.case_insensitive),
            flag_state(self.multi_line),
            flag_state(self.dot_matches_new_line)
        )
    }
}

fn flag_state(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

pub fn compile_regex(pattern: &str, flags: RegexFlags) -> Result<Regex, regex::Error> {
    RegexBuilder::new(pattern)
        .case_insensitive(flags.case_insensitive)
        .multi_line(flags.multi_line)
        .dot_matches_new_line(flags.dot_matches_new_line)
        .build()
}

pub fn capture_descriptions(regex: &Regex) -> Vec<String> {
    regex
        .capture_names()
        .enumerate()
        .map(|(idx, name)| match name {
            Some(name) => format!("{idx}: (?P<{name}>)"),
            None => format!("{idx}: <unnamed>"),
        })
        .collect()
}

pub fn highlight_line<'a>(
    line: &'a str,
    regex: Option<&Regex>,
    width: usize,
    truncated: bool,
) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let visible = truncate_char_boundary(line, width);
    let Some(regex) = regex else {
        push_plain_line(&mut spans, visible, line, truncated);
        return spans;
    };

    let ranges = group_ranges(visible, regex);
    if ranges.is_empty() {
        push_plain_line(&mut spans, visible, line, truncated);
        return spans;
    }

    let mut boundaries = Vec::with_capacity(ranges.len() * 2 + 2);
    boundaries.push(0);
    boundaries.push(visible.len());
    for range in &ranges {
        boundaries.push(range.start);
        boundaries.push(range.end);
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    for pair in boundaries.windows(2) {
        let start = pair[0];
        let end = pair[1];
        if start == end {
            continue;
        }

        if let Some(range) = best_range_for_segment(&ranges, start, end) {
            spans.push(Span::styled(
                &visible[start..end],
                group_style(range.group_index),
            ));
        } else {
            spans.push(Span::raw(&visible[start..end]));
        }
    }

    push_truncation_marker(&mut spans, visible, line, truncated);
    spans
}

fn push_plain_line<'a>(
    spans: &mut Vec<Span<'a>>,
    visible: &'a str,
    original: &str,
    truncated: bool,
) {
    spans.push(Span::raw(visible));
    push_truncation_marker(spans, visible, original, truncated);
}

fn push_truncation_marker<'a>(
    spans: &mut Vec<Span<'a>>,
    visible: &str,
    original: &str,
    truncated: bool,
) {
    if truncated || visible.len() < original.len() {
        spans.push(Span::styled(" ...", Style::default().fg(Color::DarkGray)));
    }
}

#[derive(Clone, Copy)]
struct GroupRange {
    start: usize,
    end: usize,
    group_index: usize,
}

fn group_ranges(line: &str, regex: &Regex) -> Vec<GroupRange> {
    let mut ranges = Vec::new();
    let has_capture_groups = regex.captures_len() > 1;

    for captures in regex.captures_iter(line) {
        if !has_capture_groups {
            if let Some(matched) = captures.get(0) {
                if matched.start() != matched.end() {
                    ranges.push(GroupRange {
                        start: matched.start(),
                        end: matched.end(),
                        group_index: 0,
                    });
                }
            }
            continue;
        }

        for group_index in 1..captures.len() {
            if let Some(matched) = captures.get(group_index) {
                if matched.start() != matched.end() {
                    ranges.push(GroupRange {
                        start: matched.start(),
                        end: matched.end(),
                        group_index,
                    });
                }
            }
        }
    }

    ranges
}

fn best_range_for_segment(ranges: &[GroupRange], start: usize, end: usize) -> Option<GroupRange> {
    ranges
        .iter()
        .copied()
        .filter(|range| range.start <= start && end <= range.end)
        .min_by_key(|range| {
            let len = range.end - range.start;
            (len, usize::MAX - range.group_index)
        })
}

fn group_style(group_index: usize) -> Style {
    let color = GROUP_COLORS[group_index % GROUP_COLORS.len()];
    let fg = match color {
        Color::Magenta => Color::White,
        _ => Color::Black,
    };

    Style::default()
        .fg(fg)
        .bg(color)
        .add_modifier(Modifier::BOLD)
}

fn truncate_char_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
