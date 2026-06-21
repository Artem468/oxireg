use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
};

use regex::{CaptureNames, Regex};

const BATCH_SIZE: usize = 1024;
const CHUNK_LINES: usize = 4096;
const MAX_QUEUED_CHUNKS_PER_WORKER: usize = 2;
const MATCH_MAP_BUCKETS: usize = 8192;
const MAX_FREQUENCY_VALUES_PER_GROUP: usize = 256;

type LineNumber = u32;
type FrequencyBatch = BTreeMap<usize, BTreeMap<String, u64>>;

pub struct FrequencyDelta {
    pub group: String,
    pub value: String,
    pub count: u64,
}

pub enum ScanMessage {
    Batch {
        generation: u64,
        chunk_index: usize,
        matching_lines: Vec<LineNumber>,
        map_buckets: Vec<u16>,
        hit_count: u64,
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
    matching_lines: Vec<LineNumber>,
    pub hit_count: u64,
    pub bytes_scanned: u64,
    pub lines_scanned: usize,
    pub file_size: u64,
    pub complete: bool,
    pub error: Option<String>,
    pub frequencies: BTreeMap<String, BTreeMap<String, u64>>,
    map_buckets: Vec<u16>,
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
            matching_lines: Vec::new(),
            hit_count: 0,
            bytes_scanned: 0,
            lines_scanned: 0,
            file_size: 0,
            complete: true,
            error: None,
            frequencies: BTreeMap::new(),
            map_buckets: Vec::new(),
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
        self.matching_lines.clear();
        self.hit_count = 0;
        self.bytes_scanned = 0;
        self.lines_scanned = 0;
        self.file_size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        self.complete = false;
        self.error = None;
        self.frequencies.clear();
        self.map_buckets.clear();
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
        self.matching_lines.clear();
        self.hit_count = 0;
        self.bytes_scanned = 0;
        self.lines_scanned = 0;
        self.file_size = 0;
        self.complete = true;
        self.error = None;
        self.frequencies.clear();
        self.map_buckets.clear();
        self.next_chunk = 0;
        self.pending.clear();
    }

    pub fn drain(&mut self) {
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                ScanMessage::Batch {
                    generation,
                    chunk_index,
                    mut matching_lines,
                    mut map_buckets,
                    hit_count,
                    frequencies,
                    bytes_scanned,
                    lines_scanned,
                } if generation == self.generation => {
                    self.pending.insert(
                        chunk_index,
                        PendingBatch {
                            matching_lines: std::mem::take(&mut matching_lines),
                            map_buckets: std::mem::take(&mut map_buckets),
                            hit_count,
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
        self.matching_lines
            .partition_point(|line| line_to_usize(*line) <= current_line)
            .checked_sub(0)
            .and_then(|idx| self.matching_lines.get(idx))
            .copied()
            .map(line_to_usize)
            .or_else(|| self.matching_lines.first().copied().map(line_to_usize))
    }

    pub fn prev_line_before(&self, current_line: usize) -> Option<usize> {
        let idx = self
            .matching_lines
            .partition_point(|line| line_to_usize(*line) < current_line);
        if idx > 0 {
            self.matching_lines.get(idx - 1).copied().map(line_to_usize)
        } else {
            self.matching_lines.last().copied().map(line_to_usize)
        }
    }

    pub fn matching_lines_window(&self, start: usize, len: usize) -> Vec<usize> {
        self.matching_lines
            .iter()
            .skip(start)
            .take(len)
            .copied()
            .map(line_to_usize)
            .collect()
    }

    pub fn matching_line_count(&self) -> usize {
        self.matching_lines.len()
    }

    pub fn match_map_rows(&self, rows: usize) -> Vec<bool> {
        let mut marks = vec![false; rows];
        if rows == 0 {
            return marks;
        }

        for &bucket in &self.map_buckets {
            let row = ((bucket as usize).saturating_mul(rows) / MATCH_MAP_BUCKETS)
                .min(rows.saturating_sub(1));
            marks[row] = true;
        }
        marks
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
                percent, self.hit_count, self.lines_scanned
            ),
        }
    }

    fn cancel_current(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.cancel = None;
    }

    fn apply_frequencies(&mut self, frequencies: Vec<FrequencyDelta>) {
        for delta in frequencies {
            let values = self.frequencies.entry(delta.group).or_default();
            if let Some(count) = values.get_mut(&delta.value) {
                *count += delta.count;
            } else if values.len() < MAX_FREQUENCY_VALUES_PER_GROUP {
                values.insert(delta.value, delta.count);
            }
        }
    }

    fn apply_matching_lines(&mut self, lines: &[LineNumber]) {
        for &line in lines {
            if self.matching_lines.last().copied() != Some(line) {
                self.matching_lines.push(line);
            }
        }
    }

    fn apply_map_buckets(&mut self, buckets: &[u16]) {
        for &bucket in buckets {
            if self.map_buckets.last().copied() != Some(bucket) {
                self.map_buckets.push(bucket);
            }
        }
    }

    fn apply_pending(&mut self) {
        while let Some(batch) = self.pending.remove(&self.next_chunk) {
            self.apply_matching_lines(&batch.matching_lines);
            self.apply_map_buckets(&batch.map_buckets);
            self.hit_count += batch.hit_count;
            self.apply_frequencies(batch.frequencies);
            self.bytes_scanned = self.bytes_scanned.max(batch.bytes_scanned);
            self.lines_scanned = self.lines_scanned.max(batch.lines_scanned);
            self.next_chunk += 1;
        }
    }
}

struct PendingBatch {
    matching_lines: Vec<LineNumber>,
    map_buckets: Vec<u16>,
    hit_count: u64,
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

