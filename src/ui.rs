use crate::app::{App, Item};
use crate::commands::{self, CmdResult};
use crate::config::{self, ColorSettings};
use crate::picker::Picker;
use crate::tutorial;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    Terminal,
    backend::Backend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
        Wrap,
    },
};
use std::path::{Path, PathBuf};
use similar::TextDiff;

#[derive(Default, PartialEq)]
enum Mode {
    #[default]
    Normal,
    Command,
    Search,
}

fn resolve_color(s: &str) -> Color {
    match s.to_lowercase().as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" | "pink" | "purple" => Color::Magenta,
        "cyan" | "lavender" => Color::Cyan,
        "white" => Color::White,
        "default" => Color::Reset,
        hex if hex.starts_with('#') && hex.len() == 7 => {
            let r = u8::from_str_radix(&hex[1..3], 16).unwrap_or(255);
            let g = u8::from_str_radix(&hex[3..5], 16).unwrap_or(255);
            let b = u8::from_str_radix(&hex[5..7], 16).unwrap_or(255);
            Color::Rgb(r, g, b)
        }
        _ => Color::Reset,
    }
}

struct Colors {
    normal: Style,
    highlight: Style,
    group: Style,
    error: Style,
}

impl Colors {
    fn from_app(app: &App) -> Self {
        Self::from_settings(&app.cfg.settings.colors)
    }

    fn from_settings(c: &ColorSettings) -> Self {
        let fg = resolve_color(&c.fg);
        let bg = resolve_color(&c.bg);
        let hi = resolve_color(&c.highlight);
        let gr = resolve_color(&c.group);
        Self {
            normal: Style::default().fg(fg).bg(bg),
            highlight: Style::default()
                .fg(hi)
                .bg(bg)
                .add_modifier(Modifier::REVERSED),
            group: Style::default().fg(gr).bg(bg).add_modifier(Modifier::BOLD),
            error: Style::default().fg(Color::Red).bg(bg),
        }
    }
}

pub fn run_with_search<B: Backend>(terminal: &mut Terminal<B>, app: &mut App, initial_search: Option<String>) -> Result<()> {
    let mut mode = if initial_search.is_some() { Mode::Search } else { Mode::Normal };
    let mut cmd_buf = String::new();
    let mut search_buf = initial_search.unwrap_or_default();
    if !search_buf.is_empty() {
        app.rebuild_flat_view_filtered(&search_buf);
        app.selected = 0;
    }
    let mut table_state = TableState::default();

    if app.show_tutorial {
        run_tutorial(terminal, app)?;
        app.show_tutorial = false;
        app.cfg.settings.first_startup = false;
        app.save();
    }

    loop {
        table_state.select(if app.flat_view.is_empty() {
            None
        } else {
            Some(app.selected)
        });

        terminal.draw(|f| {
            let colors = Colors::from_app(app);
            draw_main::<B>(
                f,
                app,
                &colors,
                &mode,
                &cmd_buf,
                &search_buf,
                &mut table_state,
            );
        })?;

        if app.popup.is_some() {
            terminal.draw(|f| {
                let colors = Colors::from_app(app);
                draw_main::<B>(
                    f,
                    app,
                    &colors,
                    &mode,
                    &cmd_buf,
                    &search_buf,
                    &mut table_state,
                );
                draw_popup(f, app.popup.as_deref().unwrap_or(""));
            })?;
            event::read()?;
            app.popup = None;
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match mode {
            Mode::Command => match key.code {
                KeyCode::Enter => {
                    let raw = cmd_buf.clone();
                    cmd_buf.clear();
                    mode = Mode::Normal;
                    match commands::handle(app, &raw) {
                        CmdResult::Quit => break,
                        CmdResult::NeedPicker { group } => {
                            terminal.clear()?;
                            let browse = app
                                .local_path(&app.cfg.config_dir)
                                .to_string_lossy()
                                .into_owned();
                            let mut picker = Picker::new(&browse, false);
                            if let Some(picked) = picker.run(terminal)? {
                                let tracked = app.tracked_path(&picked);
                                app.track_file(&std::path::PathBuf::from(&tracked), &group);
                            }
                            terminal.clear()?;
                        }
                        CmdResult::NeedCdPicker => {
                            terminal.clear()?;
                            let mut picker = Picker::new(&app.cfg.config_dir, true);
                            if let Some(dir) = picker.run(terminal)? {
                                app.cfg.config_dir = dir.to_string_lossy().into_owned();
                                app.save();
                                app.popup = Some(format!("config dir → {}", app.cfg.config_dir));
                            }
                            terminal.clear()?;
                        }
                        CmdResult::NeedGitDirPicker => {
                            terminal.clear()?;
                            let mut picker = Picker::new(&app.cfg.git_dir, true);
                            if let Some(dir) = picker.run(terminal)? {
                                app.cfg.git_dir = dir.to_string_lossy().into_owned();
                                app.save();
                                app.popup = Some(format!("git dir → {}", app.cfg.git_dir));
                            }
                            terminal.clear()?;
                        }
                        CmdResult::NeedBouncer => {
                            run_bouncer(terminal, app)?;
                        }
                        CmdResult::NeedThemeWizard => {
                            run_theme_wizard(terminal, app)?;
                        }
                        CmdResult::NeedThemeBrowser => {
                            run_themenet(terminal, app)?;
                        }
                        CmdResult::NeedStats => {
                            run_stats(terminal, app, true, false)?;
                        }
                        CmdResult::NeedDiff { path } => {
                            run_diff_menu(terminal, app, &path)?;
                        }
                        CmdResult::NeedHooks => {
                            run_hooks_editor(terminal, app)?;
                        }
                        CmdResult::NeedImport { candidates } => {
                            run_import_review(terminal, app, candidates)?;
                        }
                        CmdResult::NeedCommitMsg { then_push } => {
                            drop_alt(terminal)?;
                            let msg = prompt_line("commit message: ")?;
                            restore_alt(terminal)?;
                            if !msg.trim().is_empty() {
                                commands::git_commit_and_maybe_push(app, msg.trim(), then_push);
                            } else {
                                app.popup = Some("commit cancelled (empty message)".into());
                            }
                        }
                        CmdResult::Ok => {
                            if app.show_tutorial {
                                run_tutorial(terminal, app)?;
                                app.show_tutorial = false;
                            }
                        }
                    }
                }
                KeyCode::Esc => {
                    cmd_buf.clear();
                    mode = Mode::Normal;
                }
                KeyCode::Backspace => {
                    cmd_buf.pop();
                }
                KeyCode::Char(c) => cmd_buf.push(c),
                _ => {}
            },

            Mode::Search => match key.code {
                KeyCode::Enter => {
                    mode = Mode::Normal;
                }
                KeyCode::Esc => {
                    search_buf.clear();
                    app.rebuild_flat_view();
                    app.selected = 0;
                    mode = Mode::Normal;
                }
                KeyCode::Backspace => {
                    search_buf.pop();
                    app.rebuild_flat_view_filtered(&search_buf);
                    app.selected = 0;
                }
                KeyCode::Char(c) => {
                    search_buf.push(c);
                    app.rebuild_flat_view_filtered(&search_buf);
                    app.selected = 0;
                }
                _ => {}
            },

            Mode::Normal => match key.code {
                KeyCode::Char(':') => {
                    cmd_buf.clear();
                    mode = Mode::Command;
                }
                KeyCode::Char('/') => {
                    search_buf.clear();
                    mode = Mode::Search;
                }
                KeyCode::Char('q') => break,
                KeyCode::Char('j') | KeyCode::Down => app.move_down(),
                KeyCode::Char('k') | KeyCode::Up => app.move_up(),
                KeyCode::Char('p') => {
                    app.cfg.preview_enabled = !app.cfg.preview_enabled;
                    app.save();
                }
                KeyCode::Char(' ') => app.toggle_group(),
                KeyCode::Enter => match app.selected_item().cloned() {
                    Some(Item::File { path, .. }) => {
                        drop_alt(terminal)?;
                        app.open_file(&path);
                        restore_alt(terminal)?;
                    }
                    Some(Item::Group(_)) => app.toggle_group(),
                    None => {}
                },
                _ => {}
            },
        }
    }
    Ok(())
}

fn draw_main<B: Backend>(
    f: &mut ratatui::Frame,
    app: &App,
    colors: &Colors,
    mode: &Mode,
    cmd_buf: &str,
    search_buf: &str,
    table_state: &mut TableState,
) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(area);
    draw_header(f, app, colors, chunks[0]);
    draw_body(f, app, colors, table_state, chunks[1]);
    draw_footer(f, app, colors, mode, cmd_buf, search_buf, chunks[2]);
}

