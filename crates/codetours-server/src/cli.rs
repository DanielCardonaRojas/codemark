use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[arg(long, help = "Path to the configuration file")]
    pub config: Option<PathBuf>,

    #[arg(long, help = "Enable JSON structured logging")]
    pub json_logs: bool,
}
