// confy - a config manager for linux/unix systems
// copyright (c) 2025-2026 phluxjr
// licensed under gpl-3.0-or-later

//! fetches community themes from the `confy-tui/community-themes` github repo.
//!
//! repository layout:
//!
//! ```text
//! themes/
//!   <kebab-case-name>/
//!     theme.toml   (normal custom theme keys)
//!     meta.toml    (display metadata)
//! ```

use crate::config::{self, ColorSettings};
use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::time::Duration;

pub const REPO: &str = "confy-tui/community-themes";
pub const BRANCH: &str = "main";

/// display metadata from a theme's `meta.toml`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeMeta {
    pub name: String,
    pub author: String,
    pub link: Option<String>,
    pub tags: Vec<String>,
    pub description: String,
}

/// a theme downloaded from the community repository.
#[derive(Debug, Clone)]
pub struct RemoteTheme {
    /// folder name / install id (lowercase kebab-case)
    pub id: String,
    pub meta: ThemeMeta,
    pub theme_toml: String,
    pub meta_toml: String,
    pub colors: ColorSettings,
}

#[derive(Debug, Deserialize)]
struct ContentEntry {
    name: String,
    #[serde(rename = "type")]
    kind: String,
}

/// fetch the complete community theme catalog.
///
/// one api call lists the `themes/` folder, then each theme's files are
/// downloaded from `raw.githubusercontent.com` (which has no api rate limit).
pub fn fetch_themes() -> Result<Vec<RemoteTheme>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("confy-tui/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .context("couldn't create http client")?;

    let api = format!("https://api.github.com/repos/{REPO}/contents/themes?ref={BRANCH}");
    let entries: Vec<ContentEntry> = client
        .get(&api)
        .header("accept", "application/vnd.github+json")
        .send()
        .context("couldn't reach github")?
        .error_for_status()
        .context("community themes folder isn't available")?
        .json()
        .context("couldn't read the community theme index")?;

    let mut ids: Vec<String> = entries
        .into_iter()
        .filter(|e| e.kind == "dir" && e.name != "example")
        .map(|e| e.name)
        .collect();
    ids.sort();

    if ids.is_empty() {
        anyhow::bail!("no community themes are available yet");
    }

    let mut themes = Vec::new();
    let mut last_error = None;
    for id in ids {
        match fetch_one(&client, &id) {
            Ok(theme) => themes.push(theme),
            Err(e) => last_error = Some(e),
        }
    }

    if themes.is_empty() {
        if let Some(e) = last_error {
            return Err(e.context("couldn't download any community themes"));
        }
        anyhow::bail!("no community themes are available yet");
    }

    Ok(themes)
}

fn fetch_one(client: &reqwest::blocking::Client, id: &str) -> Result<RemoteTheme> {
    let theme_toml = fetch_raw(client, id, "theme.toml")?;
    let meta_toml = fetch_raw(client, id, "meta.toml")?;

    let colors = config::theme_from_toml(&theme_toml)
        .ok_or_else(|| anyhow!("{id}/theme.toml isn't a valid theme"))?;
    let meta = parse_meta(id, &meta_toml);

    Ok(RemoteTheme {
        id: id.to_string(),
        meta,
        theme_toml,
        meta_toml,
        colors,
    })
}

fn fetch_raw(client: &reqwest::blocking::Client, id: &str, file: &str) -> Result<String> {
    let url = format!("https://raw.githubusercontent.com/{REPO}/{BRANCH}/themes/{id}/{file}");
    Ok(client
        .get(&url)
        .send()
        .with_context(|| format!("couldn't fetch {id}/{file}"))?
        .error_for_status()
        .with_context(|| format!("{id}/{file} is missing"))?
        .text()
        .with_context(|| format!("couldn't read {id}/{file}"))?)
}

// ── meta.toml parsing ─────────────────────────────────────────────────────────

/// parse a `meta.toml`, tolerating a few common key aliases.
pub fn parse_meta(id: &str, content: &str) -> ThemeMeta {
    let table = match content.parse::<toml::Value>() {
        Ok(toml::Value::Table(t)) => t,
        _ => {
            return ThemeMeta {
                name: id.to_string(),
                ..Default::default()
            };
        }
    };

    ThemeMeta {
        name: first_string(&table, &["name", "title", "display_name", "display-name"])
            .unwrap_or_else(|| id.to_string()),
        author: first_string(&table, &["author", "authors", "creator", "by"]).unwrap_or_default(),
        link: first_string(&table, &["link", "url", "homepage", "website"]),
        tags: string_list(&table, &["tags", "tag", "categories"]),
        description: first_string(&table, &["description", "desc", "summary", "about"])
            .unwrap_or_default(),
    }
}

fn first_string(table: &toml::map::Map<String, toml::Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = table.get(*key) {
            match value {
                toml::Value::String(s) => return Some(s.trim().to_string()),
                toml::Value::Array(items) => {
                    let parts: Vec<String> = items
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect();
                    if !parts.is_empty() {
                        return Some(parts.join(", "));
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn string_list(table: &toml::map::Map<String, toml::Value>, keys: &[&str]) -> Vec<String> {
    for key in keys {
        if let Some(value) = table.get(*key) {
            match value {
                toml::Value::Array(items) => {
                    return items
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                toml::Value::String(s) if !s.trim().is_empty() => {
                    return s
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                _ => {}
            }
        }
    }
    Vec::new()
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_meta() {
        let meta = parse_meta(
            "aurora",
            r#"
name = "aurora"
author = "jane doe"
link = "https://example.com/jane"
tags = ["dark", "cozy", "purple"]
description = "a calm dark theme with soft purples."
"#,
        );
        assert_eq!(meta.name, "aurora");
        assert_eq!(meta.author, "jane doe");
        assert_eq!(meta.link.as_deref(), Some("https://example.com/jane"));
        assert_eq!(meta.tags, vec!["dark", "cozy", "purple"]);
        assert_eq!(meta.description, "a calm dark theme with soft purples.");
    }

    #[test]
    fn meta_aliases_and_defaults() {
        let meta = parse_meta(
            "fallback-id",
            r#"
title = "friendly name"
creator = ["a", "b"]
url = "https://example.com"
tag = "light"
summary = "short"
"#,
        );
        assert_eq!(meta.name, "friendly name");
        assert_eq!(meta.author, "a, b");
        assert_eq!(meta.link.as_deref(), Some("https://example.com"));
        assert_eq!(meta.tags, vec!["light"]);
        assert_eq!(meta.description, "short");

        let empty = parse_meta("fallback-id", "");
        assert_eq!(empty.name, "fallback-id");
        assert!(empty.author.is_empty());
        assert!(empty.tags.is_empty());
    }

    #[test]
    fn invalid_meta_falls_back_to_id() {
        let meta = parse_meta("broken", "this is not = toml [");
        assert_eq!(meta.name, "broken");
        assert_eq!(meta.description, "");
    }
}
