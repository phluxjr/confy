use anyhow::{Context, Result};
use dirs::home_dir;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

// ── paths ─────────────────────────────────────────────────────────────────────

pub fn confy_config_dir() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".config")
        .join("confy")
}

pub fn confy_config_file() -> PathBuf {
    confy_config_dir().join("config.toml")
}

pub fn hooks_file() -> PathBuf {
    confy_config_dir().join("hooks.toml")
}

pub fn confy_legacy_file() -> PathBuf {
    confy_config_dir().join("tracked.json")
}

pub fn mount_root() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".cache")
        .join("confy")
        .join("mounts")
}

pub fn backup_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| {
            home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".local/share")
        })
        .join("confy")
        .join("backups")
}

// ── themes ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorSettings {
    pub bg: String,
    pub fg: String,
    pub highlight: String,
    pub group: String,
    pub border: String,
}

impl Default for ColorSettings {
    fn default() -> Self {
        Self {
            bg: "default".into(),
            fg: "default".into(),
            highlight: "#cba6f7".into(),
            group: "#89b4fa".into(),
            border: "default".into(),
        }
    }
}

pub fn builtin_themes() -> HashMap<&'static str, ColorSettings> {
    let mut m = HashMap::new();
    m.insert(
        "catppuccin",
        ColorSettings {
            highlight: "#cba6f7".into(),
            group: "#89b4fa".into(),
            ..Default::default()
        },
    );
    m.insert(
        "dracula",
        ColorSettings {
            highlight: "#bd93f9".into(),
            group: "#8be9fd".into(),
            ..Default::default()
        },
    );
    m.insert(
        "gruvbox",
        ColorSettings {
            highlight: "#fabd2f".into(),
            group: "#83a598".into(),
            ..Default::default()
        },
    );
    m.insert(
        "nord",
        ColorSettings {
            highlight: "#88c0d0".into(),
            group: "#81a1c1".into(),
            ..Default::default()
        },
    );
    m.insert(
        "tokyo-night",
        ColorSettings {
            highlight: "#bb9af7".into(),
            group: "#7aa2f7".into(),
            ..Default::default()
        },
    );
    m.insert(
        "one-dark",
        ColorSettings {
            highlight: "#c678dd".into(),
            group: "#61afef".into(),
            ..Default::default()
        },
    );
    m
}

// ── custom themes ─────────────────────────────────────────────────────────────
/// directory where custom user themes are stored (toml files)
pub fn custom_themes_dir() -> PathBuf {
    confy_config_dir().join("themes")
}

/// path to a custom theme's toml file
pub fn custom_theme_path(name: &str) -> PathBuf {
    custom_themes_dir().join(format!("{name}.toml"))
}

pub fn load_custom_themes() -> HashMap<String, ColorSettings> {
    let dir = custom_themes_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir).ok();
    }
    load_themes_in_dir(&dir)
}

fn load_themes_in_dir(dir: &Path) -> HashMap<String, ColorSettings> {
    let mut themes = HashMap::new();
    if let Ok(read_dir) = fs::read_dir(dir) {
        for entry in read_dir {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let file = dir.join(format!("{}.toml", name));
            if !file.exists() {
                continue;
            }
            let content = match fs::read_to_string(&file) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let cs = match theme_from_toml(&content) {
                Some(cs) => cs,
                None => continue,
            };
            themes.insert(name, cs);
        }
    }
    themes
}

/// parse a single theme toml document into colorsettings.
/// missing keys fall back to defaults, so a minimal theme file works.
pub(crate) fn theme_from_toml(content: &str) -> Option<ColorSettings> {
    let value: toml::Value = content.parse::<toml::Value>().ok()?;
    let mut cs = ColorSettings::default();
    if let Some(v) = value.get("bg").and_then(|v| v.as_str()) {
        cs.bg = v.into();
    }
    if let Some(v) = value.get("fg").and_then(|v| v.as_str()) {
        cs.fg = v.into();
    }
    if let Some(v) = value.get("highlight").and_then(|v| v.as_str()) {
        cs.highlight = v.into();
    }
    if let Some(v) = value.get("group").and_then(|v| v.as_str()) {
        cs.group = v.into();
    }
    if let Some(v) = value.get("border").and_then(|v| v.as_str()) {
        cs.border = v.into();
    }
    Some(cs)
}