fn draw_header(f: &mut ratatui::Frame, app: &App, colors: &Colors, area: Rect) {
    let last = app
        .cfg
        .last_opened
        .as_deref()
        .and_then(|p| Path::new(p).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "none".into());
    let device_part = app
        .device_name
        .as_deref()
        .map(|d| format!("  [remote: {d}]"))
        .unwrap_or_default();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("confy", colors.group),
            Span::styled(device_part, colors.error),
        ])),
        chunks[0],
    );

    let info = format!(
        "previous: {{{}}}    sort: {} ({})    config dir: {}",
        last, app.cfg.sort_mode, app.cfg.sort_order, app.cfg.config_dir
    );
    f.render_widget(Paragraph::new(Span::styled(info, colors.normal)), chunks[1]);
}

fn draw_body(
    f: &mut ratatui::Frame,
    app: &App,
    colors: &Colors,
    table_state: &mut TableState,
    area: Rect,
) {
    let preview_on = app.cfg.preview_enabled && area.width >= 60;
    let (list_area, preview_area) = if preview_on {
        let half = area.width / 2;
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(half), Constraint::Min(0)])
            .split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };

    let rows: Vec<Row> = app
        .flat_view
        .iter()
        .map(|item| match item {
            Item::Group(name) => {
                let collapsed = if app.cfg.collapsed_groups.contains(name) {
                    "▶"
                } else {
                    "▼"
                };
                let count = app.cfg.groups.get(name).map(|v| v.len()).unwrap_or(0);
                Row::new(vec![
                    Cell::from(format!("{collapsed} {name}/ ({count} files)")),
                    Cell::from(""),
                    Cell::from(""),
                ])
                .style(colors.group)
            }
            Item::File { path, .. } => {
                let fname = Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                let exists = app.local_path(path).exists();
                let style = if exists { colors.normal } else { colors.error };
                if app.cfg.settings.streamer_mode {
                    Row::new(vec![
                        Cell::from(format!("  {fname}")),
                        Cell::from(""),
                        Cell::from("***"),
                    ])
                    .style(style)
                } else {
                    let dir = app.display_dir(path);
                    let (mtime, _size) = app.file_info(path);
                    Row::new(vec![
                        Cell::from(format!("  {fname}")),
                        Cell::from(mtime),
                        Cell::from(dir),
                    ])
                    .style(style)
                }
            }
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(20),
            Constraint::Length(17),
            Constraint::Length(40),
        ],
    )
    .block(Block::default().borders(Borders::TOP))
    .row_highlight_style(colors.highlight)
    .highlight_symbol("▶ ");
    f.render_stateful_widget(table, list_area, table_state);

    if let Some(parea) = preview_area {
        f.render_widget(Clear, parea);

        let (header, body) = match app.selected_item() {
            None => ("(nothing selected)".into(), vec![]),
            Some(Item::Group(name)) => {
                let count = app.cfg.groups.get(name).map(|v| v.len()).unwrap_or(0);
                (
                    format!("{name}/"),
                    vec![
                        format!("{count} file(s) in group"),
                        String::new(),
                        "select a file to preview".into(),
                    ],
                )
            }
            Some(Item::File { path, .. }) => {
                let lines = app.preview_lines(
                    path,
                    (parea.height as usize).saturating_sub(3),
                    (parea.width as usize).saturating_sub(2),
                );
                let fname = Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (fname, lines)
            }
        };
        let preview_text: Vec<Line> =
            std::iter::once(Line::from(Span::styled(header, colors.group)))
                .chain(std::iter::once(Line::from("")))
                .chain(
                    body.into_iter()
                        .map(|l| Line::from(Span::styled(l, colors.normal))),
                )
                .collect();
        f.render_widget(
            Paragraph::new(preview_text)
                .block(Block::default().borders(Borders::TOP | Borders::LEFT))
                .wrap(Wrap { trim: false }),
            parea,
        );
    }
}

fn draw_footer(
    f: &mut ratatui::Frame,
    app: &App,
    colors: &Colors,
    mode: &Mode,
    cmd_buf: &str,
    search_buf: &str,
    area: Rect,
) {
    let terminal_h = area.bottom() as usize;
    let body_h = terminal_h.saturating_sub(5).max(1);
    let total = app.flat_view.len().max(1);
    let pages = (total + body_h - 1) / body_h;
    let page = (app.selected / body_h) + 1;

    let status = match mode {
        Mode::Command => format!("page {page}/{pages} ▌ :{cmd_buf}"),
        Mode::Search => format!("page {page}/{pages} ▌ /{search_buf}"),
        Mode::Normal => {
            let preview = if app.cfg.preview_enabled { "on" } else { "off" };
            format!("page {page}/{pages} ▌ p: preview {preview}")
        }
    };
    f.render_widget(
        Paragraph::new(Span::styled(status, colors.normal))
            .block(Block::default().borders(Borders::TOP)),
        area,
    );
}

fn confy_since() -> (String, String) {
    let created = config::confy_config_dir().metadata().and_then(|metadata| metadata.created()).ok();
    let Some(created) = created else { return ("unknown".into(), "date unavailable".into()); };
    let Some(seconds) = created.duration_since(std::time::UNIX_EPOCH).ok().map(|duration| duration.as_secs()) else {
        return ("unknown".into(), "date unavailable".into());
    };
    let date = crate::app::format_unix_ts(seconds).split_whitespace().next().unwrap_or("unknown").to_string();
    let days = std::time::SystemTime::now().duration_since(created).unwrap_or_default().as_secs() / 86_400;
    let age = if days >= 365 { format!("{}y {}mo ago", days / 365, (days % 365) / 30) }
        else if days >= 30 { format!("{}mo ago", days / 30) }
        else { format!("{}d ago", days) };
    (date, age)
}

