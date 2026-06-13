use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use recode_core::{DEFAULT_STATE_DIR, SessionStore};
use serde_json::json;

#[derive(Debug, Parser)]
#[command(name = "recode", version, about = "Recode CLI")]
struct Cli {
    #[arg(long, global = true, default_value = DEFAULT_STATE_DIR)]
    state_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Version,
    Session(SessionCommand),
}

#[derive(Debug, Args)]
struct SessionCommand {
    #[command(subcommand)]
    action: SessionAction,
}

#[derive(Debug, Subcommand)]
enum SessionAction {
    Init {
        #[arg(long)]
        name: String,
    },
    Inspect {
        #[arg(long)]
        id: String,
    },
}

fn main() {
    if let Err(error) = run() {
        let payload = json!({
            "ok": false,
            "error": error.to_string(),
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let store = SessionStore::new(cli.state_dir);

    let payload = match cli.command {
        Command::Version => json!({
            "ok": true,
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
        }),
        Command::Session(command) => match command.action {
            SessionAction::Init { name } => {
                let session = store.init_session(name)?;
                json!({
                    "ok": true,
                    "session": session,
                })
            }
            SessionAction::Inspect { id } => {
                let session = store.load_session(id.parse()?)?;
                json!({
                    "ok": true,
                    "session": session,
                })
            }
        },
    };

    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}
