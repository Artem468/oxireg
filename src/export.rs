use std::{
    fs::File,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use regex::Regex;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportFormat {
    Json,
    Csv,
    Text,
}

impl ExportFormat {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "json" => Some(Self::Json),
            "csv" => Some(Self::Csv),
            "txt" | "text" => Some(Self::Text),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Csv => "csv",
            Self::Text => "txt",
        }
    }

    pub fn extension(self) -> &'static str {
        self.label()
    }

    pub fn next(self) -> Self {
        match self {
            Self::Json => Self::Csv,
            Self::Csv => Self::Text,
            Self::Text => Self::Json,
        }
    }
}

pub struct ExportResult {
    pub path: PathBuf,
    pub matches: usize,
}

pub fn export_matches(
    path: &Path,
    regex: &Regex,
    format: ExportFormat,
) -> io::Result<ExportResult> {
    let output_path = output_path(format);
    let input = File::open(path)?;
    let mut reader = BufReader::new(input);
    let output = File::create(&output_path)?;
    let mut writer = io::BufWriter::new(output);
    let names: Vec<Option<&str>> = regex.capture_names().collect();

    match format {
        ExportFormat::Json => writer.write_all(b"[\n")?,
        ExportFormat::Csv => writer.write_all(b"line,match_start,match_end,match,captures\n")?,
        ExportFormat::Text => {}
    }

    let mut line_number = 0;
    let mut matches = 0;
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        let read = reader.read_until(b'\n', &mut buffer)?;
        if read == 0 {
            break;
        }

        line_number += 1;
        let text = String::from_utf8_lossy(&buffer);
        for captures in regex.captures_iter(&text) {
            let Some(matched) = captures.get(0) else {
                continue;
            };
            if matched.start() == matched.end() {
                continue;
            }

            matches += 1;
            match format {
                ExportFormat::Json => {
                    if matches > 1 {
                        writer.write_all(b",\n")?;
                    }
                    write_json_record(&mut writer, line_number, matched, &captures, &names)?
                }
                ExportFormat::Csv => {
                    write_csv(&mut writer, line_number, matched, &captures, &names)?
                }
                ExportFormat::Text => write_text(&mut writer, line_number, matched)?,
            }
        }
    }

    if format == ExportFormat::Json {
        writer.write_all(b"\n]\n")?;
    }
    writer.flush()?;
    Ok(ExportResult {
        path: output_path,
        matches,
    })
}

fn output_path(format: ExportFormat) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    PathBuf::from(format!(
        "oxireg-export-{}-{now}.{}",
        std::process::id(),
        format.extension()
    ))
}

fn write_json_record(
    writer: &mut impl Write,
    line: usize,
    matched: regex::Match<'_>,
    captures: &regex::Captures<'_>,
    names: &[Option<&str>],
) -> io::Result<()> {
    write!(
        writer,
        "{{\"line\":{line},\"match_start\":{},\"match_end\":{},\"match\":\"{}\",\"captures\":",
        matched.start(),
        matched.end(),
        json_escape(matched.as_str())
    )?;
    write_captures_json(writer, captures, names)?;
    writer.write_all(b"}")
}

fn write_csv(
    writer: &mut impl Write,
    line: usize,
    matched: regex::Match<'_>,
    captures: &regex::Captures<'_>,
    names: &[Option<&str>],
) -> io::Result<()> {
    let mut captures_json = Vec::new();
    write_captures_json(&mut captures_json, captures, names)?;
    writeln!(
        writer,
        "{line},{},{},{},{}",
        matched.start(),
        matched.end(),
        csv_escape(matched.as_str()),
        csv_escape(&String::from_utf8_lossy(&captures_json))
    )
}

fn write_text(writer: &mut impl Write, line: usize, matched: regex::Match<'_>) -> io::Result<()> {
    writeln!(
        writer,
        "{}:{}-{} {}",
        line,
        matched.start(),
        matched.end(),
        matched.as_str().trim_end_matches(['\r', '\n'])
    )
}

fn write_captures_json(
    writer: &mut impl Write,
    captures: &regex::Captures<'_>,
    names: &[Option<&str>],
) -> io::Result<()> {
    writer.write_all(b"{")?;
    let mut first = true;
    for idx in 0..captures.len() {
        let Some(capture) = captures.get(idx) else {
            continue;
        };
        if !first {
            writer.write_all(b",")?;
        }
        first = false;
        let key = names
            .get(idx)
            .and_then(|name| *name)
            .map(str::to_string)
            .unwrap_or_else(|| idx.to_string());
        write!(
            writer,
            "\"{}\":\"{}\"",
            json_escape(&key),
            json_escape(capture.as_str())
        )?;
    }
    writer.write_all(b"}")
}

fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{csv_escape, json_escape};

    #[test]
    fn escapes_json_strings() {
        assert_eq!(json_escape("a\"b\\c\n"), "a\\\"b\\\\c\\n");
    }

    #[test]
    fn escapes_csv_strings() {
        assert_eq!(csv_escape("a,b\"c"), "\"a,b\"\"c\"");
    }
}
