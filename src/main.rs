mod app;
mod file_view;
mod match_index;
mod regex_debug;
mod terminal;
mod ui;

use std::{env, io, path::PathBuf, time::Duration};

use crossterm::event::{self, Event};

use crate::{app::App, file_view::FileView, terminal::init_terminal};

fn main() -> io::Result<()> {
    let path = match env::args_os().nth(1) {
        Some(path) => PathBuf::from(path),
        None => {
            eprintln!("Usage: oxireg <file>");
            return Ok(());
        }
    };

    let mut terminal = init_terminal()?;
    let result = run(&mut terminal, path);
    terminal::restore_terminal(&mut terminal)?;
    result
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
