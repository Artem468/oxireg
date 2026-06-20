# oxireg

`oxireg` is a fast terminal regex debugger for local files.

It is built for the cases where browser tools stop being practical: huge logs, private data, remote shells, and workflows where sending text to a website is not acceptable.

## Why

Most regex tools are optimized for small snippets. `oxireg` is optimized for real files.

- Open large logs without loading the whole file into memory.
- Read from a file path or from stdin.
- Edit a regex interactively and see matches immediately.
- Scan the full file in the background while the UI stays responsive.
- Match large files with a chunked parallel scanner.
- Jump between matches without waiting for a full rescan.
- Collapse noise and turn a huge log into an interactive grep view.
- Analyze named capture groups as live frequency distributions.

## Features

### Streaming File Viewer

The text viewport reads only the visible window plus a small margin. The file is indexed by line offsets lazily, so opening a large file does not require reading it all upfront.

### Live Regex Highlighting

Regex input is compiled with a short debounce. Matches are highlighted in the visible viewport as you type.

Capture groups are highlighted with distinct colors. When capture groups exist, `oxireg` highlights the captured spans instead of painting the entire outer match, so the signal stays compact.

### Match Map

A background scanner walks the file and builds a match index. It reads line chunks and runs regex matching across worker threads while preserving file order in the UI:

- byte offsets
- line numbers
- unique matching lines
- scan progress

The right edge of the text pane shows a compact match map. Filled markers show where hits are located across the file, similar to editor search indicators.

### Instant Match Navigation

Once matches are indexed, navigation is immediate:

- next match
- previous match
- wrap-around navigation

This works even on very large files because navigation uses the match index instead of scanning from the current position.

### Collapse Non-Matching Lines

Concentration mode hides every line that does not match the current regex. This turns a massive log into a compact event stream while still preserving original line numbers.

### Group Frequency

Named capture groups are counted during background scanning.

For a regex like:

```regex
(?P<status_code>\d{3})
```

`oxireg` builds a live frequency view. You can immediately see distributions such as:

- `404`
- `500`
- `200`
- `302`

The frequency panel renders a donut-style chart using `ratatui` canvas plus a legend with counts.

### Explain Mode

The Explain panel gives a compact human-readable breakdown of the current regex: anchors, groups, character classes, escapes, quantifiers, literals, and common constructs.

### Resizable Panels

The text pane and inspector pane can be resized from the keyboard.

### Mouse Scroll

Mouse wheel scrolling works in the text view.

### Export Matches

Matches can be exported from the current regex into local files:

- `json`
- `csv`
- `txt`

The export scans the current input file line by line and writes one record per match. Named and numbered captures are included in structured formats.

### Regex Flags

Regex flags can be toggled without editing the pattern:

- `i` case-insensitive
- `m` multi-line
- `s` dot matches newline

### Regex History

The current session has in-memory regex history:

- save current regex
- move backward through history
- move forward through history

History is not persisted to disk yet.

## Install

Requires Rust stable with edition 2024 support.

```powershell
cargo build --release
```

The binary will be available at:

```text
target\release\oxireg.exe
```

## Usage

```powershell
cargo run -- path\to\file.log
```

Or after building:

```powershell
target\release\oxireg.exe path\to\file.log
```

Read from stdin:

```powershell
Get-Content .\file.log | target\release\oxireg.exe
```

```bash
cat file.log | oxireg
```

Headless export without opening the TUI:

```powershell
target\release\oxireg.exe --regex "status=(?P<code>\d+)" --export json .\file.log
```

```bash
cat file.log | oxireg --regex 'status=(?P<code>\d+)' --flags i --export csv
```

## Controls

| Key | Action |
| --- | --- |
| `Esc` / `Ctrl-C` | Quit |
| Type text | Edit regex |
| `Left` / `Right` | Move regex cursor |
| `Home` / `End` | Move to regex start/end |
| `Backspace` / `Delete` | Edit regex |
| `Enter` | Save regex to session history |
| `Ctrl-Up` / `Ctrl-Down` | Navigate regex history |
| `Up` / `Down` | Scroll file |
| `PageUp` / `PageDown` | Scroll faster |
| Mouse wheel | Scroll |
| `Ctrl-Home` / `Ctrl-End` | Jump to file start/end |
| `Alt-Left` / `Alt-Right` | Resize text/inspector panes |
| `Alt-E` | Cycle export format |
| `Ctrl-E` | Export matches using the current format |
| `F2` / click Frequency | Collapse/expand frequency panel |
| `F3` / click Status | Collapse/expand status panel |
| `F4` / `Alt-g` | Toggle matching-lines-only mode |
| `F5` / `Alt-i` | Toggle case-insensitive mode |
| `F6` / `Alt-m` | Toggle multi-line mode |
| `F7` / `Alt-s` | Toggle dot-matches-newline mode |
| `F8` / `Alt-n` | Next match |
| `F9` / `Alt-p` | Previous match |


## Performance Model

`oxireg` avoids the usual trap of reading the whole file into a string.

The UI path reads only what it needs for the current viewport. The full-file scan runs separately and reports batches back to the UI thread. Changing the regex cancels the previous scan and starts a new one.

The scanner uses a producer/worker model: the reader splits the file into line chunks, worker threads run regex matching over those chunks, and the UI applies completed chunks in original file order.

This keeps the interface responsive while still allowing full-file features like match maps, jump navigation, and group frequency analysis.

## Current Limitations

- Regex history is session-only and is not saved after restart.
- Multi-line regex matching is compiled, but the file scanner currently processes line by line.
- Frequency analysis is based on named capture groups from indexed matches.
- Paste support depends on terminal bracketed paste behavior.
- Stdin input is spooled to a temporary file before the TUI starts, so viewport seeking and background indexing still work.

## License

MIT