    let file_size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let named_groups = named_capture_names(regex.capture_names());
    let worker_count = thread::available_parallelism()
        .map(|count| count.get().saturating_sub(1).max(1))
        .unwrap_or(1);
    let (work_sender, work_receiver) = mpsc::sync_channel(
        worker_count
            .saturating_mul(MAX_QUEUED_CHUNKS_PER_WORKER)
            .max(1),
    );
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
                file_size,
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
            text: std::mem::take(&mut line),
        });
        byte_offset += read as u64;

        if chunk.len() >= CHUNK_LINES {
            let work = WorkChunk {
                index: chunk_index,
                lines: std::mem::take(&mut chunk),
                bytes_scanned: byte_offset,
                lines_scanned: line_number,
            };
            if !send_work(&work_sender, work, &cancel) {
                return;
            }
            chunk_index += 1;
        }
    }

    if !chunk.is_empty()
        && !send_work(
            &work_sender,
            WorkChunk {
                index: chunk_index,
                lines: chunk,
                bytes_scanned: byte_offset,
                lines_scanned: line_number,
            },
            &cancel,
        )
    {
        return;
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
    text: Vec<u8>,
}

struct WorkChunk {
    index: usize,
    lines: Vec<ScanLine>,
    bytes_scanned: u64,
    lines_scanned: usize,
}

