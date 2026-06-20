use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
};

use regex::Regex;

const BATCH_SIZE: usize = 1024;
const CHUNK_LINES: usize = 4096;

#[derive(Clone)]
pub struct MatchHit {
    pub line: usize,
    pub byte_offset: u64,
}

pub struct FrequencyDelta {
    pub group: String,
    pub value: String,
    pub count: u64,
}

pub enum ScanMessage {
    Batch {
        generation: u64,
        chunk_index: usize,
        hits: Vec<MatchHit>,
        frequencies: Vec<FrequencyDelta>,
        bytes_scanned: u64,
        lines_scanned: usize,
    },
    Done {
        generation: u64,
        bytes_scanned: u64,
        lines_scanned: usize,
    },
    Error {
        generation: u64,
        message: String,
    },
}

pub struct MatchIndex {
    pub hits: Vec<MatchHit>,
    pub matching_lines: Vec<usize>,
    pub bytes_scanned: u64,
    pub lines_scanned: usize,
    pub file_size: u64,
    pub complete: bool,
    pub error: Option<String>,
    pub frequencies: BTreeMap<String, BTreeMap<String, u64>>,
    generation: u64,
    next_chunk: usize,
    pending: BTreeMap<usize, PendingBatch>,
    receiver: Receiver<ScanMessage>,
    cancel: Option<Arc<AtomicBool>>,
    worker: Option<JoinHandle<()>>,
}

impl MatchIndex {
    pub fn new() -> Self {
        let (_sender, receiver) = mpsc::channel();
        Self {
            hits: Vec::new(),
            matching_lines: Vec::new(),
            bytes_scanned: 0,
            lines_scanned: 0,
            file_size: 0,
            complete: true,
            error: None,
            frequencies: BTreeMap::new(),
            generation: 0,
            next_chunk: 0,
            pending: BTreeMap::new(),
            receiver,
            cancel: None,
            worker: None,
        }
    }

    pub fn start(&mut self, path: PathBuf, regex: Regex) {
        self.cancel_current();

        self.generation = self.generation.wrapping_add(1);
        self.hits.clear();
        self.matching_lines.clear();
        self.bytes_scanned = 0;
        self.lines_scanned = 0;
        self.file_size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        self.complete = false;
        self.error = None;
        self.frequencies.clear();
        self.next_chunk = 0;
        self.pending.clear();

        let generation = self.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (sender, receiver) = mpsc::channel();
        self.receiver = receiver;
        self.cancel = Some(cancel);
        self.worker = Some(thread::spawn(move || {
            scan_file(path, regex, generation, worker_cancel, sender);
        }));
    }

    pub fn clear(&mut self) {
        self.cancel_current();
        self.generation = self.generation.wrapping_add(1);
        self.hits.clear();
        self.matching_lines.clear();
        self.bytes_scanned = 0;
        self.lines_scanned = 0;
        self.file_size = 0;
        self.complete = true;
        self.error = None;
        self.frequencies.clear();
        self.next_chunk = 0;
        self.pending.clear();
    }