pub fn save_custom_theme(name: &str, colors: &ColorSettings) -> Result<()> {
    validate_theme_name(name)?;
    // don't allow overwriting built-in themes
    if builtin_themes().contains_key(name) {
        anyhow::bail!("'{name}' is a built-in theme. choose another name");
    }
    if is_reserved_theme_name(name) {
        anyhow::bail!("'{name}' is reserved for a theme command. choose another name");
    }
    let dir = custom_themes_dir();
    fs::create_dir_all(&dir).with_context(|| format!("couldn't create {}", dir.display()))?;
    save_theme_in_dir(&dir, name, colors)
}

/// names that would collide with `:theme <subcommand>`
pub fn is_reserved_theme_name(name: &str) -> bool {
    matches!(name, "new" | "create" | "list" | "delete" | "rm")
}

/// install a theme downloaded from the community repository.
/// the provided toml is validated before being written.
pub fn install_custom_theme(name: &str, theme_toml: &str) -> Result<()> {
    validate_theme_name(name)?;
    if builtin_themes().contains_key(name) {
        anyhow::bail!("'{name}' is a built-in theme. choose another name");
    }
    if is_reserved_theme_name(name) {
        anyhow::bail!("'{name}' is reserved for a theme command. choose another name");
    }
    if theme_from_toml(theme_toml).is_none() {
        anyhow::bail!("downloaded theme file isn't valid toml");
    }
    let dir = custom_themes_dir();
    fs::create_dir_all(&dir).with_context(|| format!("couldn't create {}", dir.display()))?;
    write_theme_content_in_dir(&dir, name, theme_toml)
}

fn write_theme_content_in_dir(dir: &Path, name: &str, content: &str) -> Result<()> {
    let file = dir.join(format!("{name}.toml"));
    fs::write(&file, content).with_context(|| format!("couldn't write {}", file.display()))?;
    Ok(())
}

fn save_theme_in_dir(dir: &Path, name: &str, colors: &ColorSettings) -> Result<()> {
    let file = dir.join(format!("{}.toml", name));
    let toml_str = toml::to_string(colors).with_context(|| "couldn't serialise theme")?;
    fs::write(&file, toml_str).with_context(|| format!("couldn't write {}", file.display()))?;
    Ok(())
}

pub fn delete_custom_theme(name: &str) -> Result<()> {
    validate_theme_name(name)?;
    if builtin_themes().contains_key(name) {
        anyhow::bail!("'{name}' is a built-in theme and can't be deleted");
    }
    let file = custom_themes_dir().join(format!("{}.toml", name));
    if !file.exists() {
        anyhow::bail!("no custom theme named '{name}'");
    }
    fs::remove_file(&file).with_context(|| format!("couldn't delete {}", file.display()))?;
    Ok(())
}

fn validate_theme_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("theme name cannot be empty");
    }
    if name.chars().any(char::is_whitespace) {
        anyhow::bail!("theme names can't contain spaces");
    }
    // guard against path traversal / weird names
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        anyhow::bail!("invalid theme name '{name}'");
    }
    Ok(())
}

pub fn custom_theme_names() -> Vec<String> {
    let mut names: Vec<String> = load_custom_themes().keys().cloned().collect();
    names.sort();
    names
}

// ── settings ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_true")]
    pub rollback: bool,
    #[serde(default = "default_true")]
    pub first_startup: bool,
    #[serde(default = "default_theme")]
    pub theme: String,

    // background: draw a solid bg instead of terminal default (textual-style)
    #[serde(default)]
    pub background_enable: bool,
    #[serde(default = "default_bg_color")]
    pub background_color: String,

    // streamer mode: hide /home/username paths, show only filenames
    #[serde(default)]
    pub streamer_mode: bool,
    // in streamer mode, also hide files that are git-blacklisted
    #[serde(default = "default_true")]
    pub streamer_hide_git: bool,

    // override $editor (useful for .desktop launchers)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor: Option<String>,

    // allow sshfs / :device integration (can be disabled if sshfs isn't installed)
    #[serde(default = "default_true")]
    pub ssh_allow: bool,

    // must come last: toml requires tables after scalar values
    #[serde(default)]
    pub colors: ColorSettings,
}

fn default_true() -> bool {
    true
}
fn default_theme() -> String {
    "catppuccin".into()
}
fn default_bg_color() -> String {
    "#1e1e2e".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            rollback: true,
            first_startup: true,
            theme: "catppuccin".into(),
            colors: ColorSettings::default(),
            background_enable: false,
            background_color: default_bg_color(),
            streamer_mode: false,
            streamer_hide_git: true,
            editor: None,
            ssh_allow: true,
        }
    }
}

