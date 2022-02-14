use std::env;
use std::{io, net::SocketAddr};

use crossterm::{execute, style, terminal};
use libp2p::{multiaddr::multiaddr, Multiaddr};
use structopt::StructOpt;

use p2p_chat::{gen_id_keys, Client};

pub mod app;
use app::App;

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
    let mut client = Client::new(&nick, id_keys).await?;

    let port = opts.port.unwrap_or_default();
    client.listen_on(multiaddr!(Ip4([0, 0, 0, 0]), Tcp(port)))?;
    for addr in opts.dial {
        client.dial(addr)?;
    }

    // setup tui

    let mut stdout = io::stdout();

    execute!(stdout, terminal::EnterAlternateScreen)?;
    terminal::enable_raw_mode()?;

    let mut app = App::new(client);
    app.run(&mut stdout).await?;

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
