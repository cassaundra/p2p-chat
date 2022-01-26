use std::io;
use std::time::Duration;

use crossterm::{event::EventStream, execute, style, terminal};
use futures::StreamExt;
use futures_timer::Delay;
use p2p_chat::{gen_id_keys, name_from_peer, Client, ClientEvent};
use structopt::StructOpt;
use tokio::select;

pub mod app;
use app::App;

use crate::app::AppEvent;

#[derive(StructOpt)]
#[structopt(name = "p2p-chat-tui")]
struct Opt {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _opt = Opt::from_args();

    // setup logger
    setup_logger(log::LevelFilter::Debug)?;

    // start client
    let id_keys = gen_id_keys();
    let mut client = Client::new(id_keys).await?;
    client.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    // setup tui

    let mut stdout = io::stdout();

    execute!(stdout, terminal::EnterAlternateScreen)?;
    terminal::enable_raw_mode()?;

    let mut app = App::new();

    let mut term_events = EventStream::new().fuse();

    loop {
        let tick = Delay::new(Duration::from_millis(1000 / 60));

        select! {
            _ = tick => {
                app.draw(&mut stdout)?;
            }
            event = client.next() => {
                if let Some(Some(ClientEvent::Message { contents, source })) = event {
                    let peer_name = name_from_peer(source);
                    app.push_history(format!("{peer_name}: {contents}"));
                }
            }
            event = term_events.select_next_some() => {
                // TODO handle error?
                if let Some(event) = app.handle_event(event?) {
                    match event {
                        AppEvent::SendMessage(message) => client.send_message(&message)?,
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
