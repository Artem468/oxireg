mod app;
mod explain;
mod export;
mod file_view;
mod match_index;
mod regex_debug;
mod terminal;
mod ui;

use std::{
    env,
    ffi::OsString,
    fs::File,
    io::{self, IsTerminal, Read, Write},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crossterm::event::{self, Event};

use crate::{
    app::App,
    export::{ExportFormat, export_matches},
    file_view::FileView,
    regex_debug::{RegexFlags, compile_regex},
    terminal::init_terminal,
};

fn main() -> io::Result<()> {
    let options = match CliOptions::parse(env::args_os().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            print_usage();
            return Ok(());
        }
    };

    let input = match input_source(options.path.clone())? {
        Some(input) => input,
        None => {
            print_usage();
            return Ok(());
        }
    };

    if let Some(format) = options.export {
        let result = run_export(&input.path, &options, format);
        if let Some(path) = input.temp_path {
            let _ = std::fs::remove_file(path);
        }
        return result;
    }

    let mut terminal = init_terminal()?;
    let result = run(&mut terminal, input.path.clone());
    terminal::restore_terminal(&mut terminal)?;
    if let Some(path) = input.temp_path {
        let _ = std::fs::remove_file(path);
    }
    result
}

#[derive(Default)]
struct CliOptions {
    path: Option<PathBuf>,
    regex: Option<String>,
    flags: RegexFlags,
    export: Option<ExportFormat>,
}

impl CliOptions {
    fn parse(args: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let mut options = Self::default();
        let mut positional = false;
        let mut args = args.peekable();

        while let Some(arg) = args.next() {
            if !positional && arg == "--" {
                positional = true;
                continue;
            }

            if !positional && arg == "--regex" {
                let Some(value) = args.next() else {
                    return Err("--regex requires a pattern".to_string());
                };
                options.regex = Some(value.to_string_lossy().into_owned());
                continue;
            }

            if !positional && arg == "--flags" {
                let Some(value) = args.next() else {
                    return Err("--flags requires a value like ims".to_string());
                };
                options.flags = parse_flags(&value.to_string_lossy())?;
                continue;
            }

            if !positional && arg == "--export" {
                let Some(value) = args.next() else {
                    return Err("--export requires json, csv, or txt".to_string());
                };
                let value = value.to_string_lossy();
                options.export = Some(
                    ExportFormat::parse(&value)
                        .ok_or_else(|| format!("unsupported export format: {value}"))?,
                );
                continue;
            }

            if !positional && arg.to_string_lossy().starts_with("--") {
                return Err(format!("unknown option: {}", arg.to_string_lossy()));
            }

            if options.path.is_some() {
                return Err(format!(
                    "unexpected extra argument: {}",
                    arg.to_string_lossy()
                ));
            }
            options.path = Some(PathBuf::from(arg));
        }

        Ok(options)
    }
}

fn parse_flags(value: &str) -> Result<RegexFlags, String> {
    let mut flags = RegexFlags::default();
    for ch in value.chars() {
        match ch {
            'i' => flags.case_insensitive = true,
            'm' => flags.multi_line = true,
            's' => flags.dot_matches_new_line = true,
            ch => return Err(format!("unsupported regex flag: {ch}")),
        }
    }
    Ok(flags)
}

fn run_export(path: &PathBuf, options: &CliOptions, format: ExportFormat) -> io::Result<()> {
    let Some(pattern) = options.regex.as_deref() else {
        eprintln!("--export requires --regex");
        print_usage();
        return Ok(());
    };

    let regex = match compile_regex(pattern, options.flags) {
        Ok(regex) => regex,
        Err(err) => {
            eprintln!("invalid regex: {err}");
            return Ok(());
        }
    };

    let result = export_matches(path, &regex, format)?;
    println!(
        "exported {} matches to {}",
        result.matches,
        result.path.display()
    );
    Ok(())
}

fn print_usage() {
    eprintln!("Usage: oxireg [options] <file>");
    eprintln!("       cat file.log | oxireg [options]");
    eprintln!("Options:");
    eprintln!("       --regex <pattern>       Regex for headless export");
    eprintln!("       --flags <ims>           Enable regex flags");
    eprintln!("       --export <json|csv|txt> Export immediately instead of opening TUI");
}

struct InputSource {
    path: PathBuf,
    temp_path: Option<PathBuf>,
}

fn input_source(path: Option<PathBuf>) -> io::Result<Option<InputSource>> {
    if let Some(path) = path {
        return Ok(Some(InputSource {
            path,
            temp_path: None,
        }));
    }

    if io::stdin().is_terminal() {
        return Ok(None);
    }

    let path = stdin_temp_path();
    let mut file = File::create(&path)?;
    let mut stdin = io::stdin().lock();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = stdin.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])?;
    }
    file.flush()?;

    Ok(Some(InputSource {
        path: path.clone(),
        temp_path: Some(path),
    }))
}

fn stdin_temp_path() -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    env::temp_dir().join(format!("oxireg-stdin-{}-{now}.log", std::process::id()))
}

fn run(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
    path: PathBuf,
) -> io::Result<()> {
    let file = FileView::open(path)?;
    let mut app = App::new(file);

    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) => app.handle_key(key)?,
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                Event::Paste(text) => app.handle_paste(&text),
                _ => {}
            }
        }

        app.drain_match_index();
        app.maybe_compile_regex();
    }

    Ok(())
}
