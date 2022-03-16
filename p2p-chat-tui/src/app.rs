use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Write;
use std::rc::Rc;
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

#[derive(Clone, Debug)]
struct Buffer {
    /// The messages that have been sent to this buffer, sorted chronologically.
    history: VecDeque<HistoryEntry>,
    /// Whether there are any messages in the buffer that have not been read by
    /// the user.
    has_unread: bool,
    /// The type of buffer this is (system or channel).
    buffer_type: BufferType,
}

impl Buffer {
    pub fn new(buffer_type: BufferType) -> Self {
        Buffer {
            history: VecDeque::new(),
            has_unread: false,
            buffer_type,
        }
    }

    pub fn name(&self) -> &str {
        match &self.buffer_type {
            BufferType::System => "*system*",
            BufferType::Channel(ident) => ident,
        }
    }
}

#[derive(Clone, Debug)]
enum BufferType {
    /// A buffer in which system-wide messages are read.
    System,
    /// A channel in which users communicate with one another.
    Channel(ChannelIdentifier),
}

#[derive(Clone, Debug)]
enum HistoryEntry {
    Message {
        sender: PeerId,
        contents: String,
        message_type: MessageType,
    },
    Log(String),
}

/// A TUI implementation of p2p-chat.
pub struct App {
    /// The underlying client which this app represents.
    client: Fuse<Client>,
    /// The message/command input buffer.
    input_buffer: String,
    /// The list of currently open buffers (system or channels). In practice,
    /// this should consist of a single system buffer, followed by an arbitrary
    /// number of channels.
    buffers: Vec<Rc<RefCell<Buffer>>>,
    /// The buffer the user currently has open,
    current_buffer: Rc<RefCell<Buffer>>,
    /// The primary system buffer.
    system_buffer: Rc<RefCell<Buffer>>,
    /// The index of the currently focused buffer.
    /// Whether or not the user has requested to exit the program.
    wants_to_exit: bool,
}

