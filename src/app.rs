use crate::config::{self, AppConfig, builtin_themes, confy_config_dir, confy_config_file};
use crate::device::{self, MountInfo, RemoteFormat};
use anyhow::Result;
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// ── flat view item ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Item {
    Group(String),
    File { path: String, group: String },
}

#[derive(Debug, Clone)]
pub struct BackupSnapshot {
    pub label: String,
    pub contents: Vec<u8>,
}

fn backup_key(path: &str) -> String {
    // stable fnv-1a key for the complete path, avoiding basename collisions.
    let hash = path.as_bytes().iter().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

// ── app state ─────────────────────────────────────────────────────────────────

pub struct App {
    pub cfg: AppConfig,

    // active config location (repointed when a device is mounted)
    pub active_config_dir: PathBuf,
    pub active_config_file: PathBuf,

    // device mode
    pub mount: Option<MountInfo>,
    pub device_name: Option<String>,
    local_state: Option<Box<AppConfig>>,

    // view state
    pub flat_view: Vec<Item>,
    pub selected: usize,
    pub scroll: usize,

    // ui state
    pub popup: Option<String>,
    pub show_tutorial: bool,
}

impl App {
    pub fn new() -> Self {
        let config_dir = confy_config_dir();
        let config_file = confy_config_file();

        config::migrate_if_needed(&config_dir);

        let (cfg, first) = config::load(&config_file).unwrap_or_else(|e| {
            eprintln!("warn: {e}");
            (AppConfig::default(), true)
        });

        let mut app = Self {
            cfg,
            active_config_dir: config_dir,
            active_config_file: config_file,
            mount: None,
            device_name: None,
            local_state: None,
            flat_view: vec![],
            selected: 0,
            scroll: 0,
            popup: None,
            show_tutorial: first,
        };
        let mut milestone_change = app.refresh_milestones();
        if first { milestone_change |= app.add_milestone("welcome_to_confy"); }
        if milestone_change { app.save(); }
        app.rebuild_flat_view();
        app
    }

    // ── persistence ───────────────────────────────────────────────────────────

    pub fn save(&self) {
        let _ = config::save(&self.active_config_file, &self.cfg);
    }

    // ── path resolution ───────────────────────────────────────────────────────

    /// tracked path → local filesystem path (transparent on local; rewritten on device)
    pub fn local_path(&self, tracked: &str) -> PathBuf {
        match &self.mount {
            Some(m) => m.to_local_path(tracked),
            None => PathBuf::from(tracked),
        }
    }

    /// local filesystem path → tracked path (reverse of local_path)
    pub fn tracked_path(&self, local: &Path) -> String {
        match &self.mount {
            Some(m) => m.to_remote_path(local),
            None => local.to_string_lossy().into_owned(),
        }
    }

    // ── flat view ─────────────────────────────────────────────────────────────

    pub fn rebuild_flat_view(&mut self) {
        self.flat_view.clear();
        let mut group_names: Vec<_> = self.cfg.groups.keys().cloned().collect();
        // ungrouped always goes last regardless of sort mode
        group_names.sort_by(|a, b| match (a.as_str(), b.as_str()) {
            ("ungrouped", _) => std::cmp::Ordering::Greater,
            (_, "ungrouped") => std::cmp::Ordering::Less,
            _ => a.cmp(b),
        });
        for group_name in group_names {
            self.flat_view.push(Item::Group(group_name.clone()));
            if !self.cfg.collapsed_groups.contains(&group_name) {
                let mut files = self.cfg.groups[&group_name].clone();
                self.sort_files(&mut files);
                for f in files {
                    self.flat_view.push(Item::File {
                        path: f,
                        group: group_name.clone(),
                    });
                }
            }
        }
    }

    pub fn rebuild_flat_view_filtered(&mut self, query: &str) {
        self.flat_view.clear();
        let q = query.to_lowercase();
        let mut group_names: Vec<_> = self.cfg.groups.keys().cloned().collect();
        group_names.sort_by(|a, b| match (a.as_str(), b.as_str()) {
            ("ungrouped", _) => std::cmp::Ordering::Greater,
            (_, "ungrouped") => std::cmp::Ordering::Less,
            _ => a.cmp(b),
        });
        for group_name in group_names {
            let files: Vec<_> = self.cfg.groups[&group_name]
                .iter()
                .filter(|f| {
                    let name = Path::new(f)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    name.contains(&q) || group_name.to_lowercase().contains(&q)
                })
                .cloned()
                .collect();

            if !files.is_empty() || group_name.to_lowercase().contains(&q) {
                self.flat_view.push(Item::Group(group_name.clone()));
                if !self.cfg.collapsed_groups.contains(&group_name) {
                    let mut sorted = files;
                    self.sort_files(&mut sorted);
                    for f in sorted {
                        self.flat_view.push(Item::File {
                            path: f,
                            group: group_name.clone(),
                        });
                    }
                }
            }
        }
    }

    fn sort_files(&self, files: &mut Vec<String>) {
        match self.cfg.sort_mode.as_str() {
            "date" => files.sort_by_key(|f| {
                self.local_path(f)
                    .metadata()
                    .and_then(|m| m.modified())
                    .map(|t| t.elapsed().unwrap_or_default())
                    .unwrap_or_default()
            }),
            "size" => {
                files.sort_by_key(|f| self.local_path(f).metadata().map(|m| m.len()).unwrap_or(0))
            }
            _ => files.sort_by(|a, b| {
                let an = Path::new(a)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let bn = Path::new(b)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                an.cmp(&bn)
            }),
        }
        if self.cfg.sort_order == "desc" {
            files.reverse();
        }
    }

    // ── selected item helpers ─────────────────────────────────────────────────

    pub fn selected_item(&self) -> Option<&Item> {
        self.flat_view.get(self.selected)
    }

    pub fn selected_file(&self) -> Option<&str> {
        match self.selected_item()? {
            Item::File { path, .. } => Some(path),
            _ => None,
        }
    }

    // ── navigation ────────────────────────────────────────────────────────────

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.scroll {
                self.scroll = self.selected;
            }
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.flat_view.len() {
            self.selected += 1;
        }
    }

    pub fn clamp_selected(&mut self) {
        if self.flat_view.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.flat_view.len() {
            self.selected = self.flat_view.len() - 1;
        }
    }

    // ── group ops ─────────────────────────────────────────────────────────────

    pub fn toggle_group(&mut self) {
        if let Some(Item::Group(name)) = self.selected_item().cloned() {
            if self.cfg.collapsed_groups.contains(&name) {
                self.cfg.collapsed_groups.remove(&name);
            } else {
                self.cfg.collapsed_groups.insert(name);
            }
            self.save();
            self.rebuild_flat_view();
        }
    }

    pub fn add_group(&mut self, name: &str) {
        if !name.is_empty() && !self.cfg.groups.contains_key(name) {
            self.cfg.groups.insert(name.into(), vec![]);
            self.save();
            self.rebuild_flat_view();
        }
    }

    pub fn remove_group(&mut self, name: &str) {
        if name == "ungrouped" {
            return;
        }
        if let Some(files) = self.cfg.groups.remove(name) {
            self.cfg
                .groups
                .entry("ungrouped".into())
                .or_default()
                .extend(files);
            self.save();
            self.rebuild_flat_view();
        }
    }

    pub fn move_to_group(&mut self, group_name: &str) {
        let Some(Item::File {
            path,
            group: old_group,
        }) = self.selected_item().cloned()
        else {
            return;
        };
        if !self.cfg.groups.contains_key(group_name) {
            self.cfg.groups.insert(group_name.into(), vec![]);
        }
        if let Some(files) = self.cfg.groups.get_mut(&old_group) {
            files.retain(|f| f != &path);
        }
        self.cfg
            .groups
            .entry(group_name.into())
            .or_default()
            .push(path);
        self.save();
        self.rebuild_flat_view();
        self.clamp_selected();
    }

    // ── file tracking ops ─────────────────────────────────────────────────────

    pub fn track_file(&mut self, local_picked: &Path, group: &str) -> bool {
        let tracked = self.tracked_path(local_picked);
        if self.cfg.groups.values().any(|v| v.contains(&tracked)) {
            self.popup = Some("already tracked!".into());
            return false;
        }
        self.cfg
            .groups
            .entry(group.into())
            .or_default()
            .push(tracked.clone());
        self.refresh_milestones();
        self.save();
        crate::hooks::run("tracked", &tracked);
        self.rebuild_flat_view();
        true
    }

    pub fn import_files(&mut self, paths: &[PathBuf]) -> usize {
        let mut added = 0;
        let mut added_paths = Vec::new();
        for path in paths {
            let tracked = self.tracked_path(path);
            if !self.cfg.groups.values().any(|files| files.contains(&tracked)) {
                self.cfg.groups.entry("ungrouped".into()).or_default().push(tracked.clone());
                added_paths.push(tracked);
                added += 1;
            }
        }
        if added > 0 {
            self.refresh_milestones();
            self.save();
            for path in added_paths { crate::hooks::run("tracked", &path); }
            self.rebuild_flat_view();
        }
        added
    }

    pub fn unlock_milestone(&mut self, name: &str) {
        if self.add_milestone(name) { self.save(); }
    }

    fn add_milestone(&mut self, name: &str) -> bool {
        if self.cfg.stats.milestones.iter().any(|milestone| milestone == name) { return false; }
        self.cfg.stats.milestones.push(name.to_string());
        true
    }

    pub fn remove_selected(&mut self) {
        let Some(Item::File { path, group }) = self.selected_item().cloned() else {
            return;
        };
        if let Some(files) = self.cfg.groups.get_mut(&group) {
            files.retain(|f| f != &path);
        }
        self.save();
        crate::hooks::run("untracked", &path);
        self.rebuild_flat_view();
        self.clamp_selected();
    }

    // ── rollback ──────────────────────────────────────────────────────────────

    pub fn save_backup(&self, tracked: &str) {
        if !self.cfg.settings.rollback {
            return;
        }
        let local = self.local_path(tracked);
        let Ok(data) = fs::read(&local) else {
            return;
        };
        let _ = Self::write_backup(tracked, &data);
    }

    pub fn backup_before_write(&self, tracked: &str) -> Result<()> {
        if !self.cfg.settings.rollback {
            return Ok(());
        }
        let local = self.local_path(tracked);
        if !local.exists() {
            return Ok(());
        }
        let data = fs::read(&local)?;
        Self::write_backup(tracked, &data)
    }

    fn write_backup(tracked: &str, data: &[u8]) -> Result<()> {
        let dir = config::backup_dir();
        fs::create_dir_all(&dir)?;
        let key = backup_key(tracked);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = dir.join(format!("{key}-{stamp}.confbak.gz"));
        let file = fs::File::create(&path)?;
        let mut gz = GzEncoder::new(file, Compression::default());
        gz.write_all(data)?;
        gz.finish()?;

        let mut backups: Vec<_> = fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(&format!("{key}-")) && n.ends_with(".confbak.gz"))
                    .unwrap_or(false)
            })
            .collect();
        backups.sort_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok());
        while backups.len() > 3 {
            let oldest = backups.remove(0);
            let _ = fs::remove_file(oldest);
        }
        Ok(())
    }

    pub fn rollback(&mut self, tracked: &str) -> Result<String> {
        self.restore_snapshot(tracked, 0, "rolled_back")?;
        Ok(format!(
            "rolled back {}!",
            Path::new(tracked)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ))
    }

    pub fn backup_snapshots(&self, tracked: &str) -> Result<Vec<BackupSnapshot>> {
        let dir = config::backup_dir();
        let key = backup_key(tracked);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(error) => return Err(error.into()),
        };
        let mut paths: Vec<(u128, PathBuf)> = entries.filter_map(Result::ok).filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let stamp = name.strip_prefix(&format!("{key}-"))?.strip_suffix(".confbak.gz")?.parse().ok()?;
            Some((stamp, path))
        }).collect();
        paths.sort_by(|left, right| right.0.cmp(&left.0));
        paths.into_iter().map(|(stamp, path)| {
            let file = fs::File::open(path)?;
            let mut gz = GzDecoder::new(file);
            let mut contents = Vec::new();
            gz.read_to_end(&mut contents)?;
            let seconds = (stamp / 1_000_000_000).min(u128::from(u64::MAX)) as u64;
            let label = format_unix_ts(seconds);
            Ok(BackupSnapshot { label, contents })
        }).collect()
    }

    pub fn restore_backup(&mut self, tracked: &str, index: usize) -> Result<String> {
        let snapshots = self.backup_snapshots(tracked)?;
        let snapshot = snapshots.get(index).ok_or_else(|| anyhow::anyhow!("backup no longer exists"))?;
        self.restore_contents(tracked, &snapshot.contents, "restored")?;
        Ok(format!("restored {} from {}", Path::new(tracked).file_name().unwrap_or_default().to_string_lossy(), snapshot.label))
    }

    fn restore_snapshot(&mut self, tracked: &str, index: usize, event: &str) -> Result<()> {
        let snapshots = self.backup_snapshots(tracked)?;
        let snapshot = snapshots.get(index).ok_or_else(|| anyhow::anyhow!("no backup found for {}", Path::new(tracked).display()))?;
        self.restore_contents(tracked, &snapshot.contents, event)
    }

    fn restore_contents(&mut self, tracked: &str, contents: &[u8], event: &str) -> Result<()> {
        let local = self.local_path(tracked);
        self.backup_before_write(tracked)?;
        fs::write(&local, contents)?;
        self.cfg.stats.rollbacks += 1;
        self.add_milestone("oops");
        self.refresh_milestones();
        self.save();
        if event == "restored" { crate::hooks::run("rolled_back", tracked); }
        crate::hooks::run(event, tracked);
        Ok(())
    }

    // ── open file ─────────────────────────────────────────────────────────────

    /// resolve the editor to use: config.toml > $editor > nano
    pub fn editor(&self) -> String {
        self.cfg
            .settings
            .editor
            .clone()
            .filter(|e| !e.is_empty())
            .or_else(|| std::env::var("EDITOR").ok())
            .unwrap_or_else(|| "nano".into())
    }

    pub fn open_file(&mut self, tracked: &str) {
        self.save_backup(tracked);
        let local = self.local_path(tracked);
        let before = fs::read(&local).ok();
        let editor = self.editor();
        let _ = std::process::Command::new(&editor).arg(&local).status();
        self.record_edit_if_changed(tracked, before.as_deref(), &local);
        crate::hooks::run("opened", tracked);
        *self.cfg.stats.opened_files.entry(tracked.to_string()).or_default() += 1;
        self.cfg.last_opened = Some(tracked.into());
        self.save();
    }

    pub fn open_file_elevated(&mut self, tracked: &str) -> Result<()> {
        if !device::pkexec_available() {
            return Err(anyhow::anyhow!("pkexec not found. needs polkit"));
        }
        if let Some(m) = &self.mount {
            if !m.su_available {
                return Err(anyhow::anyhow!(
                    "can't :su through this mount. add 'user_allow_other' to /etc/fuse.conf and reconnect"
                ));
            }
        }
        self.save_backup(tracked);
        let local = self.local_path(tracked);
        let before = fs::read(&local).ok();
        let editor = self.editor();
        let editor_path = which_editor(&editor)
            .ok_or_else(|| anyhow::anyhow!("editor '{editor}' not found on path"))?;
        let status = std::process::Command::new("pkexec")
            .args([&editor_path, &local.to_string_lossy().into_owned()])
            .status()?;
        if !status.success() {
            return Err(anyhow::anyhow!(
                "pkexec exited with code {} (auth cancelled?)",
                status.code().unwrap_or(-1)
            ));
        }
        self.add_milestone("with_great_power");
        self.record_edit_if_changed(tracked, before.as_deref(), &local);
        crate::hooks::run("opened", tracked);
        *self.cfg.stats.opened_files.entry(tracked.to_string()).or_default() += 1;
        self.cfg.last_opened = Some(tracked.into());
        self.save();
        Ok(())
    }

    // ── themes ────────────────────────────────────────────────────────────────

    pub fn set_theme(&mut self, name: &str) -> Result<()> {
        let changed = self.cfg.settings.theme != name;
        // check built-in themes first
        if let Some(colors) = builtin_themes().get(name) {
            self.cfg.settings.theme = name.into();
            self.cfg.settings.colors = colors.clone();
            self.record_theme(name);
            if changed { self.add_milestone("new_clothes"); }
            self.save();
            return Ok(());
        }
        // check custom themes
        let custom = crate::config::load_custom_themes();
        if let Some(colors) = custom.get(name) {
            self.cfg.settings.theme = name.into();
            self.cfg.settings.colors = colors.clone();
            self.record_theme(name);
            if changed { self.add_milestone("new_clothes"); }
            self.save();
            return Ok(());
        }
        anyhow::bail!(
            "unknown theme '{name}'. use :theme new to create one, or :theme list to see available themes"
        )
    }

    fn record_theme(&mut self, name: &str) {
        if !self.cfg.stats.themes.iter().any(|theme| theme == name) {
            self.cfg.stats.themes.push(name.to_string());
        }
    }

    fn record_edit_if_changed(&mut self, tracked: &str, before: Option<&[u8]>, local: &Path) {
        let after = fs::read(local).ok();
        if before != after.as_deref() {
            self.cfg.stats.edits += 1;
            *self.cfg.stats.edited_files.entry(tracked.to_string()).or_default() += 1;
            if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
                let day = format_unix_ts(now.as_secs()).split_whitespace().next().unwrap_or("unknown").to_string();
                *self.cfg.stats.edits_by_day.entry(day).or_default() += 1;
            }
            self.refresh_milestones();
            crate::hooks::run("changed", tracked);
        }
    }

    fn refresh_milestones(&mut self) -> bool {
        let tracked: usize = self.cfg.groups.values().map(Vec::len).sum();
        let earned = [
            ("youre_in", self.cfg.stats.edits > 0),
            ("collection", tracked >= 10),
            ("hoarder", tracked >= 20),
            ("storage_room", tracked >= 30),
            ("configs_managed", tracked >= 50),
            ("dotfile_demon", tracked >= 100),
            ("editorial", self.cfg.stats.edits >= 100),
            ("going_live", self.cfg.settings.streamer_mode),
            ("happy_birthday_to_me", is_local_november_tenth()),
            ("one_of_us", config::confy_config_dir().metadata().ok()
                .and_then(|metadata| metadata.created().ok())
                .and_then(|created| SystemTime::now().duration_since(created).ok())
                .map(|age| age.as_secs() >= 365 * 24 * 60 * 60)
                .unwrap_or(false)),
        ];
        let mut changed = false;
        for (name, unlocked) in earned {
            if unlocked { changed |= self.add_milestone(name); }
        }
        changed
    }

    pub fn stats_summary(&self) -> String {
        let tracked: usize = self.cfg.groups.values().map(Vec::len).sum();
        let most_edited = self.cfg.stats.edited_files.iter()
            .max_by(|(path_a, count_a), (path_b, count_b)| count_a.cmp(count_b).then_with(|| path_b.cmp(path_a)));
        let top = most_edited.map(|(path, count)| format!("{} ({count})", Path::new(path).file_name().unwrap_or_default().to_string_lossy())).unwrap_or_else(|| "none".into());
        let most_opened = self.cfg.stats.opened_files.iter()
            .max_by(|(path_a, count_a), (path_b, count_b)| count_a.cmp(count_b).then_with(|| path_b.cmp(path_a)));
        let opened = most_opened.map(|(path, count)| format!("{} ({count})", Path::new(path).file_name().unwrap_or_default().to_string_lossy())).unwrap_or_else(|| "none".into());
        let active_day = self.cfg.stats.edits_by_day.iter()
            .max_by(|(day_a, count_a), (day_b, count_b)| count_a.cmp(count_b).then_with(|| day_a.cmp(day_b)))
            .map(|(day, count)| format!("{day} ({count} edits)")).unwrap_or_else(|| "none".into());
        let streak = SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|now| {
            let today = now.as_secs() / 86_400;
            let today_key = format_unix_ts(today * 86_400).split_whitespace().next().unwrap_or("").to_string();
            let start = if self.cfg.stats.edits_by_day.contains_key(&today_key) { today } else { today.saturating_sub(1) };
            (0..=today).take_while(|offset| {
                let day = start.saturating_sub(*offset);
                let key = format_unix_ts(day * 86_400).split_whitespace().next().unwrap_or("").to_string();
                self.cfg.stats.edits_by_day.contains_key(&key)
            }).count()
        }).unwrap_or(0);
        let themes = if self.cfg.stats.themes.is_empty() { "none".into() } else { self.cfg.stats.themes.join(", ") };
        let since = config::confy_config_dir().metadata().ok().and_then(|metadata| metadata.created().ok())
            .and_then(|created| created.duration_since(UNIX_EPOCH).ok())
            .map(|created| format_unix_ts(created.as_secs()).split_whitespace().next().unwrap_or("unknown").to_string())
            .unwrap_or_else(|| "unknown".into());
        let milestone_names: Vec<&str> = self.cfg.stats.milestones.iter().map(|id| milestone_title(id)).collect();
        let milestones = if milestone_names.is_empty() { "none".into() } else { milestone_names.join(", ") };
        format!("# confy stats\n\n- using confy since: {since}\n- configs tracked: {tracked}\n- edits made: {}\n- rollbacks used: {}\n- themes tried: {themes}\n- most edited config: {top}\n- most opened config: {opened}\n- editing streak: {streak} days\n- most active day: {active_day}\n- milestones unlocked: {milestones}", self.cfg.stats.edits, self.cfg.stats.rollbacks)
    }

    pub fn all_theme_names(&self) -> Vec<String> {
        let mut names: Vec<String> = builtin_themes()
            .keys()
            .copied()
            .map(|s| s.to_string())
            .collect();
        names.sort();
        for name in crate::config::custom_theme_names() {
            if !names.iter().any(|n| n == &name) {
                names.push(name);
            }
        }
        names
    }

    // ── device mode ───────────────────────────────────────────────────────────

    pub fn switch_device(&mut self, arg: &str) -> Result<()> {
        let arg = arg.trim();
        if arg.is_empty() || arg.eq_ignore_ascii_case("local") {
            self.switch_to_local();
            return Ok(());
        }

        if !device::sshfs_available() {
            return Err(anyhow::anyhow!(
                "sshfs not found. install it: {}",
                device::SSHFS_URL
            ));
        }

        let target = device::resolve_target(arg);

        // unmount existing device if any
        if let Some(m) = &self.mount {
            device::unmount(&m.mountpoint);
        }

        let mount_root = config::mount_root();
        let info = device::mount(&target, &mount_root)?;

        let confy_dir = info.to_local_path(&format!("{}/.config/confy", info.remote_home));
        let fmt = device::check_remote_format(&confy_dir);

        if fmt == RemoteFormat::Unreachable {
            device::unmount(&info.mountpoint);
            return Err(anyhow::anyhow!(
                "couldn't read remote confy dir on {target}"
            ));
        }
        if fmt == RemoteFormat::Legacy {
            device::unmount(&info.mountpoint);
            return Err(anyhow::anyhow!(
                "{target} is running an older confy (tracked.json). update confy there first."
            ));
        }

        // stash local state
        if self.local_state.is_none() {
            self.local_state = Some(Box::new(self.cfg.clone()));
        }

        let su_ok = info.su_available;
        let note = if fmt == RemoteFormat::Empty {
            " (empty, nothing tracked there yet)"
        } else {
            ""
        };

        self.device_name = Some(arg.into());
        self.active_config_dir = confy_dir.clone();
        self.active_config_file = confy_dir.join("config.toml");
        self.cfg.config_dir = format!("{}/.config", info.remote_home);
        self.mount = Some(info);

        config::migrate_if_needed(&self.active_config_dir);
        let (remote_cfg, _) =
            config::load(&self.active_config_file).unwrap_or_else(|_| (AppConfig::default(), true));
        self.cfg = remote_cfg;
        self.add_milestone("haunting_a_remote_machine");
        self.save();
        if let Some(local) = self.local_state.as_mut() {
            if !local.stats.milestones.iter().any(|milestone| milestone == "haunting_a_remote_machine") {
                local.stats.milestones.push("haunting_a_remote_machine".into());
                let _ = config::save(&config::confy_config_file(), local);
            }
        }

        self.selected = 0;
        self.scroll = 0;
        self.rebuild_flat_view();

        if !su_ok {
            self.popup = Some(format!(
                "device → {}{note}. note: :su won't work. add 'user_allow_other' to /etc/fuse.conf",
                self.device_name.as_deref().unwrap_or("?")
            ));
        } else {
            self.popup = Some(format!(
                "device → {}{note}",
                self.device_name.as_deref().unwrap_or("?")
            ));
        }

        Ok(())
    }

    pub fn switch_to_local(&mut self) {
        if let Some(m) = self.mount.take() {
            device::unmount(&m.mountpoint);
        }
        self.device_name = None;

        if let Some(saved) = self.local_state.take() {
            self.cfg = *saved;
        }

        self.active_config_dir = config::confy_config_dir();
        self.active_config_file = config::confy_config_file();
        self.selected = 0;
        self.scroll = 0;
        self.rebuild_flat_view();
        self.popup = Some("device → local".into());
    }

    // ── display path (streamer mode aware) ───────────────────────────────────

    /// returns the path as it should be shown in the ui.
    /// in streamer mode, only the filename is shown. no /home/username leaking on stream.
    pub fn display_path<'a>(&self, tracked: &'a str) -> std::borrow::Cow<'a, str> {
        if self.cfg.settings.streamer_mode {
            Path::new(tracked)
                .file_name()
                .map(|n| std::borrow::Cow::Owned(n.to_string_lossy().into_owned()))
                .unwrap_or(std::borrow::Cow::Borrowed(tracked))
        } else {
            std::borrow::Cow::Borrowed(tracked)
        }
    }

    /// parent dir for display. hidden in streamer mode
    pub fn display_dir(&self, tracked: &str) -> String {
        if self.cfg.settings.streamer_mode {
            return "***".into();
        }
        Path::new(tracked)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    // ── file info ─────────────────────────────────────────────────────────────

    pub fn file_info(&self, tracked: &str) -> (String, String) {
        let local = self.local_path(tracked);
        match local.metadata() {
            Ok(m) => {
                let mtime = m
                    .modified()
                    .ok()
                    .and_then(|t| {
                        use std::time::UNIX_EPOCH;
                        let secs = t.duration_since(UNIX_EPOCH).ok()?.as_secs();
                        // basic formatting without chrono dep
                        Some(format_unix_ts(secs))
                    })
                    .unwrap_or_else(|| "unknown".into());
                let size = format_size(m.len());
                (mtime, size)
            }
            Err(_) => ("unknown".into(), "unknown".into()),
        }
    }

    pub fn preview_lines(&self, tracked: &str, max_lines: usize, max_width: usize) -> Vec<String> {
        let local = self.local_path(tracked);
        if !local.exists() {
            return vec!["(file not found)".into()];
        }
        if local.is_dir() {
            return vec!["(directory)".into()];
        }
        match local.metadata() {
            Ok(m) if m.len() > 5 * 1024 * 1024 => {
                return vec!["(file too large to preview, 5mb+)".into()];
            }
            _ => {}
        }
        match fs::read_to_string(&local) {
            Err(_) => vec!["(binary or unreadable file)".into()],
            Ok(content) => {
                let mut lines: Vec<String> = content
                    .lines()
                    .take(max_lines)
                    .map(|l| {
                        let char_count = l.chars().count();
                        if char_count > max_width {
                            // slice on char boundaries, never bytes. avoids panic on unicode
                            let truncated: String =
                                l.chars().take(max_width.saturating_sub(1)).collect();
                            format!("{truncated}…")
                        } else {
                            l.to_string()
                        }
                    })
                    .collect();
                if content.lines().count() > max_lines {
                    lines.push("...".into());
                }
                if lines.is_empty() {
                    lines.push("(empty file)".into());
                }
                lines
            }
        }
    }
}