// ── main config ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub stats: UsageStats,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_opened: Option<String>,
    #[serde(default)]
    pub collapsed_groups: HashSet<String>,
    #[serde(default = "default_sort_mode")]
    pub sort_mode: String,
    #[serde(default = "default_sort_order")]
    pub sort_order: String,
    #[serde(default = "default_config_dir")]
    pub config_dir: String,
    #[serde(default)]
    pub preview_enabled: bool,

    // git integration
    #[serde(default = "default_git_dir")]
    pub git_dir: String,
    #[serde(default)]
    pub git_blacklist: Vec<String>,
    #[serde(default)]
    pub git_blacklist_groups: Vec<String>,
    #[serde(default)]
    pub git_auto_push: bool,
    #[serde(default)]
    pub git_auto_commit: bool,

    // tables must come last: toml requires them after scalar/array values
    #[serde(default)]
    pub groups: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageStats {
    #[serde(default)]
    pub edits: u64,
    #[serde(default)]
    pub rollbacks: u64,
    #[serde(default)]
    pub themes: Vec<String>,
    #[serde(default)]
    pub edited_files: HashMap<String, u64>,
    #[serde(default)]
    pub opened_files: HashMap<String, u64>,
    #[serde(default)]
    pub edits_by_day: HashMap<String, u64>,
    #[serde(default)]
    pub milestones: Vec<String>,
}

fn default_sort_mode() -> String {
    "name".into()
}
fn default_sort_order() -> String {
    "asc".into()
}
fn default_config_dir() -> String {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("/home"))
        .join(".config")
        .to_string_lossy()
        .into_owned()
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut groups = HashMap::new();
        groups.insert("ungrouped".into(), vec![]);
        Self {
            stats: UsageStats::default(),
            groups,
            last_opened: None,
            collapsed_groups: HashSet::new(),
            sort_mode: default_sort_mode(),
            sort_order: default_sort_order(),
            config_dir: default_config_dir(),
            settings: Settings::default(),
            preview_enabled: false,
            git_dir: default_git_dir(),
            git_blacklist: vec![],
            git_blacklist_groups: vec![],
            git_auto_push: false,
            git_auto_commit: false,
        }
    }
}

// ── legacy migration ──────────────────────────────────────────────────────────

/// migrate older config formats to `config.toml`.
///
/// - `config.json` is kept on disk and copied to `config.json.bak` first.
/// - `tracked.json` is migrated when no `config.json` exists.
///
/// returns a status message when a migration actually happened.
pub fn migrate_if_needed(config_dir: &Path) -> Option<String> {
    let current = config_dir.join("config.toml");
    if current.exists() {
        return None;
    }

    let json = config_dir.join("config.json");
    let legacy = config_dir.join("tracked.json");

    let (source, backup) = if json.exists() {
        (json.clone(), Some(json.with_extension("json.bak")))
    } else if legacy.exists() {
        (legacy.clone(), None)
    } else {
        return None;
    };

    let raw = fs::read_to_string(&source).ok()?;
    if raw.trim().is_empty() {
        return None;
    }

    // parse the old json (also handles the very old {"files": [...]} shape)
    let cfg = parse_json_config(&raw, &source).ok()?;

    // keep a copy of the old file before rewriting anything
    if let Some(bak) = &backup {
        if !bak.exists() {
            let _ = fs::copy(&source, bak);
        }
    }

    let toml_str = toml::to_string_pretty(&cfg).ok()?;
    let tmp = current.with_extension("toml.tmp");
    fs::write(&tmp, toml_str).ok()?;
    fs::rename(&tmp, &current).ok()?;

    let from = source.file_name()?.to_string_lossy().into_owned();
    if backup.is_some() {
        Some(format!(
            "migrated {from} → config.toml (backup: config.json.bak)!"
        ))
    } else {
        Some(format!("migrated {from} → config.toml!"))
    }
}

// ── load / save ───────────────────────────────────────────────────────────────

pub fn load(path: &Path) -> Result<(AppConfig, bool)> {
    // prefer toml, but fall back to a sibling config.json if the toml file
    // doesn't exist yet (or a migration failed partway through).
    let actual = if path.exists() {
        path.to_path_buf()
    } else {
        let json = path.with_extension("json");
        if json.exists() {
            json
        } else {
            return Ok((AppConfig::default(), true)); // first startup
        }
    };

    let raw = fs::read_to_string(&actual)
        .with_context(|| format!("couldn't read {}", actual.display()))?;
    if raw.trim().is_empty() {
        return Ok((AppConfig::default(), true));
    }

    let is_json = actual.extension().and_then(|e| e.to_str()) == Some("json");
    let cfg = if is_json {
        parse_json_config(&raw, &actual)?
    } else {
        parse_toml_config(&raw, &actual)?
    };

    let first = cfg.settings.first_startup;
    Ok((cfg, first))
}

