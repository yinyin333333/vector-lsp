use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
pub struct CliArgs {
    #[arg(long, default_value = "config.json")]
    pub config_file: String,
    /// Validate the workspace and exit (overrides the config `single_shot` setting).
    #[arg(long)]
    pub single_shot: bool,
    /// Path to the schema directory (overrides the config `schema_path` setting).
    #[arg(long)]
    pub schema_path: Option<PathBuf>,
}
