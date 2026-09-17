mod config;
mod remarkable;
mod sync;
mod zotero;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "zoterable",
    version,
    about = "Sync Zotero PDF attachments to a reMarkable tablet"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the config file template and show setup instructions
    Init,
    /// Pair with the reMarkable cloud using a one-time code from
    /// https://my.remarkable.com/device/browser/connect
    Pair { code: String },
    /// Upload newly added Zotero PDF attachments to the reMarkable cloud
    Sync {
        /// Show what would be uploaded without uploading
        #[arg(long)]
        dry_run: bool,
    },
    /// Mark every PDF currently in the library as already synced, so that
    /// `sync` only uploads things added from now on
    Baseline,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init => config::init(),
        Command::Pair { code } => remarkable::pair(&code),
        Command::Sync { dry_run } => sync::run(dry_run),
        Command::Baseline => sync::baseline(),
    }
}