pub fn save(path: &Path, cfg: &AppConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let toml_str = toml::to_string_pretty(cfg).with_context(|| "couldn't serialise config")?;
    fs::write(path, toml_str).with_context(|| format!("couldn't write {}", path.display()))?;
    Ok(())
}

/// parse the old json config format, including `{"files": [...]}`
fn parse_json_config(raw: &str, path: &Path) -> Result<AppConfig> {
    let mut val: serde_json::Value =
        serde_json::from_str(raw).with_context(|| format!("couldn't parse {}", path.display()))?;
    if let Some(files) = val.get("files").cloned() {
        let mut groups = serde_json::Map::new();
        groups.insert("ungrouped".into(), files);
        val["groups"] = serde_json::Value::Object(groups);
    }
    serde_json::from_value(val).with_context(|| format!("couldn't deserialise {}", path.display()))
}

/// parse a toml config, including the old `files = [...]` array
fn parse_toml_config(raw: &str, path: &Path) -> Result<AppConfig> {
    let mut val: toml::Value =
        toml::from_str(raw).with_context(|| format!("couldn't parse {}", path.display()))?;
    if let toml::Value::Table(table) = &mut val {
        if let Some(files) = table.remove("files") {
            if !table.contains_key("groups") {
                let mut groups = toml::map::Map::new();
                groups.insert("ungrouped".into(), files);
                table.insert("groups".into(), toml::Value::Table(groups));
            }
        }
    }
    val.try_into()
        .with_context(|| format!("couldn't deserialise {}", path.display()))
}

// ── git helpers (used by git.rs) ──────────────────────────────────────────────

