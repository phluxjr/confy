// confy - a config manager for linux/unix systems
// copyright (c) 2025-2026 phluxjr
// licensed under gpl-3.0-or-later

mod app;
mod cli;
mod commands;
mod config;
mod device;
mod git;
mod hooks;
mod import;
mod picker;
mod themenet;
mod tutorial;
mod ui;
mod stats_image;
mod web;

use anyhow::Result;
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--web") {
        let mut options = web::LaunchOptions::default();
        if let Ok(password) = std::env::var("CONFY_WEB_PASSWORD") {
            if !password.is_empty() {
                options.password = Some(password);
                options.password_overridden = true;
            }
        }
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--web" => i += 1,
                "--bind" | "--ip" if i + 1 < args.len() => {
                    options.ip = Some(args[i + 1].clone());
                    i += 2;
                }
                "--port" if i + 1 < args.len() => {
                    options.port = Some(args[i + 1].parse()?);
                    i += 2;
                }
                "--password" if i + 1 < args.len() => {
                    options.password = Some(args[i + 1].clone());
                    options.password_overridden = true;
                    i += 2;
                }
                "--no-password" => {
                    options.password = None;
                    options.password_overridden = true;
                    i += 1;
                }
                "--gui" => {
                    options.mode = Some(web::WebMode::Gui);
                    i += 1;
                }
                "--text" => {
                    options.mode = Some(web::WebMode::Text);
                    i += 1;
                }
                "--mode" if i + 1 < args.len() => {
                    options.mode = Some(match args[i + 1].as_str() {
                        "gui" => web::WebMode::Gui,
                        "text" => web::WebMode::Text,
                        other => anyhow::bail!("unknown web mode '{other}' (expected gui or text)"),
                    });
                    i += 2;
                }
                "-ni" | "--no-interactive" => {
                    options.no_interactive = true;
                    i += 1;
                }
                "--help" | "-h" => {
                    println!(
                        "usage: confy --web [--gui | --text | --mode gui|text] [--ip address] [--port port] [--password password | --no-password] [-ni]\n\nlaunch arguments override the interactive setup. defaults: gui, 127.0.0.1, port 3000, no password."
                    );
                    return Ok(());
                }
                other => anyhow::bail!("unknown web option '{other}' (try confy --web --help)"),
            }
        }
        options.prepare()?;
        return web::serve(options);
    }
    // restore terminal on panic so the user isn't left with a mangled shell
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(io::stderr(), LeaveAlternateScreen);
        orig_hook(info);
    }));

    use clap::{CommandFactory, Parser};
    if args.len() == 1 && matches!(args[0].as_str(), "h" | "help" | "man") {
        cli::Cli::command().print_help()?;
        println!();
        return Ok(());
    }
    let cli = match cli::Cli::try_parse_from(std::iter::once("confy".to_string()).chain(args)) {
        Ok(cli) => cli,
        Err(error) if matches!(error.kind(), clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion) => {
            error.print()?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    match cli.command {
        None => run_main_tui(app::App::new(), None),
        Some(cli::CliCommand::Last) => {
            let mut app = app::App::new();
            match app.cfg.last_opened.clone() {
                Some(path) if app.local_path(&path).is_file() => app.open_file(&path),
                Some(_) => app.popup = Some("last file no longer exists".into()),
                None => app.popup = Some("nothing opened yet".into()),
            }
            run_main_tui(app, None)
        }
        Some(cli::CliCommand::Stats) => {
            let app = app::App::new();
            run_terminal(|terminal| ui::run_stats_pane(terminal, &app, false))
        }
        Some(cli::CliCommand::Milestones) => {
            let app = app::App::new();
            run_terminal(|terminal| ui::run_stats_pane(terminal, &app, true))
        }
        Some(cli::CliCommand::Hooks) => {
            let path = hooks::ensure_file()?;
            std::process::Command::new(app::App::new().editor()).arg(path).status()?;
            Ok(())
        }
        Some(cli::CliCommand::Import { source }) => run_import(source.as_deref()),
        Some(cli::CliCommand::Completions { shell }) => {
            let mut command = cli::command();
            clap_complete::generate(shell, &mut command, "confy", &mut io::stdout());
            Ok(())
        }
        Some(cli::CliCommand::Help) => {
            cli::Cli::command().print_help()?;
            println!();
            Ok(())
        }
        Some(cli::CliCommand::Version) => {
            println!("confy {} (edition 2024)", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(cli::CliCommand::External(raw)) => match parse_compact(raw)? {
            Compact::Search(query) => run_main_tui(app::App::new(), Some(query)),
            Compact::Last => {
                let mut app = app::App::new();
                match app.cfg.last_opened.clone() {
                    Some(path) if app.local_path(&path).is_file() => app.open_file(&path),
                    Some(_) => app.popup = Some("last file no longer exists".into()),
                    None => app.popup = Some("nothing opened yet".into()),
                }
                run_main_tui(app, None)
            }
            Compact::Open { path, group } => {
                let mut app = app::App::new();
                open_config(&mut app, &path, &group)?;
                run_main_tui(app, None)
            }
            Compact::Device(target) => {
                let mut app = app::App::new();
                if let Err(error) = app.switch_device(&target) { app.popup = Some(error.to_string()); }
                run_main_tui(app, None)
            }
            Compact::Share => { println!("{}", app::App::new().stats_summary()); Ok(()) }
            Compact::Image { format, transparent, destination } => {
                let path = stats_image::write(&app::App::new(), format, transparent, destination.as_deref())?;
                println!("stats image written to {}", path.display());
                Ok(())
            }
            Compact::Import(source) => run_import(source.as_deref()),
            Compact::Help => {
                cli::Cli::command().print_help()?;
                println!();
                Ok(())
            }
            Compact::Version => { println!("confy {} (edition 2024)", env!("CARGO_PKG_VERSION")); Ok(()) }
        },
    }
}

enum Compact {
    Search(String),
    Last,
    Open { path: String, group: String },
    Device(String),
    Share,
    Image { format: stats_image::Format, transparent: bool, destination: Option<std::path::PathBuf> },
    Import(Option<String>),
    Help,
    Version,
}

fn parse_compact(args: Vec<std::ffi::OsString>) -> Result<Compact> {
    let args: Vec<String> = args.into_iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    let action = args.first().map(String::as_str).unwrap_or("");
    if matches!(action, "h" | "help" | "man") { return Ok(Compact::Help); }
    if matches!(action, "v" | "ver" | "version" | "e" | "edition") { return Ok(Compact::Version); }
    if matches!(action, "l" | "last") {
        return Ok(Compact::Last);
    }
    if action == "import" { return Ok(Compact::Import(args.get(1).cloned())); }
    if action == "st:share" { return Ok(Compact::Share); }
    if let Some(query) = action.strip_prefix("s:") { return Ok(Compact::Search(query.to_string())); }
    if action == "s" { return Ok(Compact::Search(args.get(1..).unwrap_or_default().join(" "))); }
    if let Some(target) = action.strip_prefix("d:") {
        if target.is_empty() { anyhow::bail!("usage: confy d:<ssh-target>"); }
        return Ok(Compact::Device(target.to_string()));
    }
    if let Some(path) = action.strip_prefix("o:") {
        let mut path_parts = vec![path.to_string()];
        let mut group = "ungrouped".to_string();
        for item in args.iter().skip(1) {
            if let Some(name) = item.strip_prefix("g:") { group = name.to_string(); }
            else { path_parts.push(item.to_string()); }
        }
        if path.is_empty() { anyhow::bail!("usage: confy o:<path> [g:<group>]"); }
        return Ok(Compact::Open { path: path_parts.join(" "), group });
    }
    if action.starts_with("st:") {
        let parts: Vec<&str> = action.split(':').collect();
        let format = match parts.get(1).copied().unwrap_or("") {
            "img" => stats_image::Format::Image,
            "banner" => stats_image::Format::Banner,
            "post" => stats_image::Format::Post,
            other => anyhow::bail!("unknown stats image type '{other}' (use img, banner, or post)"),
        };
        let transparent = parts.get(2).copied() == Some("transparent");
        let destination = args.iter().skip(1).find_map(|arg| arg.strip_prefix("to:").map(std::path::PathBuf::from));
        return Ok(Compact::Image { format, transparent, destination });
    }
    anyhow::bail!("unknown command '{action}' (try confy --help)")
}

fn open_config(app: &mut app::App, path: &str, group: &str) -> Result<()> {
    let expanded = if path == "~" { dirs::home_dir().unwrap_or_default() }
        else if let Some(rest) = path.strip_prefix("~/") { dirs::home_dir().unwrap_or_default().join(rest) }
        else { std::path::PathBuf::from(path) };
    let path = if expanded.is_absolute() { expanded } else { std::env::current_dir()?.join(expanded) };
    if !path.is_file() { anyhow::bail!("not a file: {}", path.display()); }
    let tracked = app.tracked_path(&path);
    if !app.cfg.groups.values().any(|files| files.contains(&tracked)) {
        app.track_file(&path, group);
    }
    app.open_file(&tracked);
    Ok(())
}

fn run_main_tui(mut app: app::App, search: Option<String>) -> Result<()> {
    run_terminal(|terminal| ui::run_with_search(terminal, &mut app, search))
}

fn run_import(source: Option<&str>) -> Result<()> {
    let candidates = import::discover(source)?;
    let mut app = app::App::new();
    run_terminal(|terminal| ui::run_import_review(terminal, &mut app, candidates))?;
    if let Some(message) = &app.popup { println!("{message}"); }
    Ok(())
}

fn run_terminal(action: impl FnOnce(&mut Terminal<CrosstermBackend<io::Stderr>>) -> Result<()>) -> Result<()> {
    enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;
    let result = action(&mut terminal);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}
