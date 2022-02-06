use std::env;
use std::time::Duration;
use std::{io, net::SocketAddr};

use crossterm::{event::EventStream, execute, style, terminal};
use futures::StreamExt;
use futures_timer::Delay;
use libp2p::gossipsub::error::PublishError;
use libp2p::{multiaddr::multiaddr, Multiaddr};
use p2p_chat::{gen_id_keys, Client, ClientEvent, Error};
use structopt::StructOpt;
use tokio::select;

pub mod app;
use app::App;

use crate::app::AppEvent;

#[derive(StructOpt)]
#[structopt(name = "p2p-chat-tui")]
struct Opt {
    /// Nickname.
    #[structopt(short, long)]
    nick: Option<String>,
    /// Port to listen on.
    #[structopt(short, long)]
    port: Option<u16>,
    /// Peers to dial, separated by commas.
    #[structopt(short, long, parse(try_from_str = parse_multiaddrs))]
    dial: Vec<Multiaddr>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opts = Opt::from_args();

    // setup logger
    setup_logger(log::LevelFilter::Info)?;

    // start client
    let nick = opts
        .nick
        .or_else(|| env::var("USER").ok())
        .unwrap_or_else(|| "user".to_owned());
    let id_keys = gen_id_keys();
    let mut client = Client::new(&nick, id_keys).await?.fuse();

    let port = opts.port.unwrap_or_default();
    client
        .get_mut()
        .listen_on(multiaddr!(Ip4([0, 0, 0, 0]), Tcp(port)))?;
    for addr in opts.dial {
        client.get_mut().dial(addr)?;
    }

    // setup tui

    let mut stdout = io::stdout();

    execute!(stdout, terminal::EnterAlternateScreen)?;
    terminal::enable_raw_mode()?;

    let mut app = App::new(&nick);

    let mut term_events = EventStream::new().fuse();

    loop {
        let tick = Delay::new(Duration::from_millis(1000 / 20));

        select! {
            _ = tick => {
                app.draw(&mut stdout)?;
            }
            Some(event) = client.select_next_some() => {
                match event {
                    ClientEvent::Message { contents, nick, timestamp: _, source: _ } => {
                        app.push_message(nick, contents);
                    }
                    ClientEvent::PeerConnected(peer_id) => {
                        app.push_info(format!("peer connected: {peer_id}"));
                    }
                    ClientEvent::PeerDisconnected(peer_id) => {
                        app.push_info(format!("peer disconnected: {peer_id}"));
                    }
                    ClientEvent::Dialing(peer_id) => {
                        app.push_info(format!("dialing: {peer_id}"));
                    }
                    ClientEvent::OutgoingConnectionError {
                        peer_id: _,
                        error: _,
                    } => {
                        app.push_info(format!("failed to connect to peer"));
                    }
                    _ => {}
                }
                app.draw(&mut stdout)?;
            }
            event = term_events.select_next_some() => {
                // TODO handle multiple messages at a time, mostly for copy/paste
                if let Some(event) = app.handle_event(event?) {
                    match event {
                        AppEvent::SendMessage(message) => {
                            match client.get_mut().send_message(&message) {
                                Err(Error::PublishError(PublishError::InsufficientPeers)) => {
                                    app.push_info("could not send message, insufficient peers");
                                }
                                Err(err) => app.push_info(format!("{err:?}")),
                                Ok(_) => app.push_message(&nick, message),
                            }
                        },
                        AppEvent::Quit => break,
                    }
                }
                app.draw(&mut stdout)?;
            }
        }
    }

    terminal::disable_raw_mode()?;
    execute!(
        io::stdout(),
        style::ResetColor,
        terminal::LeaveAlternateScreen
    )?;

    Ok(())
}

fn setup_logger(level: log::LevelFilter) -> anyhow::Result<()> {
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{}][{}][{}] {}",
                chrono::Local::now()
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                record.target(),
                record.level(),
                message
            ))
        })
        .level(level)
        // .chain(std::io::stdout())
        .chain(fern::log_file("current.log")?)
        .apply()?;
    Ok(())
}

fn parse_multiaddrs(s: &str) -> anyhow::Result<Multiaddr> {
    Ok(match s.parse::<SocketAddr>()? {
        SocketAddr::V4(addr) => multiaddr!(Ip4(*addr.ip()), Tcp(addr.port())),
        SocketAddr::V6(addr) => multiaddr!(Ip6(*addr.ip()), Tcp(addr.port())),
    })
}