fn edit_activity(stats: &crate::config::UsageStats) -> (u64, Option<(String, u64)>) {
    let busiest = stats.edits_by_day.iter()
        .max_by(|(day_a, count_a), (day_b, count_b)| count_a.cmp(count_b).then_with(|| day_a.cmp(day_b)))
        .map(|(day, count)| (day.clone(), *count));
    let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else { return (0, busiest); };
    let today = now.as_secs() / 86_400;
    let today_key = crate::app::format_unix_ts(today * 86_400).split_whitespace().next().unwrap_or("").to_string();
    let start = if stats.edits_by_day.contains_key(&today_key) { today } else { today.saturating_sub(1) };
    let mut streak = 0;
    for offset in 0..=today {
        let day = start.saturating_sub(offset);
        let key = crate::app::format_unix_ts(day * 86_400).split_whitespace().next().unwrap_or("").to_string();
        if stats.edits_by_day.contains_key(&key) { streak += 1; } else { break; }
    }
    (streak, busiest)
}

const ACHIEVEMENTS: [(&str, &str, &str); 17] = [
    ("welcome_to_confy", "welcome to confy!", "first launch"),
    ("youre_in", "you're in", "first edit"),
    ("oops", "oops", "first rollback"),
    ("collection", "collection", "10 configs tracked"),
    ("hoarder", "hoarder", "20 configs tracked"),
    ("storage_room", "storage room", "30 configs tracked"),
    ("configs_managed", "configs: managed", "50 configs tracked"),
    ("dotfile_demon", "dotfile demon", "100 configs tracked"),
    ("editorial", "editorial", "100 edits"),
    ("new_clothes", "new clothes!", "first theme switch"),
    ("self_sewed", "self-sewed", "make your first custom theme"),
    ("thrift_shop", "thrift shop", "download a themenet theme"),
    ("going_live", "going live", "use streamer mode"),
    ("haunting_a_remote_machine", "haunting a remote machine", "use :device"),
    ("with_great_power", "with great power..", "use :su"),
    ("happy_birthday_to_me", "happy birthday to me!", "launch confy on november 10"),
    ("one_of_us", "one of us", "one year since first launch"),
];

pub fn run_stats_pane<B: Backend>(terminal: &mut Terminal<B>, app: &App, show_milestones: bool) -> Result<()> {
    run_stats(terminal, app, true, show_milestones)
}

fn run_stats<B: Backend>(terminal: &mut Terminal<B>, app: &App, mut sort_by_count: bool, mut show_milestones: bool) -> Result<()> {
    loop {
        terminal.draw(|f| if show_milestones { draw_milestones(f, app) } else { draw_stats(f, app, sort_by_count) })?;
        let Event::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press { continue; }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => break,
            KeyCode::Char('s') | KeyCode::Tab => sort_by_count = !sort_by_count,
            KeyCode::Char('m') => show_milestones = !show_milestones,
            _ => {}
        }
    }
    Ok(())
}

fn draw_milestones(f: &mut ratatui::Frame, app: &App) {
    let area = f.area();
    let colors = Colors::from_app(app);
    let accent = resolve_color(&app.cfg.settings.colors.highlight);
    let glow = Style::default().fg(accent).add_modifier(Modifier::BOLD);
    let earned = app.cfg.stats.milestones.len().min(ACHIEVEMENTS.len());
    let outer = Block::default().title(format!(" ✦ milestone wall · {earned}/{} unlocked ✦ ", ACHIEVEMENTS.len()))
        .title_style(glow).borders(Borders::ALL).border_style(Style::default().fg(accent));
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    let columns = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)]).split(inner);
    for column in 0..2 {
        let start = column * ACHIEVEMENTS.len().div_ceil(2);
        let end = ((column + 1) * ACHIEVEMENTS.len().div_ceil(2)).min(ACHIEVEMENTS.len());
        let lines: Vec<Line> = ACHIEVEMENTS[start..end].iter().map(|(id, name, description)| {
            let unlocked = app.cfg.stats.milestones.iter().any(|milestone| milestone == id);
            let style = if unlocked { glow } else { colors.group };
            Line::from(vec![
                Span::styled(if unlocked { "◆ " } else { "◇ " }, style),
                Span::styled(*name, style),
                Span::styled(format!("  ·  {description}"), colors.normal),
            ])
        }).collect();
        let block = Block::default().title(if column == 0 { " achievements " } else { " side quests " })
            .borders(Borders::ALL).border_style(Style::default().fg(resolve_color(&app.cfg.settings.colors.group)));
        f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: true }), columns[column]);
    }
}

pub fn run_import_review<B: Backend>(terminal: &mut Terminal<B>, app: &mut App, candidates: Vec<crate::import::ImportCandidate>) -> Result<()> {
    let tracked: std::collections::HashSet<String> = app.cfg.groups.values().flatten().cloned().collect();
    let mut selected: Vec<bool> = candidates.iter().map(|candidate| {
        !tracked.contains(&candidate.path.to_string_lossy().into_owned())
    }).collect();
    let mut cursor = 0usize;
    loop {
        terminal.draw(|f| {
            let area = f.area();
            let colors = Colors::from_app(app);
            let block = Block::default().title(" ✦ import review · choose configs to track ✦ ")
                .title_style(colors.highlight).borders(Borders::ALL).border_style(colors.group);
            let inner = block.inner(area);
            f.render_widget(block, area);
            let layout = Layout::default().direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(2)]).split(inner);
            let visible = layout[0].height.saturating_sub(1) as usize;
            let start = cursor.saturating_sub(visible.saturating_sub(1));
            let lines: Vec<ListItem> = candidates.iter().enumerate().skip(start).take(visible).map(|(index, candidate)| {
                let is_tracked = tracked.contains(&candidate.path.to_string_lossy().into_owned());
                let mark = if is_tracked { "✓" } else if selected[index] { "[x]" } else { "[ ]" };
                let label = candidate.path.strip_prefix(dirs::home_dir().unwrap_or_default()).unwrap_or(&candidate.path).display().to_string();
                let status = if is_tracked { " already tracked" } else { "" };
                let style = if index == cursor { colors.highlight } else { colors.normal };
                ListItem::new(Line::from(vec![Span::styled(format!("{mark} "), style), Span::styled(format!("{label}  "), style), Span::styled(format!("[{}]{status}", candidate.source), colors.group)]))
            }).collect();
            let mut state = ListState::default();
            state.select(Some(cursor.saturating_sub(start)));
            f.render_stateful_widget(List::new(lines).highlight_style(colors.highlight), layout[0], &mut state);
            let count = selected.iter().filter(|item| **item).count();
            f.render_widget(Paragraph::new(format!("{count} selected · j/k move · space toggle · a all · n none · enter import · esc cancel"))
                .alignment(Alignment::Center).style(colors.group), layout[1]);
        })?;
        let Event::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press { continue; }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => break,
            KeyCode::Down | KeyCode::Char('j') => cursor = (cursor + 1).min(candidates.len().saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => cursor = cursor.saturating_sub(1),
            KeyCode::Char(' ') => {
                if let Some(item) = selected.get_mut(cursor) {
                    let is_tracked = tracked.contains(&candidates[cursor].path.to_string_lossy().into_owned());
                    if !is_tracked { *item = !*item; }
                }
            }
            KeyCode::Char('a') => for (index, item) in selected.iter_mut().enumerate() {
                *item = !tracked.contains(&candidates[index].path.to_string_lossy().into_owned());
            },
            KeyCode::Char('n') => selected.fill(false),
            KeyCode::Enter => {
                let paths: Vec<PathBuf> = candidates.iter().zip(&selected).filter_map(|(candidate, include)| include.then_some(candidate.path.clone())).collect();
                let added = app.import_files(&paths);
                app.popup = Some(if added == 0 { "no new configs selected".into() } else { format!("imported {added} configs into ungrouped") });
                break;
            }
            _ => {}
        }
    }
    Ok(())
}