pub fn default_git_dir() -> String {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("/home"))
        .join("dotfiles-git")
        .to_string_lossy()
        .into_owned()
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_theme_toml_roundtrip() {
        let cs = ColorSettings {
            bg: "#101010".into(),
            fg: "#eeeeee".into(),
            highlight: "#ff5555".into(),
            group: "#55ff55".into(),
            border: "#5555ff".into(),
        };
        let toml_str = toml::to_string(&cs).unwrap();
        let parsed = theme_from_toml(&toml_str).unwrap();
        assert_eq!(parsed.bg, cs.bg);
        assert_eq!(parsed.fg, cs.fg);
        assert_eq!(parsed.highlight, cs.highlight);
        assert_eq!(parsed.group, cs.group);
        assert_eq!(parsed.border, cs.border);
    }

    #[test]
    fn custom_theme_toml_partial_uses_defaults() {
        // a minimal theme file should still parse, filling in defaults
        let parsed = theme_from_toml("highlight = \"#abcdef\"\n").unwrap();
        let defaults = ColorSettings::default();
        assert_eq!(parsed.highlight, "#abcdef");
        assert_eq!(parsed.bg, defaults.bg);
        assert_eq!(parsed.group, defaults.group);
    }

    #[test]
    fn invalid_theme_toml_is_rejected() {
        assert!(theme_from_toml("this is not = valid = toml [").is_none());
    }

    #[test]
    fn custom_theme_save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "confy-theme-test-{}-{}",
            std::process::id(),
            "roundtrip"
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let cs = ColorSettings {
            bg: "black".into(),
            fg: "white".into(),
            highlight: "#ff00aa".into(),
            group: "#00ffaa".into(),
            border: "default".into(),
        };
        save_theme_in_dir(&dir, "mytheme", &cs).unwrap();

        let loaded = load_themes_in_dir(&dir);
        let got = loaded.get("mytheme").expect("theme should be loaded");
        assert_eq!(got.bg, cs.bg);
        assert_eq!(got.fg, cs.fg);
        assert_eq!(got.highlight, cs.highlight);
        assert_eq!(got.group, cs.group);
        assert_eq!(got.border, cs.border);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn theme_name_validation() {
        assert!(validate_theme_name("mytheme").is_ok());
        assert!(validate_theme_name("my-theme_2").is_ok());
        assert!(validate_theme_name("").is_err());
        assert!(validate_theme_name("   ").is_err());
        assert!(validate_theme_name("has space").is_err());
        assert!(validate_theme_name("../escape").is_err());
        assert!(validate_theme_name("a/b").is_err());
    }

    #[test]
    fn reserved_theme_names() {
        assert!(is_reserved_theme_name("new"));
        assert!(is_reserved_theme_name("list"));
        assert!(is_reserved_theme_name("delete"));
        assert!(!is_reserved_theme_name("mytheme"));
    }

    fn temp_config_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "confy-config-test-{}-{}",
            std::process::id(),
            label
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_config() -> AppConfig {
        let mut cfg = AppConfig::default();
        cfg.groups.insert(
            "hyprland".into(),
            vec!["/home/user/.config/hypr/hyprland.conf".into()],
        );
        cfg.groups
            .get_mut("ungrouped")
            .unwrap()
            .push("/home/user/.bashrc".into());
        cfg.last_opened = Some("/home/user/.bashrc".into());
        cfg.settings.theme = "dracula".into();
        cfg.settings.rollback = false;
        cfg.settings.colors.highlight = "#ff00aa".into();
        cfg.git_blacklist
            .push("/home/user/.config/secret.env".into());
        cfg
    }

    #[test]
    fn config_toml_roundtrip() {
        let dir = temp_config_dir("roundtrip");
        let path = dir.join("config.toml");
        let cfg = sample_config();

        save(&path, &cfg).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("[groups]"), "expected a groups table:\n{raw}");
        assert!(
            raw.contains("[settings]"),
            "expected a settings table:\n{raw}"
        );
        assert!(
            raw.contains("[settings.colors]"),
            "expected a colors table:\n{raw}"
        );

        let (loaded, _) = load(&path).unwrap();
        assert_eq!(loaded.settings.theme, cfg.settings.theme);
        assert_eq!(loaded.settings.rollback, cfg.settings.rollback);
        assert_eq!(
            loaded.settings.colors.highlight,
            cfg.settings.colors.highlight
        );
        assert_eq!(loaded.groups["hyprland"], cfg.groups["hyprland"]);
        assert_eq!(loaded.groups["ungrouped"], cfg.groups["ungrouped"]);
        assert_eq!(loaded.last_opened, cfg.last_opened);
        assert_eq!(loaded.git_blacklist, cfg.git_blacklist);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_to_toml_migration() {
        let dir = temp_config_dir("migrate");
        let json_path = dir.join("config.json");
        let cfg = sample_config();
        fs::write(&json_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();

        let msg = migrate_if_needed(&dir).expect("migration should run");
        assert!(msg.contains("config.toml"), "unexpected message: {msg}");

        let toml_path = dir.join("config.toml");
        assert!(toml_path.exists(), "config.toml should be created");
        assert!(json_path.exists(), "old config.json should be kept");
        assert!(
            dir.join("config.json.bak").exists(),
            "a backup of config.json should be created"
        );

        let (loaded, _) = load(&toml_path).unwrap();
        assert_eq!(loaded.settings.theme, "dracula");
        assert_eq!(loaded.groups["hyprland"], cfg.groups["hyprland"]);
        assert_eq!(loaded.last_opened, cfg.last_opened);

        // running it again is a no-op
        assert!(migrate_if_needed(&dir).is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_files_json_migrates_to_ungrouped() {
        let dir = temp_config_dir("legacy-files");
        fs::write(
            dir.join("config.json"),
            r#"{"files":["/home/user/.bashrc","/home/user/.zshrc"]}"#,
        )
        .unwrap();

        migrate_if_needed(&dir).expect("migration should run");
        let (loaded, _) = load(&dir.join("config.toml")).unwrap();
        assert_eq!(
            loaded.groups["ungrouped"],
            vec![
                "/home/user/.bashrc".to_string(),
                "/home/user/.zshrc".to_string()
            ]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tracked_json_migrates_to_toml() {
        let dir = temp_config_dir("tracked");
        let cfg = sample_config();
        fs::write(
            dir.join("tracked.json"),
            serde_json::to_string_pretty(&cfg).unwrap(),
        )
        .unwrap();

        let msg = migrate_if_needed(&dir).expect("migration should run");
        assert!(msg.contains("tracked.json"), "unexpected message: {msg}");
        assert!(dir.join("config.toml").exists());
        assert!(!dir.join("config.json").exists());

        let (loaded, _) = load(&dir.join("config.toml")).unwrap();
        assert_eq!(loaded.settings.theme, "dracula");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_falls_back_to_json_when_toml_missing() {
        let dir = temp_config_dir("fallback");
        let cfg = sample_config();
        fs::write(
            dir.join("config.json"),
            serde_json::to_string_pretty(&cfg).unwrap(),
        )
        .unwrap();

        // ask for config.toml even though it doesn't exist
        let (loaded, _) = load(&dir.join("config.toml")).unwrap();
        assert_eq!(loaded.settings.theme, "dracula");
        assert_eq!(loaded.groups["hyprland"], cfg.groups["hyprland"]);

        let _ = fs::remove_dir_all(&dir);
    }
}
