//! A bot that plays Minecraft from a fresh spawn to the dragon's death.
//!
//! `speedrun_bot [--server host:port] [--name Bot] [--owner Player] [--go]`. In chat the
//! owner says `!go` (the whole run), `!stage <name>` (one stage, for testing), `!stop`,
//! `!inv`, `!pos`.

mod craft;
mod inv;
mod obtain;
mod run;
mod world;

use std::sync::Arc;

use azalea::prelude::*;
use parking_lot::Mutex;
use tokio::task::JoinHandle;
use tracing::{error, info};

#[derive(Clone, Component, Default)]
struct State {
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

struct Args {
    server: String,
    name: String,
    owner: Option<String>,
    autostart: bool,
}

fn args() -> Args {
    let mut a = Args { server: "localhost".into(), name: "SpeedBot".into(), owner: None, autostart: false };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        match k.as_str() {
            "--server" => a.server = it.next().expect("--server needs a value"),
            "--name" => a.name = it.next().expect("--name needs a value"),
            "--owner" => a.owner = it.next(),
            "--go" => a.autostart = true,
            other => panic!("unknown argument {other}"),
        }
    }
    a
}

static ARGS: std::sync::OnceLock<Args> = std::sync::OnceLock::new();

#[tokio::main]
async fn main() -> AppExit {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "speedrun_bot=info,azalea=warn".into()),
        )
        .init();
    let a = ARGS.get_or_init(args);
    info!("joining {} as {}", a.server, a.name);
    ClientBuilder::new().set_handler(handle).start(Account::offline(&a.name), a.server.as_str()).await
}

async fn handle(bot: Client, event: Event, state: State) -> eyre::Result<()> {
    let a = ARGS.get().unwrap();
    match event {
        Event::Spawn if a.autostart && state.task.lock().is_none() => start(&bot, &state, None),
        Event::Chat(m) => {
            let (sender, content) = m.split_sender_and_content();
            let Some(sender) = sender else { return Ok(()) };
            if sender == bot.username() || a.owner.as_ref().is_some_and(|o| *o != sender) {
                return Ok(());
            }
            let mut words = content.split_whitespace();
            match words.next() {
                Some("!go") => start(&bot, &state, None),
                Some("!stage") => start(&bot, &state, words.next().map(str::to_owned)),
                Some("!stop") => {
                    if let Some(t) = state.task.lock().take() {
                        t.abort();
                    }
                    bot.force_stop_pathfinding();
                    bot.walk(azalea::WalkDirection::None);
                    bot.chat("stopped");
                }
                Some("!pos") => bot.chat(format!("{}", world::feet(&bot))),
                Some("!inv") => {
                    let menu = inv::player_menu(&bot);
                    let list: Vec<String> = menu
                        .slots()
                        .iter()
                        .filter(|s| s.is_present())
                        .map(|s| format!("{:?}x{}", s.kind(), s.count()))
                        .collect();
                    bot.chat(list.join(", "));
                }
                _ => {}
            }
        }
        _ => {}
    }
    Ok(())
}

fn start(bot: &Client, state: &State, stage: Option<String>) {
    let mut task = state.task.lock();
    if task.as_ref().is_some_and(|t| !t.is_finished()) {
        bot.chat("already running, !stop first");
        return;
    }
    let bot = bot.clone();
    *task = Some(tokio::spawn(async move {
        let result = match &stage {
            Some(s) => run::stage(&bot, s).await,
            None => run::all(&bot).await,
        };
        match result {
            Ok(()) => bot.chat(format!("done: {}", stage.as_deref().unwrap_or("run"))),
            Err(e) => {
                error!("{e:?}");
                bot.chat(format!("failed: {e}"));
            }
        }
    }));
}
