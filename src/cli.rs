use clap::{Parser, Subcommand};
use std::ffi::OsString;

#[derive(Debug, Parser)]
#[command(
    name = "confy",
    version,
    about = "a config manager for linux/unix based systems",
    long_about = "a config manager for linux/unix based systems. use compact actions to open directly into confy.",
    disable_help_subcommand = true,
    help_template = "{name} {version}\n\n{about}\n\nusage: {usage}\n\n{all-args}{after-help}",
    next_help_heading = "options",
    subcommand_help_heading = "commands",
    after_help = "actions:\n  l, last                 open the last edited config\n  s:<text>                open search with a query\n  o:<path> [g:<group>]   track and open a config\n  d:<host>                switch to a remote device\n  stats                   open the stats dashboard\n  milestones              open the milestone wall\n  :diff                   compare and restore backup versions\n  :hooks                  edit event hooks\n  st:share                print copyable stats\n  st:img[:transparent]    create a stats image\n  st:banner[:transparent] create a wide stats image\n  st:post[:transparent]   create a portrait stats image\n  use to:<path> after an image type to choose the output path\n  hooks                   open hooks.toml in the configured editor\n  import [source]         review configs managed by chezmoi, stow, or yadm\n  h, help, man            show this help\n  v, ver, version, e, edition  show version information\n  completions <shell>     generate shell completions"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

#[derive(Debug, Subcommand)]
pub enum CliCommand {
    #[command(name = "last", alias = "l", about = "open the last edited config")]
    Last,
    #[command(about = "open the stats dashboard")]
    Stats,
    #[command(about = "open the milestone wall")]
    Milestones,
    #[command(about = "edit hooks.toml")]
    Hooks,
    #[command(about = "import configs from chezmoi, stow, or yadm")]
    Import { source: Option<String> },
    #[command(about = "generate shell completions")]
    Completions { shell: clap_complete::Shell },
    #[command(name = "help", alias = "h", alias = "man", about = "show usage information")]
    Help,
    #[command(name = "version", alias = "v", alias = "ver", alias = "e", alias = "edition", about = "show version information")]
    Version,
    #[command(external_subcommand)]
    External(Vec<OsString>),
}

pub fn command() -> clap::Command {
    use clap::CommandFactory;
    Cli::command()
}