fn run_diff_menu<B: Backend>(terminal: &mut Terminal<B>, app: &mut App, tracked: &str) -> Result<()> {
    let snapshots = match app.backup_snapshots(tracked) {
        Ok(snapshots) => snapshots,
        Err(error) => {
            app.popup = Some(error.to_string());
            return Ok(());
        }
    };
    let local = app.local_path(tracked);
    let current_bytes = match std::fs::read(&local) {
        Ok(contents) => contents,
        Err(error) => {
            app.popup = Some(format!("couldn't read current file: {error}"));
            return Ok(());
        }
    };
    let current = String::from_utf8_lossy(&current_bytes).into_owned();
    let title = app.display_path(tracked).into_owned();
    let mut selected = 0usize;
    let mut scroll = 0u16;
    loop {
        terminal.draw(|f| {
            let area = f.area();
            let colors = Colors::from_app(app);
            let outer = Block::default().title(format!(" ✦ diff · {title} ✦ "))
                .title_style(colors.highlight).borders(Borders::ALL).border_style(colors.group);
            let inner = outer.inner(area);
            f.render_widget(outer, area);
            let layout = Layout::default().direction(Direction::Vertical)
                .constraints([Constraint::Length(2), Constraint::Min(3), Constraint::Length(1)]).split(inner);
            let header = snapshots.get(selected).map(|snapshot| format!("current  ←→  backup from {}", snapshot.label))
                .unwrap_or_else(|| "current file  ·  no backups available".into());
            f.render_widget(Paragraph::new(header).style(colors.highlight).alignment(Alignment::Center), layout[0]);
            let panels = Layout::default().direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(28), Constraint::Percentage(72)]).split(layout[1]);
            let items: Vec<ListItem> = if snapshots.is_empty() {
                vec![ListItem::new("no backups found")]
            } else {
                snapshots.iter().enumerate().map(|(index, snapshot)| {
                    ListItem::new(format!("{:02}  {}", index + 1, snapshot.label))
                }).collect()
            };
            let mut state = ListState::default();
            if !snapshots.is_empty() { state.select(Some(selected)); }
            f.render_stateful_widget(List::new(items).block(Block::default().title(" backups ").borders(Borders::ALL))
                .highlight_style(colors.highlight), panels[0], &mut state);

            let diff_lines = snapshots.get(selected).map(|snapshot| {
                let previous = String::from_utf8_lossy(&snapshot.contents).into_owned();
                let diff = TextDiff::from_lines(&previous, &current).unified_diff()
                    .context_radius(3).header(&snapshot.label, &title).to_string();
                if diff.is_empty() { "no differences from this backup".into() } else { diff }
            }).unwrap_or_else(|| "edit this config to create a backup, then return here to compare versions".into());
            let diff_text: Vec<Line> = diff_lines.lines().map(|line| {
                let style = if line.starts_with('+') && !line.starts_with("+++") {
                    Style::default().fg(Color::Green)
                } else if line.starts_with('-') && !line.starts_with("---") {
                    Style::default().fg(Color::Red)
                } else if line.starts_with("@@") || line.starts_with("---") || line.starts_with("+++") {
                    colors.group
                } else { colors.normal };
                Line::from(Span::styled(line.to_string(), style))
            }).collect();
            f.render_widget(Paragraph::new(diff_text).block(Block::default().title(" current vs selected backup ").borders(Borders::ALL))
                .scroll((scroll, 0)).wrap(Wrap { trim: false }), panels[1]);
            f.render_widget(Paragraph::new(" j/k choose backup · pgup/pgdn scroll · r restore · q/esc close ")
                .style(colors.group).alignment(Alignment::Center), layout[2]);
        })?;
        let Event::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press { continue; }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => break,
            KeyCode::Down | KeyCode::Char('j') if !snapshots.is_empty() => {
                selected = (selected + 1).min(snapshots.len() - 1);
                scroll = 0;
            }
            KeyCode::Up | KeyCode::Char('k') if !snapshots.is_empty() => {
                selected = selected.saturating_sub(1);
                scroll = 0;
            }
            KeyCode::PageDown => scroll = scroll.saturating_add(10),
            KeyCode::PageUp => scroll = scroll.saturating_sub(10),
            KeyCode::Char('r') if !snapshots.is_empty() => {
                match app.restore_backup(tracked, selected) {
                    Ok(message) => {
                        app.popup = Some(message);
                        break;
                    }
                    Err(error) => {
                        app.popup = Some(error.to_string());
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn run_hooks_editor<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    let path = match crate::hooks::ensure_file() {
        Ok(path) => path,
        Err(error) => {
            app.popup = Some(error.to_string());
            return Ok(());
        }
    };
    drop_alt(terminal)?;
    let editor_result = std::process::Command::new(app.editor()).arg(&path).status();
    restore_alt(terminal)?;
    match editor_result {
        Ok(_) => app.popup = Some("hooks.toml opened in editor".into()),
        Err(error) => app.popup = Some(format!("couldn't launch editor: {error}")),
    }
    Ok(())
}

fn draw_stats(f: &mut ratatui::Frame, app: &App, sort_by_count: bool) {
    let area = f.area();
    let colors = Colors::from_app(app);
    let accent = resolve_color(&app.cfg.settings.colors.highlight);
    let glow = Style::default().fg(accent).add_modifier(Modifier::BOLD);
    let bg = Style::default().bg(resolve_color(&app.cfg.settings.colors.bg));
    f.render_widget(Block::default().style(bg), area);
    let outer = Block::default().title(" ✦ confy / stats ✦ ").title_style(glow)
        .borders(Borders::ALL).border_style(Style::default().fg(accent));
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    let rows = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Length(5), Constraint::Min(7), Constraint::Length(5), Constraint::Length(1)])
        .split(inner);

    let tracked: usize = app.cfg.groups.values().map(Vec::len).sum();
    let total_edits = app.cfg.stats.edits;
    let top = app.cfg.stats.edited_files.iter()
        .max_by(|(pa, ca), (pb, cb)| ca.cmp(cb).then_with(|| pb.cmp(pa)));
    let top_name = top.map(|(p, _)| Path::new(p).file_name().unwrap_or_default().to_string_lossy().into_owned()).unwrap_or_else(|| "no edits yet".into());
    let (since_date, since_age) = confy_since();
    let metrics = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(20), Constraint::Percentage(20), Constraint::Percentage(20), Constraint::Percentage(20), Constraint::Percentage(20)])
        .split(rows[0]);
    let cards = [
        ("tracked", tracked.to_string(), "configs in orbit"),
        ("edits", total_edits.to_string(), "content changes"),
        ("rollbacks", app.cfg.stats.rollbacks.to_string(), "rescues deployed"),
        ("themes", app.cfg.stats.themes.len().to_string(), "looks collected"),
        ("using confy since", since_date, &since_age),
    ];
    for (i, (title, value, hint)) in cards.iter().enumerate() {
        let block = Block::default().title(format!(" {title} ")).borders(Borders::ALL)
            .border_style(Style::default().fg(if i % 2 == 0 { accent } else { resolve_color(&app.cfg.settings.colors.group) }));
        let lines = vec![Line::from(Span::styled(value.as_str(), Style::default().fg(accent).add_modifier(Modifier::BOLD))),
            Line::from(Span::styled(*hint, colors.group))];
        f.render_widget(Paragraph::new(lines).alignment(Alignment::Center).block(block), metrics[i]);
    }

    let activity_panels = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)]).split(rows[1]);
    let bar = Block::default().title(" ✦ signature config ").borders(Borders::ALL).border_style(Style::default().fg(accent));
    let bar_inner = bar.inner(activity_panels[0]);
    f.render_widget(bar, activity_panels[0]);
    let top_count = top.map(|(_, n)| *n).unwrap_or(0);
    let width = bar_inner.width.saturating_sub(28) as usize;
    let filled = if top_count == 0 { 0 } else { width.min((width as u64 * top_count / total_edits.max(1)) as usize) };
    let signature = vec![
        Line::from(vec![Span::styled(format!("  {top_name}  "), glow), Span::styled("most edited", colors.group)]),
        Line::from(vec![Span::styled("  ", colors.normal), Span::styled("█".repeat(filled), Style::default().fg(accent)), Span::styled("░".repeat(width.saturating_sub(filled)), Style::default().fg(resolve_color(&app.cfg.settings.colors.group))), Span::styled(format!("  {} edits", top.map(|(_, n)| *n).unwrap_or(0)), colors.normal)]),
    ];
    f.render_widget(Paragraph::new(signature), bar_inner);

    let (streak, busiest) = edit_activity(&app.cfg.stats);
    let active_block = Block::default().title(" ✦ editing rhythm ").borders(Borders::ALL).border_style(Style::default().fg(accent));
    let active_lines = vec![
        Line::from(vec![Span::styled("streak  ", colors.group), Span::styled(format!("{streak} {}", if streak == 1 { "day" } else { "days" }), glow)]),
        Line::from(vec![Span::styled("most active  ", colors.group), Span::styled(busiest.map(|(day, count)| format!("{day} · {count} edits")).unwrap_or_else(|| "no edits yet".into()), glow)]),
    ];
    f.render_widget(Paragraph::new(active_lines).block(active_block), activity_panels[1]);

    let panels = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(58), Constraint::Percentage(42)]).split(rows[2]);
    let mut files: Vec<_> = app.cfg.stats.edited_files.iter().collect();
    if sort_by_count { files.sort_by(|(pa, ca), (pb, cb)| cb.cmp(ca).then_with(|| pa.cmp(pb))); }
    else { files.sort_by(|(pa, _), (pb, _)| pa.cmp(pb)); }
    let file_rows: Vec<Row> = files.iter().take(panels[0].height.saturating_sub(3) as usize).map(|(path, count)| {
        let name = Path::new(path).file_name().unwrap_or_default().to_string_lossy();
        let pct = if total_edits == 0 { 0 } else { (**count * 100 / total_edits) as usize };
        let bw = panels[0].width.saturating_sub(28) as usize;
        let fill = bw.saturating_mul(pct) / 100;
        Row::new(vec![Cell::from(name.to_string()), Cell::from("█".repeat(fill)).style(Style::default().fg(accent)), Cell::from(format!("{count} · {pct}%"))])
    }).collect();
    let header = Row::new(vec!["config", "share", "edits"]).style(glow);
    let table = Table::new(file_rows, [Constraint::Percentage(48), Constraint::Percentage(32), Constraint::Percentage(20)])
        .header(header).block(Block::default().title(format!(" ✦ configs · sort: {} (s) ", if sort_by_count { "edits" } else { "name" })).borders(Borders::ALL).border_style(Style::default().fg(accent)));
    f.render_widget(table, panels[0]);

    let right = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Percentage(64), Constraint::Percentage(36)]).split(panels[1]);
    let mut opened: Vec<_> = app.cfg.stats.opened_files.iter().collect();
    opened.sort_by(|(pa, ca), (pb, cb)| cb.cmp(ca).then_with(|| pa.cmp(pb)));
    let max_opened = opened.first().map(|(_, count)| **count).unwrap_or(0).max(1);
    let rank_rows: Vec<Row> = opened.iter().take(right[0].height.saturating_sub(2) as usize).enumerate().map(|(index, (path, count))| {
        let name = Path::new(path).file_name().unwrap_or_default().to_string_lossy();
        let bar_width = right[0].width.saturating_sub(22) as usize;
        let fill = (bar_width as u64 * **count / max_opened) as usize;
        let rank = format!("#{:02}", index + 1);
        Row::new(vec![Cell::from(rank).style(if index == 0 { glow } else { colors.group }),
            Cell::from(name.to_string()),
            Cell::from("█".repeat(fill)).style(Style::default().fg(accent)),
            Cell::from(count.to_string())])
    }).collect();
    let rank_header = Row::new(vec!["rank", "config", "", "opens"]).style(glow);
    let leaderboard = Table::new(rank_rows, [Constraint::Length(5), Constraint::Percentage(42), Constraint::Percentage(35), Constraint::Length(6)])
        .header(rank_header).block(Block::default().title(" ✦ configs most opened ").borders(Borders::ALL).border_style(Style::default().fg(accent)));
    f.render_widget(leaderboard, right[0]);

    let mut theme_lines = vec![Line::from(Span::styled("themes worn", glow)), Line::from(Span::styled("────────────", colors.group))];
    if app.cfg.stats.themes.is_empty() { theme_lines.push(Line::from(Span::styled("no theme switches yet", colors.normal))); }
    for (i, name) in app.cfg.stats.themes.iter().enumerate() {
        let color = if i % 3 == 0 { accent } else if i % 3 == 1 { resolve_color(&app.cfg.settings.colors.group) } else { resolve_color(&app.cfg.settings.colors.fg) };
        theme_lines.push(Line::from(vec![Span::styled("◆ ", Style::default().fg(color)), Span::styled(name, Style::default().fg(color).add_modifier(Modifier::BOLD))]));
    }
    let theme_block = Block::default().title(" ✦ palette ").borders(Borders::ALL).border_style(Style::default().fg(accent));
    f.render_widget(Paragraph::new(theme_lines).block(theme_block).wrap(Wrap { trim: true }), right[1]);
    let milestone_lines: Vec<Line> = ACHIEVEMENTS.chunks(6).map(|chunk| {
        let mut spans = Vec::new();
        for (id, label, _) in chunk {
            let unlocked = app.cfg.stats.milestones.iter().any(|milestone| milestone == id);
            let style = if unlocked { glow } else { colors.group };
            spans.push(Span::styled(format!("{} {label}", if unlocked { "◆" } else { "◇" }), style));
            spans.push(Span::raw("   "));
        }
        Line::from(spans)
    }).collect();
    let milestone_block = Block::default().title(format!(" ✦ milestones · {}/{} unlocked · m for full wall ✦ ", app.cfg.stats.milestones.len().min(ACHIEVEMENTS.len()), ACHIEVEMENTS.len()))
        .borders(Borders::ALL).border_style(Style::default().fg(accent));
    f.render_widget(Paragraph::new(milestone_lines).block(milestone_block), rows[3]);
    f.render_widget(Paragraph::new(" s sort  ·  m milestone wall  ·  q / esc close ").style(colors.group).alignment(Alignment::Center), rows[4]);
}

