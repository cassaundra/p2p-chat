use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyCode, KeyModifiers};
use crossterm::{cursor, queue, style, terminal};
use futures::stream::Fuse;
use futures::StreamExt;
use futures_timer::Delay;
use libp2p::gossipsub::error::PublishError;
use libp2p::PeerId;
use tokio::select;

use p2p_chat::protocol::{ChannelIdentifier, MessageType};
use p2p_chat::{Client, ClientEvent, Error};

pub struct App {
    client: Fuse<Client>,
    input_buffer: String,
    current_channel: Option<ChannelIdentifier>,
    channel_histories: HashMap<ChannelIdentifier, VecDeque<HistoryEntry>>,
    system_history: VecDeque<String>,
    wants_to_quit: bool,
}

impl App {
    pub fn new(client: Client) -> Self {
        App {
            client: client.fuse(),
            input_buffer: String::with_capacity(64),
            current_channel: None,
            channel_histories: HashMap::new(),
            system_history: VecDeque::new(),
            wants_to_quit: false,
        }
    }

    /// Run the app, blocking until it finishes executing.
    pub async fn run<W: Write>(
        &mut self,
        writer: &mut W,
    ) -> anyhow::Result<()> {
        let mut term_events = EventStream::new().fuse();

        // do an initial draw
        self.draw(writer, false)?;

        while !self.wants_to_quit {
            // redraw every two seconds just in case
            let tick = Delay::new(Duration::from_secs(5));

            select! {
                _ = tick => {
                    self.draw(writer, false)?;
                }
                Some(event) = self.client.select_next_some() => {
                    match event {
                        ClientEvent::Message { contents, channel, timestamp: _, message_type, source } => {
                            self.push_message(source, contents, channel, message_type);
                        }
                        ClientEvent::PeerConnected(peer_id) => {
                            self.push_system(format!("peer connected: {peer_id}"));
                        }
                        ClientEvent::PeerDisconnected(peer_id) => {
                            self.push_system(format!("peer disconnected: {peer_id}"));
                        }
                        ClientEvent::Dialing(peer_id) => {
                            self.push_system(format!("dialing: {peer_id}"));
                        }
                        ClientEvent::OutgoingConnectionError {
                            peer_id: _,
                            error: _,
                        } => {
                            self.push_system("failed to connect to peer");
                        }
                        _ => {}
                    }
                    self.draw(writer, false)?;
                }
                event = term_events.select_next_some() => {
                    let event = event?;

                    self.handle_event(event)?;

                    let should_clear = matches!(event, Event::Resize(_, _));
                    self.draw(writer, should_clear)?;
                }
            }
        }
        Ok(())
    }

    fn draw<W: Write>(
        &mut self,
        writer: &mut W,
        should_clear: bool,
    ) -> anyhow::Result<()> {
        let (cols, rows) =
            terminal::size().expect("could not determine terminal size");

        queue!(writer, cursor::Hide)?;

        if should_clear {
            queue!(writer, terminal::Clear(terminal::ClearType::All))?;
        }

        let lines: Vec<_> = if let Some(channel) = &self.current_channel {
            self.channel_histories
                .entry(channel.clone())
                .or_default()
                .iter()
                .rev()
                .map(|entry| match entry {
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
                                sender.to_base58().chars().take(16).collect()
                            }
                        };
                        (style::Color::White, nick, contents)
                    }
                })
                .collect()
        } else {
            self.system_history
                .iter()
                .rev()
                .map(|entry| (style::Color::White, "INFO".to_owned(), entry))
                .collect()
        };

        let lines = lines
            .iter()
            .flat_map(|(color, prefix, contents)| {
                wrap(prefix, contents, cols)
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
                style::SetForegroundColor(*color),
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
        )?;

        queue!(writer, cursor::MoveTo(0, rows - 2),)?;
        match &self.current_channel {
            Some(channel) => {
                queue!(
                    writer,
                    style::Print("["),
                    style::Print(channel),
                    style::Print("]")
                )?;
            }
            None => {
                queue!(writer, style::Print("*system*"))?;
            }
        }
        queue!(writer, style::ResetColor)?;

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

    fn handle_event(&mut self, event: Event) -> anyhow::Result<()> {
        // TODO use a readline library
        if let Event::Key(event) = event {
            match event.code {
                KeyCode::Char('c' | 'd')
                    if event.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    self.wants_to_quit = true;
                }
                KeyCode::Char(c) => {
                    self.input_buffer.push(c);
                }
                KeyCode::Backspace => {
                    self.input_buffer.pop();
                }
                KeyCode::Enter => {
                    let message = self.input_buffer.clone();
                    self.input_buffer.clear();

                    if let Some(command) = message.strip_prefix('/') {
                        self.run_command(command)?;
                    } else {
                        self.send_message(message);
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn run_command(&mut self, command: &str) -> anyhow::Result<()> {
        let args = command.split(char::is_whitespace).collect::<Vec<_>>();

        match *args.as_slice() {
            ["join", channel] => {
                self.client.get_mut().join_channel(channel.to_owned())?;
                self.push_system(format!("Joined channel {channel}"));
            }
            ["leave", channel] => {
                self.client.get_mut().join_channel(channel.to_owned())?;
                self.push_system(format!("Left channel {channel}"));
            }
            ["go"] => {
                self.current_channel = None;
            }
            ["go", channel] => {
                self.current_channel = Some(channel.to_owned());
            }
            ["list"] => {
                let channels = self.client.get_ref().channels().join(", ");
                self.push_system(format!(
                    "Channels you have joined: {}",
                    channels
                ));
            }
            _ => self.push_system("Invalid command"),
        }

        Ok(())
    }

    fn send_message(&mut self, message: String) {
        if let Some(channel) = &self.current_channel {
            match self.client.get_mut().send_message(
                &message,
                MessageType::Normal,
                channel.clone(),
            ) {
                Err(Error::PublishError(PublishError::InsufficientPeers)) => {
                    self.push_system(
                        "could not send message, insufficient peers",
                    );
                }
                Err(err) => self.push_system(format!("{err:?}")),
                Ok(_) => {
                    let channel = channel.clone();
                    self.push_message(
                        self.client.get_ref().peer_id(),
                        message,
                        channel,
                        MessageType::Normal,
                    )
                }
            }
        } else {
            self.push_system("You are not in a channel.");
        }
    }

    fn push_message(
        &mut self,
        sender: PeerId,
        contents: impl Into<String>,
        channel: ChannelIdentifier,
        message_type: MessageType,
    ) {
        self.channel_histories
            .entry(channel)
            .or_default()
            .push_back(HistoryEntry::Message {
                sender,
                contents: contents.into(),
                message_type,
            });
    }

    fn push_system(&mut self, message: impl Into<String>) {
        self.system_history.push_back(message.into());
    }
}

#[derive(Clone, Debug)]
enum HistoryEntry {
    Message {
        sender: PeerId,
        contents: String,
        message_type: MessageType,
    },
}

fn wrap(prefix: &str, message: &str, columns: u16) -> Vec<String> {
    let indent = " ".repeat(prefix.len() + 2);
    let options =
        textwrap::Options::new(columns.into()).subsequent_indent(&indent);
    let wrapped = textwrap::fill(&format!("{prefix}: {message}"), options);
    wrapped.lines().map(|s| s.to_owned()).collect()
}
