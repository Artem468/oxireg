mod app;
mod explain;
mod file_view;
mod match_index;
mod regex_debug;
mod terminal;
mod ui;

use std::{
    env,
    fs::File,
    io::{self, IsTerminal, Read, Write},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crossterm::event::{self, Event};

use crate::{app::App, file_view::FileView, terminal::init_terminal};

fn main() -> io::Result<()> {
    let input = match input_source()? {
        Some(input) => input,
        None => {
            eprintln!("Usage: oxireg <file>");
            eprintln!("       cat file.log | oxireg");
            return Ok(());
        }
    };

    let mut terminal = init_terminal()?;
    let result = run(&mut terminal, input.path.clone());
    terminal::restore_terminal(&mut terminal)?;
    if let Some(path) = input.temp_path {
        let _ = std::fs::remove_file(path);
    }
    result
}

struct InputSource {
    path: PathBuf,
    temp_path: Option<PathBuf>,
}

fn input_source() -> io::Result<Option<InputSource>> {
    if let Some(path) = env::args_os().nth(1) {
        return Ok(Some(InputSource {
            path: PathBuf::from(path),
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
