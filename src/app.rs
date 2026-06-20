use std::{
    io,
    time::{Duration, Instant},
};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use regex::Regex;

use crate::{
    explain,
    export::{self, ExportFormat},
    file_view::{FileView, VIEW_MARGIN_LINES},
    match_index::MatchIndex,
    regex_debug::{RegexFlags, capture_descriptions, compile_regex},
};

const REGEX_DEBOUNCE: Duration = Duration::from_millis(80);

pub struct App {
    pub file: FileView,
    pub regex_input: String,
    pub compiled: Option<Regex>,
    pub regex_error: Option<String>,
    pub explanation: Vec<String>,
    pub captures: Vec<String>,
    pub match_index: MatchIndex,
    pub flags: RegexFlags,
    pub scroll_line: usize,
    pub cursor: usize,
    pub collapse_matches: bool,
    pub frequency_collapsed: bool,
    pub status_collapsed: bool,
    pub right_panel_percent: u16,
    pub frequency_area: Rect,
    pub status_area: Rect,
    pub export_format: ExportFormat,
    pub export_status: Option<String>,
    pub should_quit: bool,
    history: Vec<String>,
    history_cursor: Option<usize>,
    last_regex_edit: Instant,
    regex_dirty: bool,
}

impl App {
    pub fn new(file: FileView) -> Self {
        Self {
            file,
            regex_input: String::new(),
            compiled: None,
            regex_error: None,
            explanation: explain::explain(""),
            captures: Vec::new(),
            match_index: MatchIndex::new(),
            flags: RegexFlags::default(),
            scroll_line: 0,
            cursor: 0,
            collapse_matches: false,
            frequency_collapsed: false,
            status_collapsed: false,
            right_panel_percent: 32,
            frequency_area: Rect::default(),
            status_area: Rect::default(),
            export_format: ExportFormat::Json,
            export_status: None,
            should_quit: false,
            history: Vec::new(),
            history_cursor: None,
            last_regex_edit: Instant::now(),
            regex_dirty: true,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> io::Result<()> {
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }

        match key {
            KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }
            | KeyEvent {
                code: KeyCode::Esc, ..
            } => self.should_quit = true,
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => self.push_history_current(),
            KeyEvent {
                code: KeyCode::F(2),
                ..
            } => self.frequency_collapsed = !self.frequency_collapsed,
            KeyEvent {
                code: KeyCode::F(3),
                ..
            } => self.status_collapsed = !self.status_collapsed,
            KeyEvent {
                code: KeyCode::F(4),
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('g'),
                modifiers: KeyModifiers::ALT,
                ..
            } => {
                self.collapse_matches = !self.collapse_matches;
                self.scroll_line = 0;
            }
            KeyEvent {
                code: KeyCode::F(5),
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('i'),
                modifiers: KeyModifiers::ALT,
                ..
            } => {
                self.flags.toggle_case_insensitive();
                self.mark_regex_dirty();
            }
            KeyEvent {
                code: KeyCode::F(6),
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('m'),
                modifiers: KeyModifiers::ALT,
                ..
            } => {
                self.flags.toggle_multi_line();
                self.mark_regex_dirty();
            }
            KeyEvent {
                code: KeyCode::F(7),
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('s'),
                modifiers: KeyModifiers::ALT,
                ..
            } => {
                self.flags.toggle_dot_matches_new_line();
                self.mark_regex_dirty();
            }
            KeyEvent {
                code: KeyCode::F(8),
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('n'),
                modifiers: KeyModifiers::ALT,
                ..
            } => self.jump_to_next_match()?,
            KeyEvent {
                code: KeyCode::F(9),
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('p'),
                modifiers: KeyModifiers::ALT,
                ..
            } => self.jump_to_prev_match()?,
            KeyEvent {
                code: KeyCode::Char('e'),
                modifiers: KeyModifiers::ALT,
                ..
            } => {
                self.export_format = self.export_format.next();
                self.export_status = Some(format!("export format: {}", self.export_format.label()));
            }
            KeyEvent {
                code: KeyCode::Char('e'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => self.export_current_matches(),
            KeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::ALT,
                ..
            } => self.right_panel_percent = self.right_panel_percent.saturating_sub(2).max(20),
            KeyEvent {
                code: KeyCode::Right,
                modifiers: KeyModifiers::ALT,
                ..
            } => self.right_panel_percent = (self.right_panel_percent + 2).min(60),
            KeyEvent {
                code: KeyCode::Char(ch),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
                ..
            } => {
                self.regex_input.insert(self.cursor, ch);
                self.cursor += ch.len_utf8();
                self.mark_regex_dirty();
            }
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } if self.cursor > 0 => {
                if let Some((idx, _)) = self.regex_input[..self.cursor].char_indices().last() {
                    self.regex_input.drain(idx..self.cursor);
                    self.cursor = idx;
                    self.mark_regex_dirty();
                }
            }
            KeyEvent {
                code: KeyCode::Delete,
                ..
            } if self.cursor < self.regex_input.len() => {
                if let Some(ch) = self.regex_input[self.cursor..].chars().next() {
                    self.regex_input
                        .drain(self.cursor..self.cursor + ch.len_utf8());
                    self.mark_regex_dirty();
                }
            }
            KeyEvent {
                code: KeyCode::Left,
                ..
            } => self.move_cursor_left(),
            KeyEvent {
                code: KeyCode::Right,
                ..
            } => self.move_cursor_right(),
            KeyEvent {
                code: KeyCode::Home,
                modifiers: KeyModifiers::CONTROL,
                ..
            } => self.scroll_line = 0,
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => self.cursor = 0,
            KeyEvent {
                code: KeyCode::End,
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.scroll_line = if self.collapse_matches {
                    self.match_index.matching_line_count().saturating_sub(1)
                } else {
                    self.file.known_lines().saturating_sub(1)
                }
            }
            KeyEvent {
                code: KeyCode::End, ..
            } => self.cursor = self.regex_input.len(),
            KeyEvent {
                code: KeyCode::Up, ..
            } if key.modifiers == KeyModifiers::CONTROL => self.history_prev(),
            KeyEvent {
                code: KeyCode::Down,
                ..
            } if key.modifiers == KeyModifiers::CONTROL => self.history_next(),
            KeyEvent {
                code: KeyCode::Up, ..
            } => self.scroll_line = self.scroll_line.saturating_sub(1),
            KeyEvent {
                code: KeyCode::Down,
                ..
            } => {
                if self.collapse_matches {
                    self.scroll_line = (self.scroll_line + 1)
                        .min(self.match_index.matching_line_count().saturating_sub(1));
                } else {
                    self.scroll_line += 1;
                    self.file
                        .ensure_line_index(self.scroll_line + VIEW_MARGIN_LINES)?;
                }
            }
            KeyEvent {
                code: KeyCode::PageUp,
                ..
            } => self.scroll_line = self.scroll_line.saturating_sub(25),
            KeyEvent {
                code: KeyCode::PageDown,
                ..
            } => {
                if self.collapse_matches {
                    self.scroll_line = (self.scroll_line + 25)
                        .min(self.match_index.matching_line_count().saturating_sub(1));
                } else {
                    self.scroll_line += 25;
                    self.file
                        .ensure_line_index(self.scroll_line + VIEW_MARGIN_LINES)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let x = mouse.column;
                let y = mouse.row;
                if contains(self.frequency_area, x, y) {
                    self.frequency_collapsed = !self.frequency_collapsed;
                } else if contains(self.status_area, x, y) {
                    self.status_collapsed = !self.status_collapsed;
                }
            }
            MouseEventKind::ScrollUp => self.scroll_up(3),
            MouseEventKind::ScrollDown => {
                let _ = self.scroll_down(3);
            }
            _ => {}
        }
    }

    pub fn handle_paste(&mut self, text: &str) {
        self.insert_pasted_text(text);
    }

    fn insert_pasted_text(&mut self, text: &str) {
        let pasted = text.replace(['\r', '\n'], "");
        if pasted.is_empty() {
            return;
        }

        self.regex_input.insert_str(self.cursor, &pasted);
        self.cursor += pasted.len();
        self.mark_regex_dirty();
    }

    pub fn maybe_compile_regex(&mut self) {
        if !self.regex_dirty || self.last_regex_edit.elapsed() < REGEX_DEBOUNCE {
            return;
        }
        self.regex_dirty = false;

        if self.regex_input.is_empty() {
            self.compiled = None;
            self.regex_error = None;
            self.explanation = explain::explain("");
            self.captures.clear();
            self.match_index.clear();
            return;
        }

        match compile_regex(&self.regex_input, self.flags) {
            Ok(regex) => {
                self.captures = capture_descriptions(&regex);
                self.match_index
                    .start(self.file.path.clone(), regex.clone());
                self.compiled = Some(regex);
                self.regex_error = None;
            }
            Err(err) => {
                self.compiled = None;
                self.regex_error = Some(err.to_string());
                self.explanation = explain::explain(&self.regex_input);
                self.captures.clear();
                self.match_index.clear();
            }
        }
        if self.regex_error.is_none() {
            self.explanation = explain::explain(&self.regex_input);
        }
    }

    pub fn drain_match_index(&mut self) {
        self.match_index.drain();
    }

    fn mark_regex_dirty(&mut self) {
        self.last_regex_edit = Instant::now();
        self.regex_dirty = true;
        self.history_cursor = None;
        self.export_status = None;
    }

    fn move_cursor_left(&mut self) {
        if let Some((idx, _)) = self.regex_input[..self.cursor].char_indices().last() {
            self.cursor = idx;
        }
    }

    fn move_cursor_right(&mut self) {
        if self.cursor < self.regex_input.len() {
            if let Some(ch) = self.regex_input[self.cursor..].chars().next() {
                self.cursor += ch.len_utf8();
            }
        }
    }

    fn jump_to_next_match(&mut self) -> io::Result<()> {
        if self.collapse_matches {
            self.scroll_line = (self.scroll_line + 1)
                .min(self.match_index.matching_line_count().saturating_sub(1));
            return Ok(());
        }

        if let Some(line) = self.match_index.next_line_after(self.scroll_line + 1) {
            self.scroll_line = line.saturating_sub(1);
            self.file
                .ensure_line_index(self.scroll_line + VIEW_MARGIN_LINES)?;
        }
        Ok(())
    }

    fn jump_to_prev_match(&mut self) -> io::Result<()> {
        if self.collapse_matches {
            self.scroll_line = self.scroll_line.saturating_sub(1);
            return Ok(());
        }

        if let Some(line) = self.match_index.prev_line_before(self.scroll_line + 1) {
            self.scroll_line = line.saturating_sub(1);
            self.file
                .ensure_line_index(self.scroll_line + VIEW_MARGIN_LINES)?;
        }
        Ok(())
    }

    fn scroll_up(&mut self, amount: usize) {
        self.scroll_line = self.scroll_line.saturating_sub(amount);
    }

    fn scroll_down(&mut self, amount: usize) -> io::Result<()> {
        if self.collapse_matches {
            self.scroll_line = (self.scroll_line + amount)
                .min(self.match_index.matching_line_count().saturating_sub(1));
        } else {
            self.scroll_line += amount;
            self.file
                .ensure_line_index(self.scroll_line + VIEW_MARGIN_LINES)?;
        }
        Ok(())
    }

    fn export_current_matches(&mut self) {
        let Some(regex) = self.compiled.as_ref() else {
            self.export_status = Some("export failed: regex is empty or invalid".to_string());
            return;
        };

        match export::export_matches(&self.file.path, regex, self.export_format) {
            Ok(result) => {
                self.export_status = Some(format!(
                    "exported {} matches to {}",
                    result.matches,
                    result.path.display()
                ));
            }
            Err(err) => {
                self.export_status = Some(format!("export failed: {err}"));
            }
        }
    }

    fn push_history_current(&mut self) {
        if self.regex_input.is_empty() {
            return;
        }
        if self.history.last() == Some(&self.regex_input) {
            return;
        }
        if let Some(pos) = self
            .history
            .iter()
            .position(|entry| entry == &self.regex_input)
        {
            self.history.remove(pos);
        }
        self.history.push(self.regex_input.clone());
        self.history_cursor = None;
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next_idx = match self.history_cursor {
            Some(idx) => idx.saturating_sub(1),
            None => self.history.len() - 1,
        };
        self.load_history(next_idx);
    }

    fn history_next(&mut self) {
        let Some(idx) = self.history_cursor else {
            return;
        };
        if idx + 1 >= self.history.len() {
            self.history_cursor = None;
            return;
        }
        self.load_history(idx + 1);
    }

    fn load_history(&mut self, idx: usize) {
        if let Some(entry) = self.history.get(idx) {
            self.regex_input = entry.clone();
            self.cursor = self.regex_input.len();
            self.history_cursor = Some(idx);
            self.last_regex_edit = Instant::now();
            self.regex_dirty = true;
        }
    }
}

fn contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x
        && y >= area.y
        && x < area.x.saturating_add(area.width)
        && y < area.y.saturating_add(area.height)
}