fn draw_popup(f: &mut ratatui::Frame, msg: &str) {
    let area = f.area();
    let width = (msg.len() as u16 + 6).min(area.width.saturating_sub(4));
    let height = 3u16;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup_area = Rect {
        x,
        y,
        width,
        height,
    };
    f.render_widget(Clear, popup_area);
    f.render_widget(
        Paragraph::new(Line::from(format!("  {msg}  ")))
            .block(Block::default().borders(Borders::ALL))
            .alignment(Alignment::Center)
            .style(Style::default().add_modifier(Modifier::REVERSED)),
        popup_area,
    );
}

fn run_tutorial<B: Backend>(terminal: &mut Terminal<B>, app: &App) -> Result<()> {
    let total = tutorial::STEPS.len();
    for (i, step) in tutorial::STEPS.iter().enumerate() {
        terminal.draw(|f| {
            let area = f.area();
            let colors = Colors::from_app(app);
            let content_w = 64u16.min(area.width.saturating_sub(4));
            let content_h = (step.lines.len() as u16 + 4).min(area.height.saturating_sub(2));
            let x = area.x + (area.width.saturating_sub(content_w)) / 2;
            let y = area.y + (area.height.saturating_sub(content_h)) / 2;
            let popup_area = Rect {
                x,
                y,
                width: content_w,
                height: content_h,
            };
            f.render_widget(Clear, popup_area);
            let lines: Vec<Line> = step
                .lines
                .iter()
                .map(|l| Line::from(Span::styled(*l, colors.normal)))
                .collect();
            f.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {} ", step.title))
                        .title_alignment(Alignment::Center),
                ),
                popup_area,
            );
            let counter = format!(" {}/{total} ", i + 1);
            let cx = popup_area.x + popup_area.width.saturating_sub(counter.len() as u16 + 1);
            let cy = popup_area.y + popup_area.height - 1;
            if cx < area.right() && cy < area.bottom() {
                f.render_widget(
                    Paragraph::new(Span::styled(counter, colors.group)),
                    Rect {
                        x: cx,
                        y: cy,
                        width: popup_area.width / 2,
                        height: 1,
                    },
                );
            }
        })?;
        event::read()?;
    }
    Ok(())
}

