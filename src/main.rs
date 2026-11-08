use clap::{Parser, Subcommand};
use std::sync::{Arc, Mutex};
use streaming_buddy_rs::{config::Config, db::Db, web::{self, AppState}, wiki_events::{self, ChatEvent}};

#[derive(Parser)]
#[command(name = "streaming-buddy-rs")]
#[command(about = "Streaming Buddy (Rust) - local AI chat assistant for livestreams")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the web GUI server
    Server {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 8811)]
        port: u16,
    },
    /// Create a wiki backup (JSON) in data/backups/
    Backup,
    /// List wiki backups
    BackupList,
    /// Restore a backup by filename (merge). Use --replace to wipe wiki first.
    Restore {
        name: String,
        #[arg(long)]
        replace: bool,
    },
    /// Record an important event into the wiki (category: event)
    Event {
        kind: String,
        user: String,
        detail: String,
    },
    /// Ask one question through the AI pipeline (CLI)
    Ask {
        text: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    let mut cfg = Config::from_env();
    cfg.ensure_dirs()?;
    let db = Arc::new(Db::open(&cfg)?);

    match cli.command {
        Command::Server { host, port } => {
            cfg.host = host.clone();
            cfg.port = port;
            std::env::set_var("SB_SERVER_HOST", &host);
            std::env::set_var("SB_SERVER_PORT", port.to_string());
            let state = Arc::new(AppState {
                db: db.clone(),
                cfg: cfg.clone(),
                twitch: Mutex::new(None),
                vtuber: Mutex::new(web::VtuberState::default()),
            });
            // auto-connect twitch if enabled
            let auto = db.settings_get("twitch_enabled").ok().flatten().unwrap_or_default() == "1";
            if auto {
                if let Ok(h) = streaming_buddy_rs::twitch::spawn(db.clone()).await {
                    *state.twitch.lock().unwrap() = Some(h);
                    let _ = db.add_message("system", "server", "Twitch auto-connected.");
                }
            }
            let app = web::router(state);
            let addr = format!("{}:{}", cfg.host, cfg.port);
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            println!("Streaming Buddy (Rust) listening on http://{addr}");
            println!("VTuber popout (OBS capture): http://{addr}/vtuber");
            axum::serve(listener, app).await?;
        }
        Command::Backup => {
            let p = db.create_backup(&cfg)?;
            println!("Backup created: {}", p.display());
        }
        Command::BackupList => {
            for f in db.list_backups(&cfg)? { println!("{f}"); }
        }
        Command::Restore { name, replace } => {
            let n = db.restore_backup(&cfg, &name, replace)?;
            println!("Restored {n} entries from {name}");
        }
        Command::Event { kind, user, detail } => {
            let ev = ChatEvent { kind, user, channel: String::new(), detail, system_msg: "cli".into() };
            let id = wiki_events::record_event(&db, &ev)?;
            println!("Recorded event (wiki id {:?})", id);
        }
        Command::Ask { text } => {
            match streaming_buddy_rs::twitch::run_ai(&db, &text).await {
                Ok((reply, hits)) => {
                    println!("keywords/hits: {}", hits.len());
                    for h in &hits { println!(" - [{}] {} (score {})", h.entry.category, h.entry.title, h.score); }
                    println!("\n{reply}");
                }
                Err(e) => { eprintln!("AI error: {e}"); std::process::exit(1); }
            }
        }
    }
    Ok(())
}