impl App {
    pub fn new(client: Client) -> Self {
        let system_buffer =
            Rc::new(RefCell::new(Buffer::new(BufferType::System)));

        App {
            client: client.fuse(),
            input_buffer: String::with_capacity(256),
            buffers: vec![system_buffer.clone()],
            current_buffer: system_buffer.clone(),
            system_buffer,
            wants_to_exit: false,
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

        while !self.wants_to_exit {
            // redraw every once in a while just in case
            let redraw_tick = Delay::new(Duration::from_secs(10));

            select! {
                _ = redraw_tick => {
                    self.draw(writer, true)?;
                }
                Some(event) = self.client.select_next_some() => {
                    match event {
                        ClientEvent::Message { contents, channel, timestamp: _, message_type, sender } => {
                            self.push_channel_message(sender, contents, &channel, message_type);
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

                    let should_clear = self.handle_event(event)? || matches!(event, Event::Resize(_, _));
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
        let size = terminal::size().expect("could not determine terminal size");

        queue!(writer, cursor::Hide)?;

        if should_clear {
            queue!(writer, terminal::Clear(terminal::ClearType::All))?;
        }

        self.draw_current_buffer(writer, size)?;
        self.draw_status_line(writer, size)?;
        self.draw_input_buffer(writer, size)?;

        writer.flush()?;

        Ok(())
    }

    fn draw_current_buffer<W: Write>(
        &mut self,
        writer: &mut W,
        (cols, rows): (u16, u16),
    ) -> anyhow::Result<()> {
        let buffer = self.current_buffer.borrow_mut();

        let lines = buffer
            .history
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
                        None => sender
                            .to_base58()
                            .chars()
                            .skip(16)
                            .take(16)
                            .collect(),
                    };
                    (style::Color::White, nick, contents)
                }
                HistoryEntry::Log(message) => {
                    (style::Color::White, "INFO".to_owned(), message)
                }
            })
            .flat_map(|(color, prefix, contents)| {
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

        Ok(())
    }

    fn draw_status_line<W: Write>(
        &mut self,
        writer: &mut W,
        (cols, rows): (u16, u16),
    ) -> anyhow::Result<()> {
        queue!(
            writer,
            cursor::MoveTo(0, rows - 2),
            style::SetBackgroundColor(style::Color::DarkGrey),
            style::Print("-".repeat(cols.into())),
        )?;

        queue!(writer, cursor::MoveTo(0, rows - 2),)?;
        match &self.current_buffer.borrow().buffer_type {
            BufferType::Channel(ident) => {
                queue!(
                    writer,
                    style::Print("["),
                    style::Print(ident),
                    style::Print("]")
                )?;
            }
            BufferType::System => {
                queue!(writer, style::Print("*system*"))?;
            }
        }

        // TODO show rest of channels

        queue!(writer, style::ResetColor)?;

        Ok(())
    }

    fn draw_input_buffer<W: Write>(
        &mut self,
        writer: &mut W,
        (cols, rows): (u16, u16),
    ) -> anyhow::Result<()> {
        let nick = self.client.get_ref().nick().clone();

        // the amount of space we have to render the input buffer
        // takes into account the nickname and separator
        let input_space = cols as isize - nick.len() as isize - 3;

        // how many characters we will skip at the beginning of the message
        let skip = isize::max(0, self.input_buffer.len() as isize - input_space) as usize;

        queue!(
            writer,
            cursor::MoveTo(0, rows - 1),
            style::Print(format!("{}: ", nick)),
            style::Print(
                &self
                    .input_buffer
                    .chars()
                    .skip(skip)
                    .collect::<String>()
            ),
            cursor::Show,
            terminal::Clear(terminal::ClearType::UntilNewLine)
        )?;

        Ok(())
    }

    fn handle_event(&mut self, event: Event) -> anyhow::Result<bool> {
        // TODO use a readline library
        if let Event::Key(event) = event {
            match event.code {
                KeyCode::Char('c' | 'd')
                    if event.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    self.wants_to_exit = true;
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
                        return Ok(true);
                    } else {
                        self.send_message(message);
                    }
                }
                _ => {}
            }
        }

        Ok(false)
    }

    fn run_command(&mut self, command: &str) -> anyhow::Result<()> {
        let args = command.split(char::is_whitespace).collect::<Vec<_>>();

        match *args.as_slice() {
            ["join", channel] => {
                self.client
                    .get_mut()
                    .subscribe_channel(channel.to_owned())?;
                self.buffers.push(Rc::new(RefCell::new(Buffer::new(
                    BufferType::Channel(channel.to_owned()),
                ))));
                self.push_system(format!("Joined channel {channel}"));
            }
            ["leave", channel] => {
                self.client
                    .get_mut()
                    .unsubscribe_channel(channel.to_owned())?;
                self.buffers.retain(|b| matches!(&b.borrow().buffer_type, BufferType::Channel(ident) if ident != channel) );
                self.push_system(format!("Left channel {channel}"));
            }
            ["go"] => {
                self.current_buffer = self.buffers.first().unwrap().clone();
            }
            ["go", channel] => {
                if let Some(buffer) = self.channel_by_ident(channel) {
                    self.current_buffer = buffer.clone();
                }
            }
            ["list"] => {
                self.push_system("Channels you are in:");
                for buffer in &self.buffers {
                    self.push_system(format!("- {}", buffer.borrow().name()));
                }
            }
            _ => self.push_system("Invalid command"),
        }

        Ok(())
    }

    fn send_message(&mut self, message: String) {
        let buffer_type = self.current_buffer.borrow().buffer_type.clone();
        if let BufferType::Channel(channel) = &buffer_type {
            match self.client.get_mut().send_message(
                &message,
                MessageType::Normal,
                channel.clone(),
            ) {
                Err(Error::PublishError(PublishError::InsufficientPeers)) => {
                    self.push_channel_log(
                        "Could not send message: insufficient peers.",
                    );
                }
                Err(err) => self.push_system(format!("{err:?}")),
                Ok(_) => {
                    let channel = channel.clone();
                    self.push_channel_message(
                        self.client.get_ref().peer_id(),
                        message,
                        &channel,
                        MessageType::Normal,
                    )
                }
            }
        } else {
            self.push_system("You are not in a channel.");
        }
    }

    fn channel_by_ident(&self, channel: &str) -> Option<&Rc<RefCell<Buffer>>> {
        self.buffers
            .iter()
            .find(|b| {
                matches!(
                    &b.borrow().buffer_type,
                    BufferType::Channel(c) if c == channel
                )
            })
    }

    fn push_channel_message(
        &mut self,
        sender: PeerId,
        contents: impl Into<String>,
        channel: &ChannelIdentifier,
        message_type: MessageType,
    ) {
        self.channel_by_ident(channel)
            .unwrap() // TODO
            .borrow_mut()
            .history
            .push_back(HistoryEntry::Message {
                sender,
                contents: contents.into(),
                message_type,
            });
    }

    fn push_channel_log(&self, contents: impl Into<String>) {
        self.current_buffer
            .borrow_mut()
            .history
            .push_back(HistoryEntry::Log(contents.into()));
    }

    fn push_system(&self, message: impl Into<String>) {
        self.system_buffer
            .borrow_mut()
            .history
            .push_back(HistoryEntry::Log(message.into()));
    }
}

fn wrap(prefix: &str, message: &str, columns: u16) -> Vec<String> {
    let indent = " ".repeat(prefix.len() + 2);
    let options =
        textwrap::Options::new(columns.into()).subsequent_indent(&indent);
    let wrapped = textwrap::fill(&format!("{prefix}: {message}"), options);
    wrapped.lines().map(|s| s.to_owned()).collect()
}