fn send_work(sender: &SyncSender<WorkChunk>, mut work: WorkChunk, cancel: &AtomicBool) -> bool {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }

        match sender.try_send(work) {
            Ok(()) => return true,
            Err(TrySendError::Full(returned_work)) => {
                work = returned_work;
                thread::yield_now();
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

fn worker_loop(
    receiver: Arc<Mutex<Receiver<WorkChunk>>>,
    sender: Sender<ScanMessage>,
    regex: Regex,
    named_groups: Vec<(usize, String)>,
    file_size: u64,
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
        process_chunk(&sender, generation, &regex, &named_groups, file_size, work);
    }
}

fn process_chunk(
    sender: &Sender<ScanMessage>,
    generation: u64,
    regex: &Regex,
    named_groups: &[(usize, String)],
    file_size: u64,
    work: WorkChunk,
) {
    let mut matching_lines = Vec::with_capacity(BATCH_SIZE);
    let mut map_buckets = Vec::new();
    let mut bucket_seen = vec![false; MATCH_MAP_BUCKETS];
    let mut hit_count = 0;
    let mut frequencies: FrequencyBatch = BTreeMap::new();

    for line in work.lines {
        let text = String::from_utf8_lossy(&line.text);
        let mut line_matched = false;

        if named_groups.is_empty() {
            for matched in regex.find_iter(&text) {
                if matched.start() == matched.end() {
                    continue;
                }
                hit_count += 1;
                line_matched = true;
                mark_match_bucket(
                    &mut bucket_seen,
                    &mut map_buckets,
                    line.byte_offset + matched.start() as u64,
                    file_size,
                );
            }
        } else {
            for captures in regex.captures_iter(&text) {
                let Some(matched) = captures.get(0) else {
                    continue;
                };
                if matched.start() == matched.end() {
                    continue;
                }
                hit_count += 1;
                line_matched = true;
                mark_match_bucket(
                    &mut bucket_seen,
                    &mut map_buckets,
                    line.byte_offset + matched.start() as u64,
                    file_size,
                );
                for (idx, _) in named_groups {
                    if let Some(value) = captures.get(*idx) {
                        increment_frequency(&mut frequencies, *idx, value.as_str());
                    }
                }
            }
        }

        if line_matched && let Some(line_number) = to_line_number(line.line) {
            matching_lines.push(line_number);
        }
    }

    map_buckets.sort_unstable();
    let _ = sender.send(ScanMessage::Batch {
        generation,
        chunk_index: work.index,
        matching_lines,
        map_buckets,
        hit_count,
        frequencies: take_frequencies(&mut frequencies, named_groups),
        bytes_scanned: work.bytes_scanned,
        lines_scanned: work.lines_scanned,
    });
}

fn increment_frequency(frequencies: &mut FrequencyBatch, group_idx: usize, value: &str) {
    let values = frequencies.entry(group_idx).or_default();
    if let Some(count) = values.get_mut(value) {
        *count += 1;
        return;
    }

    if values.len() >= MAX_FREQUENCY_VALUES_PER_GROUP {
        return;
    }

    values.insert(value.to_string(), 1);
}

fn mark_match_bucket(seen: &mut [bool], buckets: &mut Vec<u16>, byte_offset: u64, file_size: u64) {
    let bucket = match_bucket(byte_offset, file_size);
    if !seen[bucket as usize] {
        seen[bucket as usize] = true;
        buckets.push(bucket);
    }
}

fn match_bucket(byte_offset: u64, file_size: u64) -> u16 {
    if file_size == 0 {
        return 0;
    }
    ((byte_offset.saturating_mul(MATCH_MAP_BUCKETS as u64)) / file_size)
        .min(MATCH_MAP_BUCKETS.saturating_sub(1) as u64) as u16
}

fn to_line_number(line: usize) -> Option<LineNumber> {
    line.try_into().ok()
}

fn line_to_usize(line: LineNumber) -> usize {
    line as usize
}

fn named_capture_names(capture_names: CaptureNames<'_>) -> Vec<(usize, String)> {
    capture_names
        .enumerate()
        .filter_map(|(idx, name)| name.map(|name| (idx, name.to_string())))
        .collect()
}

fn take_frequencies(
    frequencies: &mut FrequencyBatch,
    named_groups: &[(usize, String)],
) -> Vec<FrequencyDelta> {
    std::mem::take(frequencies)
        .into_iter()
        .flat_map(|(idx, values)| {
            let group = named_groups
                .iter()
                .find_map(|(group_idx, name)| (*group_idx == idx).then_some(name.clone()));
            values.into_iter().filter_map(move |(value, count)| {
                group.clone().map(|group| FrequencyDelta {
                    group,
                    value,
                    count,
                })
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_FREQUENCY_VALUES_PER_GROUP, increment_frequency, named_capture_names, take_frequencies,
    };
    use regex::Regex;
    use std::collections::BTreeMap;

    #[test]
    fn keeps_frequency_group_names_without_cloning_names_per_hit() {
        let regex = Regex::new(r"(?P<kind>\w+)").unwrap();
        let groups = named_capture_names(regex.capture_names());
        let mut frequencies = BTreeMap::from([(1, BTreeMap::from([("error".to_string(), 2)]))]);

        assert_eq!(
            take_frequencies(&mut frequencies, &groups)
                .into_iter()
                .map(|delta| (delta.group, delta.value, delta.count))
                .collect::<Vec<_>>(),
            vec![("kind".to_string(), "error".to_string(), 2)]
        );
    }

    #[test]
    fn caps_unique_frequency_values_per_group() {
        let mut frequencies = BTreeMap::new();

        for idx in 0..MAX_FREQUENCY_VALUES_PER_GROUP + 10 {
            increment_frequency(&mut frequencies, 1, &format!("value-{idx}"));
        }

        increment_frequency(&mut frequencies, 1, "value-1");

        let values = frequencies.get(&1).unwrap();
        assert_eq!(values.len(), MAX_FREQUENCY_VALUES_PER_GROUP);
        assert_eq!(values.get("value-1"), Some(&2));
    }
}
