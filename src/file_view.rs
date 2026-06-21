use std::{
    fs::File,
    io::{self, BufRead, BufReader, Seek, SeekFrom},
    path::PathBuf,
};

pub const VIEW_MARGIN_LINES: usize = 120;
const MAX_LINE_BYTES: usize = 32 * 1024;
const LINE_CHECKPOINT_STRIDE: usize = 128;

pub struct FileView {
    pub path: PathBuf,
    reader: BufReader<File>,
    line_checkpoints: Vec<u64>,
    indexed_lines: usize,
    indexed_offset: u64,
    eof: bool,
}

impl FileView {
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let file = File::open(&path)?;
        Ok(Self {
            path,
            reader: BufReader::new(file),
            line_checkpoints: vec![0],
            indexed_lines: 1,
            indexed_offset: 0,
            eof: false,
        })
    }

    pub fn known_lines(&self) -> usize {
        self.indexed_lines
    }

    pub fn byte_offset_for_line(&mut self, line: usize) -> io::Result<Option<u64>> {
        self.offset_for_line(line)
    }

    pub fn ensure_line_index(&mut self, line: usize) -> io::Result<()> {
        if self.eof || line < self.indexed_lines {
            return Ok(());
        }

        self.reader.seek(SeekFrom::Start(self.indexed_offset))?;
        let mut buffer = Vec::new();

        while self.indexed_lines <= line {
            buffer.clear();
            let read = self.reader.read_until(b'\n', &mut buffer)?;
            if read == 0 {
                self.eof = true;
                break;
            }
            self.indexed_offset += read as u64;
            if self.indexed_lines.is_multiple_of(LINE_CHECKPOINT_STRIDE) {
                self.line_checkpoints.push(self.indexed_offset);
            }
            self.indexed_lines += 1;
        }
        Ok(())
    }

    fn offset_for_line(&mut self, line: usize) -> io::Result<Option<u64>> {
        self.ensure_line_index(line)?;
        if line >= self.indexed_lines {
            return Ok(None);
        }

        let checkpoint_idx = line / LINE_CHECKPOINT_STRIDE;
        let checkpoint_line = checkpoint_idx * LINE_CHECKPOINT_STRIDE;
        let Some(mut offset) = self.line_checkpoints.get(checkpoint_idx).copied() else {
            return Ok(None);
        };
        if checkpoint_line == line {
            return Ok(Some(offset));
        }

        self.reader.seek(SeekFrom::Start(offset))?;
        let mut buffer = Vec::new();
        for _ in checkpoint_line..line {
            buffer.clear();
            let read = self.reader.read_until(b'\n', &mut buffer)?;
            if read == 0 {
                return Ok(None);
            }
            offset += read as u64;
        }
        Ok(Some(offset))
    }

    pub fn read_window(
        &mut self,
        start_line: usize,
        visible_lines: usize,
    ) -> io::Result<Vec<TextLine>> {
        let target = start_line + visible_lines + VIEW_MARGIN_LINES;
        self.ensure_line_index(target)?;

        let start_offset = self
            .offset_for_line(start_line)?
            .unwrap_or(self.indexed_offset);
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
            let Some(offset) = self.offset_for_line(line_idx)? else {
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