fn is_local_november_tenth() -> bool {
    std::process::Command::new("date").arg("+%m-%d").output().ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim() == "11-10")
        .unwrap_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).ok()
            .map(|now| format_unix_ts(now.as_secs()).split_whitespace().next().unwrap_or("").ends_with("-11-10"))
            .unwrap_or(false))
}

fn milestone_title(id: &str) -> &'static str {
    match id {
        "youre_in" => "you're in",
        "oops" => "oops",
        "collection" => "collection",
        "hoarder" => "hoarder",
        "storage_room" => "storage room",
        "configs_managed" => "configs: managed",
        "dotfile_demon" => "dotfile demon",
        "editorial" => "editorial",
        "welcome_to_confy" => "welcome to confy!",
        "new_clothes" => "new clothes!",
        "self_sewed" => "self-sewed",
        "thrift_shop" => "thrift shop",
        "going_live" => "going live",
        "haunting_a_remote_machine" => "haunting a remote machine",
        "with_great_power" => "with great power..",
        "happy_birthday_to_me" => "happy birthday to me!",
        "one_of_us" => "one of us",
        _ => "unknown milestone",
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn format_size(mut size: u64) -> String {
    for unit in ["b", "kb", "mb", "gb"] {
        if size < 1024 {
            return format!("{size}{unit}");
        }
        size /= 1024;
    }
    format!("{size}tb")
}

pub(crate) fn format_unix_ts(secs: u64) -> String {
    // basic yyyy-mm-dd hh:mm without chrono
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;
    let mut y = 1970u64;
    let mut d = days;
    loop {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let yd = if leap { 366 } else { 365 };
        if d < yd {
            break;
        }
        d -= yd;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let months = [
        31u64,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mo = 1u64;
    for mlen in months {
        if d < mlen {
            break;
        }
        d -= mlen;
        mo += 1;
    }
    format!("{y:04}-{mo:02}-{:02} {h:02}:{m:02}", d + 1)
}

fn which_editor(name: &str) -> Option<String> {
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
            .map(|p| p.to_string_lossy().into_owned())
    })
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(m) = &self.mount {
            device::unmount(&m.mountpoint);
        }
    }
}