    pub fn drain(&mut self) {
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                ScanMessage::Batch {
                    generation,
                    chunk_index,
                    mut hits,
                    frequencies,
                    bytes_scanned,
                    lines_scanned,
                } if generation == self.generation => {
                    self.pending.insert(
                        chunk_index,
                        PendingBatch {
                            hits: std::mem::take(&mut hits),
                            frequencies,
                            bytes_scanned,
                            lines_scanned,
                        },
                    );
                    self.apply_pending();
                }
                ScanMessage::Done {
                    generation,
                    bytes_scanned,
                    lines_scanned,
                } if generation == self.generation => {
                    self.bytes_scanned = bytes_scanned;
                    self.lines_scanned = lines_scanned;
                    self.complete = true;
                }
                ScanMessage::Error {
                    generation,
                    message,
                } if generation == self.generation => {
                    self.error = Some(message);
                    self.complete = true;
                }
                _ => {}
            }
        }
    }

    pub fn next_line_after(&self, current_line: usize) -> Option<usize> {
        self.hits
            .partition_point(|hit| hit.line <= current_line)
            .checked_sub(0)
            .and_then(|idx| self.hits.get(idx))
            .map(|hit| hit.line)
            .or_else(|| self.hits.first().map(|hit| hit.line))
    }

    pub fn prev_line_before(&self, current_line: usize) -> Option<usize> {
        let idx = self.hits.partition_point(|hit| hit.line < current_line);
        if idx > 0 {
            self.hits.get(idx - 1).map(|hit| hit.line)
        } else {
            self.hits.last().map(|hit| hit.line)
        }
    }

    pub fn matching_lines_window(&self, start: usize, len: usize) -> Vec<usize> {
        self.matching_lines
            .iter()
            .skip(start)
            .take(len)
            .copied()
            .collect()
    }

    pub fn matching_line_count(&self) -> usize {
        self.matching_lines.len()
    }

    pub fn top_frequency_bars(&self, limit: usize) -> Option<(String, Vec<(String, u64)>)> {
        let (group, values) = self
            .frequencies
            .iter()
            .max_by_key(|(_, values)| values.values().sum::<u64>())?;

        let mut bars: Vec<(String, u64)> = values
            .iter()
            .map(|(value, count)| (value.clone(), *count))
            .collect();
        bars.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        bars.truncate(limit);
        Some((group.clone(), bars))
    }

    pub fn progress_label(&self) -> String {
        let state = if self.complete { "done" } else { "scan" };
        let percent = if self.file_size > 0 {
            (self.bytes_scanned as f64 / self.file_size as f64 * 100.0).min(100.0)
        } else {
            100.0
        };
        match &self.error {
            Some(error) => format!("index error: {error}"),
            None => format!(
                "{state} {:.0}% | hits {} | lines {}",
                percent,
                self.hits.len(),
                self.lines_scanned
            ),
        }
    }

    fn cancel_current(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        let _ = self.worker.take();
        self.cancel = None;
    }

    fn apply_frequencies(&mut self, frequencies: Vec<FrequencyDelta>) {
        for delta in frequencies {
            *self
                .frequencies
                .entry(delta.group)
                .or_default()
                .entry(delta.value)
                .or_default() += delta.count;
        }
    }

    fn apply_hits(&mut self, hits: &[MatchHit]) {
        for hit in hits {
            if self.matching_lines.last().copied() != Some(hit.line) {
                self.matching_lines.push(hit.line);
            }
        }
    }

    fn apply_pending(&mut self) {
        while let Some(mut batch) = self.pending.remove(&self.next_chunk) {
            self.apply_hits(&batch.hits);
            self.hits.append(&mut batch.hits);
            self.apply_frequencies(batch.frequencies);
            self.bytes_scanned = self.bytes_scanned.max(batch.bytes_scanned);
            self.lines_scanned = self.lines_scanned.max(batch.lines_scanned);
            self.next_chunk += 1;
        }
    }
}

struct PendingBatch {
    hits: Vec<MatchHit>,
    frequencies: Vec<FrequencyDelta>,
    bytes_scanned: u64,
    lines_scanned: usize,
}

impl Drop for MatchIndex {
    fn drop(&mut self) {
        self.cancel_current();
    }
}