fn drop_alt<B: Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(std::io::stderr(), crossterm::terminal::LeaveAlternateScreen)?;
    Ok(())
}

fn restore_alt<B: Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(std::io::stderr(), crossterm::terminal::EnterAlternateScreen)?;
    terminal.clear()?;
    terminal.draw(|f| f.render_widget(ratatui::widgets::Clear, f.area()))?;
    Ok(())
}

// ── bouncer (bulk blacklist menu) ─────────────────────────────────────────────

fn run_bouncer<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    use crate::app::Item;
    use crate::git;

    let items: Vec<(String, String, bool)> = app
        .flat_view
        .iter()
        .filter_map(|item| match item {
            Item::Group(name) if name != "ungrouped" => {
                Some((format!("  [group] {name}/"), name.clone(), true))
            }
            Item::File { path, .. } => {
                let fname = std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                Some((format!("  {fname}"), path.clone(), false))
            }
            _ => None,
        })
        .collect();

    let mut selected: Vec<bool> = items
        .iter()
        .map(|(_, key, is_group)| {
            if *is_group {
                git::is_group_blacklisted(app, key)
            } else {
                git::is_blacklisted(app, key)
            }
        })
        .collect();

    let mut cursor = 0usize;
    let mut bouncer_state = ListState::default();

    loop {
        bouncer_state.select(Some(cursor));
        terminal.draw(|f| {
            let area = f.area();
            let colors = Colors::from_app(app);

            let list_items: Vec<ListItem> = items
                .iter()
                .enumerate()
                .map(|(i, (label, _, _))| {
                    let marker = if selected[i] { "✖ " } else { "  " };
                    let style = if selected[i] {
                        colors.error
                    } else {
                        colors.normal
                    };
                    ListItem::new(Line::from(Span::styled(format!("{marker}{label}"), style)))
                })
                .collect();

            let list = List::new(list_items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" bouncer. git blacklist  space=toggle  z=confirm  esc=cancel "),
                )
                .highlight_style(colors.highlight)
                .highlight_symbol("▶ ");

            f.render_stateful_widget(list, area, &mut bouncer_state);
        })?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    if cursor + 1 < items.len() {
                        cursor += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if cursor > 0 {
                        cursor -= 1;
                    }
                }
                KeyCode::Char(' ') => {
                    selected[cursor] = !selected[cursor];
                }
                KeyCode::Char('z') => {
                    for (i, (_, key, is_group)) in items.iter().enumerate() {
                        if *is_group {
                            if selected[i] {
                                git::blacklist_group(app, key);
                            } else {
                                git::unblacklist_group(app, key);
                            }
                        } else {
                            if selected[i] {
                                git::blacklist_file(app, key);
                            } else {
                                git::unblacklist_file(app, key);
                            }
                        }
                    }
                    let count = selected.iter().filter(|&&s| s).count();
                    app.popup = Some(format!(
                        "blacklist updated. {count} item(s) excluded from :git sync"
                    ));
                    break;
                }
                KeyCode::Esc => break,
                _ => {}
            }
        }
    }
    Ok(())
}

// ── theme creation wizard ─────────────────────────────────────────────────────

const COLOR_FIELDS: [(&str, &str); 5] = [
    ("bg", "background"),
    ("fg", "foreground"),
    ("highlight", "selection"),
    ("group", "group headers"),
    ("border", "borders"),
];

#[derive(PartialEq, Clone, Copy)]
enum WizardStep {
    Name,
    Template,
    Base,
    Colors,
}

