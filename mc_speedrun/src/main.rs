//! A bot that plays Minecraft from a fresh spawn to the dragon's death.
//!
//! `speedrun_bot [--server host:port] [--name Bot] [--owner Player] [--go] [--exit]`. In chat the
//! owner says `!go` (the whole run), `!stage <name>` (one stage, for testing), `!stop`,
//! `!inv`, `!pos`.

mod click;
mod craft;
mod dig;
mod inv;
mod nether;
mod obtain;
mod place;
mod portal;
mod telemetry;
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
    /// Quit when the autostarted run ends, so a test does not idle until its timeout.
    exit: bool,
    /// Game ticks to wait after spawning before an autostarted run (a test gives items first).
    delay: usize,
    /// Run only this stage on spawn (testing without chat).
    stage: Option<String>,
}

fn args() -> Args {
    let mut a = Args { server: "localhost".into(), name: "SpeedBot".into(), owner: None, autostart: false, exit: false, delay: 0, stage: None };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        match k.as_str() {
            "--server" => a.server = it.next().expect("--server needs a value"),
            "--name" => a.name = it.next().expect("--name needs a value"),
            "--owner" => a.owner = it.next(),
            "--go" => a.autostart = true,
            "--exit" => a.exit = true,
            "--delay" => a.delay = it.next().and_then(|v| v.parse().ok()).expect("--delay needs ticks"),
            "--stage" => {
                a.autostart = true;
                a.stage = it.next();
            }
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
    // A frozen client stops answering keep-alives and is kicked a minute later; print who
    // holds which lock when that happens.
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        for (i, threads) in parking_lot::deadlock::check_deadlock().iter().enumerate() {
            error!("deadlock #{i}");
            for t in threads {
                error!("thread {:?}:\n{:?}", t.thread_id(), t.backtrace());
            }
        }
    });
    info!("joining {} as {}", a.server, a.name);
    match telemetry::start() {
        Ok(path) => info!("run log: {path}"),
        Err(e) => error!("no run log: {e}"),
    }
    ClientBuilder::new().set_handler(handle).start(Account::offline(&a.name), a.server.as_str()).await
}

async fn handle(bot: Client, event: Event, state: State) -> eyre::Result<()> {
    let a = ARGS.get().unwrap();
    match event {
        Event::Tick => telemetry::sample(&bot),
        Event::Packet(p) => telemetry::packet(&bot, &p),
        Event::Death(_) => telemetry::record(&bot, "death", serde_json::json!({})),
        Event::Spawn if a.autostart && state.task.lock().is_none() => start(&bot, &state, a.stage.clone()),
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
                    let menu = inv::player_slots(&bot);
                    let list: Vec<String> = menu
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
        let delay = ARGS.get().map_or(0, |a| a.delay);
        if delay > 0 {
            bot.wait_ticks(delay).await;
        }
        // `--stage a,b` runs those stages in order (a test of several at once)
        let result = match &stage {
            Some(list) => {
                let mut r = Ok(());
                for s in list.split(',') {
                    r = run::stage(&bot, s).await;
                    if r.is_err() {
                        break;
                    }
                }
                r
            }
            None => run::all(&bot).await,
        };
        let ok = result.is_ok();
        match result {
            Ok(()) => bot.chat(format!("done: {}", stage.as_deref().unwrap_or("run"))),
            Err(e) => {
                error!("{e:?}");
                bot.chat(format!("failed: {e}"));
            }
        }
        if ARGS.get().is_some_and(|a| a.exit) {
            info!("run over (ok={ok}), exiting");
            // the chat line and the log's last records need a moment to go out
            bot.wait_ticks(10).await;
            std::process::exit(if ok { 0 } else { 1 });
        }
    }));
}
