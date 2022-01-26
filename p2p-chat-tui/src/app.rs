use std::io::Write;

use crossterm::event::{Event, KeyCode};
use crossterm::{cursor, execute, style, terminal};

#[derive(Default)]
pub struct App {
    input_buffer: String,
    history: Vec<String>,
}

impl App {
    pub fn draw<W: Write>(&self, w: &mut W) -> anyhow::Result<()> {
        let (_cols, rows) =
            terminal::size().expect("could not determine terminal size");

        execute!(w, terminal::Clear(terminal::ClearType::All))?;

        for (idx, line) in self.history.iter().rev().enumerate() {
            let row = rows - 2 - idx as u16;
            execute!(w, cursor::MoveTo(0, row), style::Print(line))?;
        }

        execute!(
            w,
            cursor::MoveTo(0, rows - 1),
            style::Print("> "),
            style::Print(&self.input_buffer)
        )?;

        Ok(())
    }

    pub fn handle_event(&mut self, event: Event) -> Option<AppEvent> {
        // TODO use a readline library
        if let Event::Key(event) = event {
            match event.code {
                KeyCode::Esc => return Some(AppEvent::Quit),
                KeyCode::Char(c @ ' '..='~') => {
                    self.input_buffer.push(c);
                }
                KeyCode::Backspace => {
                    self.input_buffer.pop();
                }
                KeyCode::Enter => {
                    let event =
                        Some(AppEvent::SendMessage(self.input_buffer.clone()));
                    self.input_buffer.clear();
                    return event;
                }
                _ => {}
            }
        }

        None
    }

    pub fn push_history(&mut self, message: impl Into<String>) {
        self.history.push(message.into());
    }
}

#[derive(Clone, Debug)]
pub enum AppEvent {
    SendMessage(String),
    Quit,
}