struct ThemeWizard {
    step: WizardStep,
    name: String,
    base: Option<String>,
    colors: ColorSettings,
    field: usize,
    editing: bool,
    edit_buf: String,
    error: Option<String>,
    cursor: usize,
}

impl ThemeWizard {
    fn new() -> Self {
        Self {
            step: WizardStep::Name,
            name: String::new(),
            base: None,
            colors: ColorSettings::default(),
            field: 0,
            editing: false,
            edit_buf: String::new(),
            error: None,
            cursor: 0,
        }
    }

    fn field_value(&self, i: usize) -> &str {
        match i {
            0 => &self.colors.bg,
            1 => &self.colors.fg,
            2 => &self.colors.highlight,
            3 => &self.colors.group,
            _ => &self.colors.border,
        }
    }

    fn set_field_value(&mut self, i: usize, v: String) {
        match i {
            0 => self.colors.bg = v,
            1 => self.colors.fg = v,
            2 => self.colors.highlight = v,
            3 => self.colors.group = v,
            _ => self.colors.border = v,
        }
    }
}

fn theme_name_list() -> Vec<String> {
    let mut names: Vec<String> = config::builtin_themes()
        .keys()
        .cloned()
        .map(String::from)
        .collect();
    names.sort();
    for n in config::custom_theme_names() {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    names
}

fn theme_colors(name: &str) -> Option<ColorSettings> {
    if let Some(cs) = config::builtin_themes().get(name) {
        return Some(cs.clone());
    }
    config::load_custom_themes().get(name).cloned()
}

fn run_themenet<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    terminal.clear()?;
    let themes = match crate::themenet::fetch_themes() {
        Ok(themes) => themes,
        Err(e) => {
            app.popup = Some(format!("couldn't load community themes: {e:#}"));
            return Ok(());
        }
    };
    let mut selected = 0usize;
    let mut file = 0usize;
    let mut scroll = 0u16;
    loop {
        let colors = Colors::from_settings(&themes[selected].colors);
        terminal.draw(|f| draw_themenet(f, &themes, selected, file, scroll, &colors))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => break,
            KeyCode::Up | KeyCode::Char('k') => {
                selected = selected.saturating_sub(1);
                scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                selected = (selected + 1).min(themes.len() - 1);
                scroll = 0;
            }
            KeyCode::Tab | KeyCode::Char('f') => {
                file = (file + 1) % 2;
                scroll = 0;
            }
            KeyCode::PageDown => scroll = scroll.saturating_add(10),
            KeyCode::PageUp => scroll = scroll.saturating_sub(10),
            KeyCode::Enter => {
                let theme = &themes[selected];
                match config::install_custom_theme(&theme.id, &theme.theme_toml)
                    .and_then(|_| app.set_theme(&theme.id))
                {
                    Ok(()) => {
                        app.unlock_milestone("thrift_shop");
                        app.save();
                        app.popup = Some(format!("installed and applied '{}'", theme.meta.name));
                        break;
                    }
                    Err(e) => app.popup = Some(format!("couldn't install theme: {e}")),
                }
            }
            _ => {}
        }
    }
    terminal.clear()?;
    Ok(())
}

fn draw_themenet(
    f: &mut ratatui::Frame,
    themes: &[crate::themenet::RemoteTheme],
    selected: usize,
    file: usize,
    scroll: u16,
    colors: &Colors,
) {
    let area = f.area();
    let outer = Block::default()
        .title(" community themes ")
        .borders(Borders::ALL)
        .border_style(colors.group);
    f.render_widget(outer, area);
    let inner = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(inner);
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(32), Constraint::Percentage(68)])
        .split(rows[0]);
    let items: Vec<ListItem> = themes
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let label = format!("{}  [{}]", t.meta.name, t.id);
            ListItem::new(label).style(if i == selected {
                colors.highlight
            } else {
                colors.normal
            })
        })
        .collect();
    f.render_widget(
        List::new(items).block(
            Block::default()
                .title(" themes ")
                .borders(Borders::ALL)
                .border_style(colors.group),
        ),
        panes[0],
    );

    let theme = &themes[selected];
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(0)])
        .split(panes[1]);
    let mut meta = vec![Line::from(Span::styled(&theme.meta.name, colors.group))];
    meta.push(Line::from(format!(
        "by {}{}",
        if theme.meta.author.is_empty() {
            "unknown author"
        } else {
            &theme.meta.author
        },
        theme
            .meta
            .link
            .as_deref()
            .map(|l| format!(" · {l}"))
            .unwrap_or_default()
    )));
    meta.push(Line::from(format!(
        "tags: {}",
        if theme.meta.tags.is_empty() {
            "none".into()
        } else {
            theme.meta.tags.join(", ")
        }
    )));
    meta.push(Line::from(theme.meta.description.clone()));
    f.render_widget(
        Paragraph::new(meta).wrap(Wrap { trim: false }).block(
            Block::default()
                .title(" details ")
                .borders(Borders::ALL)
                .border_style(colors.group),
        ),
        right[0],
    );

    let (title, content) = if file == 0 {
        ("theme.toml", &theme.theme_toml)
    } else {
        ("meta.toml", &theme.meta_toml)
    };
    f.render_widget(
        Paragraph::new(content.as_str())
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title(format!(" {title} (tab: switch file) "))
                    .borders(Borders::ALL)
                    .border_style(colors.group),
            ),
        right[1],
    );
    f.render_widget(
        Paragraph::new(
            "↑/↓ select · tab switch file · pgup/pgdn scroll · enter install and apply · esc back",
        ),
        rows[1],
    );
}

