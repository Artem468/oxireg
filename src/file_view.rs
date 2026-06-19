use std::{
    fs::File,
    io::{self, BufRead, BufReader, Seek, SeekFrom},
    path::PathBuf,
};

pub const VIEW_MARGIN_LINES: usize = 120;
const MAX_LINE_BYTES: usize = 32 * 1024;

pub struct FileView {
    pub path: PathBuf,
    reader: BufReader<File>,
    line_offsets: Vec<u64>,
    eof: bool,
}

impl FileView {
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let file = File::open(&path)?;
        Ok(Self {
            path,
            reader: BufReader::new(file),
            line_offsets: vec![0],
            eof: false,
        })
    }

    pub fn known_lines(&self) -> usize {
        self.line_offsets.len()
    }

    pub fn byte_offset_for_line(&self, line: usize) -> Option<u64> {
        self.line_offsets.get(line).copied()
    }

    pub fn ensure_line_index(&mut self, line: usize) -> io::Result<()> {
        if self.eof || line < self.line_offsets.len() {
            return Ok(());
        }

        let start = *self.line_offsets.last().unwrap_or(&0);
        self.reader.seek(SeekFrom::Start(start))?;
        let mut offset = start;
        let mut buffer = Vec::new();

        while self.line_offsets.len() <= line {
            buffer.clear();
            let read = self.reader.read_until(b'\n', &mut buffer)?;
            if read == 0 {
                self.eof = true;
                break;
            }
            offset += read as u64;
            self.line_offsets.push(offset);
        }
        Ok(())
    }

    pub fn read_window(
        &mut self,
        start_line: usize,
        visible_lines: usize,
    ) -> io::Result<Vec<TextLine>> {
        let target = start_line + visible_lines + VIEW_MARGIN_LINES;
        self.ensure_line_index(target)?;

        let start_offset = self
            .line_offsets
            .get(start_line)
            .copied()
            .unwrap_or_else(|| *self.line_offsets.last().unwrap_or(&0));
        self.reader.seek(SeekFrom::Start(start_offset))?;

        let mut lines = Vec::with_capacity(visible_lines);
        let mut buffer = Vec::new();
        for line_no in start_line..start_line + visible_lines {
            buffer.clear();
            let read = self.reader.read_until(b'\n', &mut buffer)?;
            if read == 0 {
                break;
            }
            let truncated = buffer.len() > MAX_LINE_BYTES;
            if truncated {
                buffer.truncate(MAX_LINE_BYTES);
            }
            let text = String::from_utf8_lossy(&buffer)
                .trim_end_matches(['\r', '\n'])
                .to_string();
            lines.push(TextLine {
                number: line_no + 1,
                text,
                truncated,
            });
        }
        Ok(lines)
    }

    pub fn read_lines_by_numbers(&mut self, line_numbers: &[usize]) -> io::Result<Vec<TextLine>> {
        let mut lines = Vec::with_capacity(line_numbers.len());
        let mut buffer = Vec::new();

        for &line_number in line_numbers {
            if line_number == 0 {
                continue;
            }

            let line_idx = line_number - 1;
            self.ensure_line_index(line_idx)?;
            let Some(offset) = self.line_offsets.get(line_idx).copied() else {
                continue;
            };

            self.reader.seek(SeekFrom::Start(offset))?;
            buffer.clear();
            let read = self.reader.read_until(b'\n', &mut buffer)?;
            if read == 0 {
                continue;
            }

            let truncated = buffer.len() > MAX_LINE_BYTES;
            if truncated {
                buffer.truncate(MAX_LINE_BYTES);
            }
            let text = String::from_utf8_lossy(&buffer)
                .trim_end_matches(['\r', '\n'])
                .to_string();
            lines.push(TextLine {
                number: line_number,
                text,
                truncated,
            });
        }

        Ok(lines)
    }
}

pub struct TextLine {
    pub number: usize,
    pub text: String,
    pub truncated: bool,
}
