use std::collections::VecDeque;
use std::io::Write;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyCode, KeyModifiers};
use crossterm::{cursor, queue, style, terminal};
use futures::stream::Fuse;
use futures::StreamExt;
use futures_timer::Delay;
use libp2p::gossipsub::error::PublishError;
use libp2p::PeerId;
use p2p_chat::protocol::MessageType;
use tokio::select;

use p2p_chat::{Client, ClientEvent, Error};

pub struct App {
    client: Fuse<Client>,
    input_buffer: String,
    history: VecDeque<HistoryEntry>,
}

impl App {
    pub fn new(client: Client) -> Self {
        App {
            client: client.fuse(),
            input_buffer: String::with_capacity(64),
            history: VecDeque::new(),
        }
    }

    pub async fn run<W: Write>(
        &mut self,
        writer: &mut W,
    ) -> anyhow::Result<()> {
        let mut term_events = EventStream::new().fuse();

        loop {
            let tick = Delay::new(Duration::from_millis(1000 / 20));

            select! {
                _ = tick => {
                    self.draw(writer)?;
                }
                Some(event) = self.client.select_next_some() => {
                    match event {
                        ClientEvent::Message { contents, timestamp: _, message_type, source } => {
                            self.push_message(source, contents, message_type);
                        }
                        ClientEvent::PeerConnected(peer_id) => {
                            self.push_info(format!("peer connected: {peer_id}"));
                        }
                        ClientEvent::PeerDisconnected(peer_id) => {
                            self.push_info(format!("peer disconnected: {peer_id}"));
                        }
                        ClientEvent::Dialing(peer_id) => {
                            self.push_info(format!("dialing: {peer_id}"));
                        }
                        ClientEvent::OutgoingConnectionError {
                            peer_id: _,
                            error: _,
                        } => {
                            self.push_info(format!("failed to connect to peer"));
                        }
                        _ => {}
                    }
                    self.draw(writer)?;
                }
                event = term_events.select_next_some() => {
                    // TODO handle multiple messages at a time, mostly for copy/paste
                    if let Some(event) = self.handle_event(event?) {
                        match event {
                            AppEvent::SendMessage(message) => {
                                match self.client.get_mut().send_message(&message, MessageType::Normal) {
                                    Err(Error::PublishError(PublishError::InsufficientPeers)) => {
                                        self.push_info("could not send message, insufficient peers");
                                    }
                                    Err(err) => self.push_info(format!("{err:?}")),
                                    Ok(_) => {
                                        self.push_message(self.client.get_ref().peer_id(), message, MessageType::Normal)
                                    },
                                }
                            },
                            AppEvent::Quit => break,
                        }
                    }
                    self.draw(writer)?;
                }
            }
        }
        Ok(())
    }

    fn draw<W: Write>(&mut self, writer: &mut W) -> anyhow::Result<()> {
        let (cols, rows) =
            terminal::size().expect("could not determine terminal size");

        queue!(writer, cursor::Hide)?;

        let lines = self
            .history
            .iter()
            .rev()
            .flat_map(|entry| {
                let (color, prefix, contents) = match entry {
                    HistoryEntry::Message {
                        sender,
                        contents,
                        message_type: _,
                    } => {
                        let nick = match self
                            .client
                            .get_mut()
                            .fetch_nickname(sender)
                            .unwrap()
                        {
                            Some(nick) => nick.to_owned(),
                            None => {
                                sender.to_base58().chars().take(12).collect()
                            }
                        };
                        (style::Color::White, nick, contents.as_str())
                    }
                    HistoryEntry::Info { message } => (
                        style::Color::DarkGrey,
                        "INFO".to_owned(),
                        message.as_str(),
                    ),
                };

                wrap(&prefix, contents, cols)
                    .into_iter()
                    .rev()
                    .map(move |line| (color, line))
            })
            .take((rows - 2).into());

        for (idx, (color, line)) in lines.enumerate() {
            let idx: u16 = idx.try_into().unwrap();
            queue!(
                writer,
                cursor::MoveTo(0, rows - 3 - idx),
                style::SetForegroundColor(color),
                style::Print(line),
                terminal::Clear(terminal::ClearType::UntilNewLine)
            )?;
        }

        queue!(writer, style::ResetColor)?;

        queue!(
            writer,
            cursor::MoveTo(0, rows - 2),
            style::SetBackgroundColor(style::Color::DarkGrey),
            style::Print("-".repeat(cols.into())),
            style::ResetColor,
        )?;

        let nick = self.client.get_ref().nick().clone();
        queue!(
            writer,
            cursor::MoveTo(0, rows - 1),
            style::Print(format!("{}: ", nick)),
            style::Print(&self.input_buffer),
            cursor::Show,
            terminal::Clear(terminal::ClearType::UntilNewLine)
        )?;

        writer.flush()?;

        Ok(())
    }

    fn handle_event(&mut self, event: Event) -> Option<AppEvent> {
        // TODO use a readline library
        if let Event::Key(event) = event {
            match event.code {
                KeyCode::Char('c')
                    if event.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    return Some(AppEvent::Quit)
                }
                KeyCode::Char(c) => {
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

    fn push_message(
        &mut self,
        sender: PeerId,
        contents: impl Into<String>,
        message_type: MessageType,
    ) {
        self.history.push_back(HistoryEntry::Message {
            sender,
            contents: contents.into(),
            message_type,
        });
    }

    fn push_info(&mut self, message: impl Into<String>) {
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
    Message {
        sender: PeerId,
        contents: String,
        message_type: MessageType,
    },
    Info {
        message: String,
    },
}

fn wrap(prefix: &str, message: &str, columns: u16) -> Vec<String> {
    let indent = " ".repeat(prefix.len() + 2);
    let options =
        textwrap::Options::new(columns.into()).subsequent_indent(&indent);
    let wrapped = textwrap::fill(&format!("{prefix}: {message}"), options);
    wrapped.lines().map(|s| s.to_owned()).collect()
}