fn run_theme_wizard<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    let mut wiz = ThemeWizard::new();
    let theme_names = theme_name_list();

    loop {
        terminal.draw(|f| {
            let colors = Colors::from_app(app);
            draw_theme_wizard(f, &wiz, &theme_names, &colors);
        })?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match wiz.step {
            WizardStep::Name => match key.code {
                KeyCode::Enter => {
                    let trimmed = wiz.name.trim().to_string();
                    if trimmed.is_empty() {
                        wiz.error = Some("theme name cannot be empty".into());
                    } else if trimmed.chars().any(char::is_whitespace) {
                        wiz.error = Some("theme names can't contain spaces".into());
                    } else if config::builtin_themes().contains_key(trimmed.as_str()) {
                        wiz.error = Some("that's a built-in theme name, pick another".into());
                    } else if config::is_reserved_theme_name(&trimmed) {
                        wiz.error = Some("that name is reserved for a theme command".into());
                    } else {
                        wiz.name = trimmed;
                        wiz.error = None;
                        wiz.step = WizardStep::Template;
                    }
                }
                KeyCode::Backspace => {
                    wiz.name.pop();
                    wiz.error = None;
                }
                KeyCode::Char(c) => {
                    wiz.name.push(c);
                    wiz.error = None;
                }
                KeyCode::Esc => {
                    app.popup = Some("theme creation cancelled".into());
                    return Ok(());
                }
                _ => {}
            },

            WizardStep::Template => match key.code {
                KeyCode::Char('b') | KeyCode::Char('B') => {
                    wiz.error = None;
                    wiz.step = WizardStep::Base;
                }
                KeyCode::Char('l') | KeyCode::Char('L') => {
                    wiz.colors = ColorSettings::default();
                    wiz.base = None;
                    wiz.field = 0;
                    wiz.error = None;
                    wiz.step = WizardStep::Colors;
                }
                KeyCode::Esc => {
                    wiz.step = WizardStep::Name;
                }
                _ => {}
            },

            WizardStep::Base => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    if wiz.cursor + 1 < theme_names.len() {
                        wiz.cursor += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if wiz.cursor > 0 {
                        wiz.cursor -= 1;
                    }
                }
                KeyCode::Enter => {
                    if let Some(name) = theme_names.get(wiz.cursor).cloned() {
                        if let Some(cs) = theme_colors(&name) {
                            wiz.colors = cs;
                            wiz.base = Some(name);
                            wiz.field = 0;
                            wiz.error = None;
                            wiz.step = WizardStep::Colors;
                        }
                    }
                }
                KeyCode::Esc => {
                    wiz.step = WizardStep::Template;
                }
                _ => {}
            },

            WizardStep::Colors => {
                if wiz.editing {
                    match key.code {
                        KeyCode::Enter => {
                            wiz.set_field_value(wiz.field, wiz.edit_buf.trim().to_string());
                            wiz.editing = false;
                            wiz.error = None;
                        }
                        KeyCode::Esc => {
                            wiz.editing = false;
                        }
                        KeyCode::Backspace => {
                            wiz.edit_buf.pop();
                        }
                        KeyCode::Char(c) => {
                            wiz.edit_buf.push(c);
                        }
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Char('j') | KeyCode::Down => {
                            if wiz.field + 1 < COLOR_FIELDS.len() {
                                wiz.field += 1;
                            }
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            if wiz.field > 0 {
                                wiz.field -= 1;
                            }
                        }
                        KeyCode::Enter => {
                            wiz.edit_buf = wiz.field_value(wiz.field).to_string();
                            wiz.editing = true;
                        }
                        KeyCode::Char('s') | KeyCode::Char('S') => {
                            let first_custom_theme = config::custom_theme_names().is_empty();
                            match config::save_custom_theme(&wiz.name, &wiz.colors) {
                                Ok(_) => {
                                    if first_custom_theme { app.unlock_milestone("self_sewed"); }
                                    match app.set_theme(&wiz.name) {
                                        Ok(_) => {
                                            app.popup = Some(format!(
                                                "theme '{}' saved and applied!",
                                                wiz.name
                                            ))
                                        }
                                        Err(e) => {
                                            app.popup = Some(format!(
                                                "theme saved, but couldn't apply: {e}"
                                            ))
                                        }
                                    }
                                    return Ok(());
                                }
                                Err(e) => wiz.error = Some(e.to_string()),
                            }
                        }
                        KeyCode::Esc => {
                            wiz.step = WizardStep::Template;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn draw_theme_wizard(
    f: &mut ratatui::Frame,
    wiz: &ThemeWizard,
    theme_names: &[String],
    colors: &Colors,
) {
    let area = f.area();
    let width = 64u16.min(area.width.saturating_sub(4));
    let height = 18u16.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect {
        x,
        y,
        width,
        height,
    };
    f.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" new theme ")
        .title_alignment(Alignment::Center);
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    match wiz.step {
        WizardStep::Name => {
            let mut lines = vec![
                Line::from(Span::styled("step 1 of 3: name your theme", colors.group)),
                Line::from(""),
                Line::from(Span::styled(format!("> {}_", wiz.name), colors.normal)),
                Line::from(""),
            ];
            if let Some(err) = &wiz.error {
                lines.push(Line::from(Span::styled(err.clone(), colors.error)));
            } else {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "enter: continue    esc: cancel",
                colors.normal,
            )));
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }

        WizardStep::Template => {
            let lines = vec![
                Line::from(Span::styled(
                    "step 2 of 3: start from a base or blank?",
                    colors.group,
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  [b]  start from a base theme",
                    colors.normal,
                )),
                Line::from(Span::styled(
                    "  [l]  start blank (default colors)",
                    colors.normal,
                )),
                Line::from(""),
                Line::from(Span::styled("esc: back", colors.normal)),
            ];
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }

        WizardStep::Base => {
            let mut lines = vec![
                Line::from(Span::styled(
                    "step 2 of 3: choose a base theme",
                    colors.group,
                )),
                Line::from(""),
            ];
            let visible = (inner.height as usize).saturating_sub(5).max(1);
            let start = wiz.cursor.saturating_sub(visible / 2);
            for (i, name) in theme_names.iter().enumerate().skip(start).take(visible) {
                let marker = if i == wiz.cursor { "▶ " } else { "  " };
                let style = if i == wiz.cursor {
                    colors.highlight
                } else {
                    colors.normal
                };
                lines.push(Line::from(Span::styled(format!("{marker}{name}"), style)));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "j/k: move    enter: select    esc: back",
                colors.normal,
            )));
            f.render_widget(Paragraph::new(lines), inner);
        }

        WizardStep::Colors => {
            let title = match &wiz.base {
                Some(b) => format!("step 3 of 3: edit colors (base: {b})"),
                None => "step 3 of 3: edit colors (blank)".to_string(),
            };
            let mut lines = vec![
                Line::from(Span::styled(title, colors.group)),
                Line::from(""),
            ];
            for (i, (key, label)) in COLOR_FIELDS.iter().enumerate() {
                let marker = if i == wiz.field && !wiz.editing {
                    "▶ "
                } else {
                    "  "
                };
                let val = if i == wiz.field && wiz.editing {
                    format!("{}_", wiz.edit_buf)
                } else {
                    wiz.field_value(i).to_string()
                };
                let style = if i == wiz.field {
                    colors.highlight
                } else {
                    colors.normal
                };
                lines.push(Line::from(Span::styled(
                    format!("{marker}{:<10} {:<26} ({})", key, val, label),
                    style,
                )));
            }
            lines.push(Line::from(""));
            // live preview using the in-progress colors
            let pv = Colors::from_settings(&wiz.colors);
            lines.push(Line::from(vec![
                Span::styled("preview  ", colors.normal),
                Span::styled("configs/ ", pv.group),
                Span::styled("settings.toml", pv.highlight),
                Span::styled("  ", pv.normal),
                Span::styled("bashrc", pv.normal),
            ]));
            lines.push(Line::from(""));
            if let Some(err) = &wiz.error {
                lines.push(Line::from(Span::styled(err.clone(), colors.error)));
            } else if wiz.editing {
                lines.push(Line::from(Span::styled(
                    "enter: apply    esc: cancel edit",
                    colors.normal,
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "j/k: move  enter: edit  s: save + apply  esc: back",
                    colors.normal,
                )));
            }
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
    }
}

// ── simple line prompt (used for commit message) ──────────────────────────────

fn prompt_line(prompt: &str) -> Result<String> {
    use std::io::{BufRead, Write};
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut buf = String::new();
    std::io::stdin().lock().read_line(&mut buf)?;
    Ok(buf.trim_end_matches('\n').to_string())
}