fn scan_file(
    path: PathBuf,
    regex: Regex,
    generation: u64,
    cancel: Arc<AtomicBool>,
    sender: Sender<ScanMessage>,
) {
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(err) => {
            let _ = sender.send(ScanMessage::Error {
                generation,
                message: err.to_string(),
            });
            return;
        }
    };

    let named_groups: Vec<(usize, String)> = regex
        .capture_names()
        .enumerate()
        .filter_map(|(idx, name)| name.map(|name| (idx, name.to_string())))
        .collect();
    let worker_count = thread::available_parallelism()
        .map(|count| count.get().saturating_sub(1).max(1))
        .unwrap_or(1);
    let (work_sender, work_receiver) = mpsc::channel();
    let work_receiver = Arc::new(Mutex::new(work_receiver));
    let mut workers = Vec::with_capacity(worker_count);

    for _ in 0..worker_count {
        let worker_receiver = Arc::clone(&work_receiver);
        let worker_sender = sender.clone();
        let worker_regex = regex.clone();
        let worker_groups = named_groups.clone();
        let worker_cancel = Arc::clone(&cancel);
        workers.push(thread::spawn(move || {
            worker_loop(
                worker_receiver,
                worker_sender,
                worker_regex,
                worker_groups,
                generation,
                worker_cancel,
            );
        }));
    }

    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut line_number = 0;
    let mut byte_offset = 0_u64;
    let mut chunk_index = 0;
    let mut chunk = Vec::with_capacity(CHUNK_LINES);

    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }

        line.clear();
        let read = match reader.read_until(b'\n', &mut line) {
            Ok(read) => read,
            Err(err) => {
                let _ = sender.send(ScanMessage::Error {
                    generation,
                    message: err.to_string(),
                });
                return;
            }
        };
        if read == 0 {
            break;
        }

        line_number += 1;
        chunk.push(ScanLine {
            line: line_number,
            byte_offset,
            text: String::from_utf8_lossy(&line).into_owned(),
        });
        byte_offset += read as u64;

        if chunk.len() >= CHUNK_LINES {
            let work = WorkChunk {
                index: chunk_index,
                lines: std::mem::take(&mut chunk),
                bytes_scanned: byte_offset,
                lines_scanned: line_number,
            };
            if work_sender.send(work).is_err() {
                return;
            }
            chunk_index += 1;
        }
    }

    if !chunk.is_empty() {
        let _ = work_sender.send(WorkChunk {
            index: chunk_index,
            lines: chunk,
            bytes_scanned: byte_offset,
            lines_scanned: line_number,
        });
    }
    drop(work_sender);
    for worker in workers {
        let _ = worker.join();
    }

    let _ = sender.send(ScanMessage::Done {
        generation,
        bytes_scanned: byte_offset,
        lines_scanned: line_number,
    });
}

struct ScanLine {
    line: usize,
    byte_offset: u64,
    text: String,
}

struct WorkChunk {
    index: usize,
    lines: Vec<ScanLine>,
    bytes_scanned: u64,
    lines_scanned: usize,
}

fn worker_loop(
    receiver: Arc<Mutex<Receiver<WorkChunk>>>,
    sender: Sender<ScanMessage>,
    regex: Regex,
    named_groups: Vec<(usize, String)>,
    generation: u64,
    cancel: Arc<AtomicBool>,
) {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let work = {
            let Ok(receiver) = receiver.lock() else {
                return;
            };
            receiver.recv()
        };
        let Ok(work) = work else {
            return;
        };
        process_chunk(&sender, generation, &regex, &named_groups, work);
    }
}

fn process_chunk(
    sender: &Sender<ScanMessage>,
    generation: u64,
    regex: &Regex,
    named_groups: &[(usize, String)],
    work: WorkChunk,
) {
    let mut hits = Vec::with_capacity(BATCH_SIZE);
    let mut frequencies: BTreeMap<(String, String), u64> = BTreeMap::new();

    for line in work.lines {
        for captures in regex.captures_iter(&line.text) {
            let Some(matched) = captures.get(0) else {
                continue;
            };
            if matched.start() == matched.end() {
                continue;
            }
            hits.push(MatchHit {
                line: line.line,
                byte_offset: line.byte_offset + matched.start() as u64,
            });
            for (idx, name) in named_groups {
                if let Some(value) = captures.get(*idx) {
                    *frequencies
                        .entry((name.clone(), value.as_str().to_string()))
                        .or_default() += 1;
                }
            }
        }
    }

    let _ = sender.send(ScanMessage::Batch {
        generation,
        chunk_index: work.index,
        hits,
        frequencies: take_frequencies(&mut frequencies),
        bytes_scanned: work.bytes_scanned,
        lines_scanned: work.lines_scanned,
    });
}

fn take_frequencies(frequencies: &mut BTreeMap<(String, String), u64>) -> Vec<FrequencyDelta> {
    std::mem::take(frequencies)
        .into_iter()
        .map(|((group, value), count)| FrequencyDelta {
            group,
            value,
            count,
        })
        .collect()
}
