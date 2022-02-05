use std::time::Duration;
use std::{io, net::SocketAddr};

use crossterm::{event::EventStream, execute, style, terminal};
use futures::StreamExt;
use futures_timer::Delay;
use libp2p::{multiaddr::multiaddr, Multiaddr};
use p2p_chat::{gen_id_keys, name_from_peer, Client, ClientEvent};
use structopt::StructOpt;
use tokio::select;

pub mod app;
use app::App;

use crate::app::AppEvent;

#[derive(StructOpt)]
#[structopt(name = "p2p-chat-tui")]
struct Opt {
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
    let id_keys = gen_id_keys();
    let mut client = Client::new(id_keys).await?.fuse();

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

    let name = name_from_peer(client.get_ref().peer_id());
    let mut app = App::new(name);

    let mut term_events = EventStream::new().fuse();

    loop {
        let tick = Delay::new(Duration::from_millis(1000 / 20));

        select! {
            _ = tick => {
                app.draw(&mut stdout)?;
            }
            Some(event) = client.select_next_some() => {
                if let ClientEvent::Message { contents, source } = event {
                    let peer_name = name_from_peer(source);
                    app.push_history(format!("{peer_name}: {contents}"));
                }
            }
            event = term_events.select_next_some() => {
                // TODO handle error?
                if let Some(event) = app.handle_event(event?) {
                    match event {
                        AppEvent::SendMessage(message) => {
                            app.push_history(format!("{name}: {message}"));
                            client.get_mut().send_message(&message)?
                        },
                        AppEvent::Quit => break,
                    }
                }
            }
        }
    }

    execute!(
        io::stdout(),
        style::ResetColor,
        terminal::LeaveAlternateScreen
    )?;
    terminal::disable_raw_mode()?;

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
