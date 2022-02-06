use std::collections::VecDeque;
use std::io::Write;

use crossterm::event::{Event, KeyCode, KeyModifiers};
use crossterm::{cursor, queue, style, terminal};

#[derive(Default)]
pub struct App {
    name: String,
    input_buffer: String,
    history: VecDeque<HistoryEntry>,
}

impl App {
    pub fn new(name: impl ToString) -> Self {
        App {
            name: name.to_string(),
            ..Default::default()
        }
    }

    pub fn draw<W: Write>(&self, w: &mut W) -> anyhow::Result<()> {
        let (cols, rows) =
            terminal::size().expect("could not determine terminal size");

        queue!(w, cursor::Hide)?;

        let lines = self
            .history
            .iter()
            .rev()
            .flat_map(|entry| {
                let (color, prefix, contents) = match entry {
                    HistoryEntry::Message { nick, contents } => {
                        (style::Color::White, nick.as_str(), contents.as_str())
                    }
                    HistoryEntry::Info { message } => {
                        (style::Color::DarkGrey, "INFO", message.as_str())
                    }
                };

                wrap(prefix, contents, cols)
                    .into_iter()
                    .rev()
                    .map(move |line| (color, line))
            })
            .take((rows - 2).into());

        for (idx, (color, line)) in lines.enumerate() {
            let idx: u16 = idx.try_into().unwrap();
            queue!(
                w,
                cursor::MoveTo(0, rows - 3 - idx),
                style::SetForegroundColor(color),
                style::Print(line),
                terminal::Clear(terminal::ClearType::UntilNewLine)
            )?;
        }

        queue!(w, style::ResetColor)?;

        queue!(
            w,
            cursor::MoveTo(0, rows - 2),
            style::SetBackgroundColor(style::Color::DarkGrey),
            style::Print("-".repeat(cols.into())),
            style::ResetColor,
        )?;

        queue!(
            w,
            cursor::MoveTo(0, rows - 1),
            style::Print(format!("{}: ", self.name)),
            style::Print(&self.input_buffer),
            cursor::Show,
            terminal::Clear(terminal::ClearType::UntilNewLine)
        )?;

        w.flush()?;

        Ok(())
    }

    pub fn handle_event(&mut self, event: Event) -> Option<AppEvent> {
        // TODO use a readline library
        if let Event::Key(event) = event {
            match event.code {
                KeyCode::Char('c')
                    if event.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    return Some(AppEvent::Quit)
                }
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

    pub fn push_message(
        &mut self,
        nick: impl Into<String>,
        contents: impl Into<String>,
    ) {
        self.history.push_back(HistoryEntry::Message {
            nick: nick.into(),
            contents: contents.into(),
        });
    }

    pub fn push_info(&mut self, message: impl Into<String>) {
        self.history.push_back(HistoryEntry::Info {
            message: message.into(),
        });
    }
}

#[derive(Clone, Debug)]
pub enum AppEvent {
    SendMessage(String),
    Quit,
}

#[derive(Clone, Debug)]
enum HistoryEntry {
    Message { nick: String, contents: String },
    Info { message: String },
}

fn wrap<'a>(prefix: &str, message: &str, columns: u16) -> Vec<String> {
    let indent = " ".repeat(prefix.len() + 2);
    let options =
        textwrap::Options::new(columns.into()).subsequent_indent(&indent);
    let wrapped = textwrap::fill(&format!("{prefix}: {message}"), options);
    wrapped.lines().map(|s| s.to_owned()).collect()
}
