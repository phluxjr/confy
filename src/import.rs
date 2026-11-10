use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct ImportCandidate {
    pub path: PathBuf,
    pub source: String,
}

pub fn discover(source: Option<&str>) -> Result<Vec<ImportCandidate>> {
    let home = dirs::home_dir().context("couldn't find home directory")?;
    let mut found = Vec::new();
    let sources: &[&str] = match source {
        Some("chezmoi") => &["chezmoi"],
        Some("stow") => &["stow"],
        Some("yadm") => &["yadm"],
        Some(other) => bail!("unknown import source '{other}' (expected chezmoi, stow, or yadm)"),
        None => &["chezmoi", "stow", "yadm"],
    };
    for name in sources {
        let result = match *name {
            "chezmoi" => discover_chezmoi(&home),
            "yadm" => discover_yadm(&home),
            "stow" => Ok(discover_stow(&home)),
            _ => unreachable!(),
        };
        match result {
            Ok(candidates) => found.extend(candidates),
            Err(error) if source.is_some() => return Err(error),
            Err(_) => {}
        }
    }
    let mut seen = HashSet::new();
    found.retain(|candidate| seen.insert(candidate.path.clone()));
    if found.is_empty() {
        bail!("no importable configs found; specify a source with :import chezmoi|stow|yadm")
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
}

fn discover_chezmoi(home: &Path) -> Result<Vec<ImportCandidate>> {
    let output = Command::new("chezmoi")
        .args(["managed", "--include=files,symlinks", "--path-style=absolute", "--null-path-separator"])
        .output()
        .context("chezmoi isn't installed or couldn't be started")?;
    if !output.status.success() {
        bail!("chezmoi couldn't list managed files");
    }
    Ok(output.stdout.split(|byte| *byte == 0).filter_map(|raw| {
        let path = PathBuf::from(String::from_utf8_lossy(raw).trim());
        (path.is_absolute() && path.starts_with(home) && path.exists()).then_some(ImportCandidate { path, source: "chezmoi".into() })
    }).collect())
}

fn discover_yadm(home: &Path) -> Result<Vec<ImportCandidate>> {
    let output = Command::new("yadm").args(["list", "-a"]).current_dir(home).output()
        .context("yadm isn't installed or couldn't be started")?;
    if !output.status.success() { bail!("yadm couldn't list managed files"); }
    Ok(String::from_utf8_lossy(&output.stdout).lines().filter_map(|line| {
        let path = PathBuf::from(line.trim());
        let path = if path.is_absolute() { path } else { home.join(path) };
        (path.starts_with(home) && path.exists()).then_some(ImportCandidate { path, source: "yadm".into() })
    }).collect())
}

fn discover_stow(home: &Path) -> Vec<ImportCandidate> {
    let roots = [home.join("dotfiles"), home.join(".dotfiles"), home.join("stow"), home.join(".local/share/stow"), home.join(".local/stow")];
    let mut found = Vec::new();
    scan_stow_links(home, home, &roots, 0, &mut found);
    found
}

fn scan_stow_links(dir: &Path, home: &Path, roots: &[PathBuf], depth: usize, found: &mut Vec<ImportCandidate>) {
    if depth > 8 { return; }
    let Ok(entries) = fs::read_dir(dir) else { return; };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else { continue; };
        if meta.file_type().is_symlink() {
            if let Ok(target) = fs::canonicalize(&path) {
                if roots.iter().any(|root| target.starts_with(root)) {
                    found.push(ImportCandidate { path, source: "stow".into() });
                }
            }
        } else if meta.is_dir() {
            let name = entry.file_name();
            if matches!(name.to_str(), Some(".cache" | ".local" | ".mozilla" | ".steam" | ".var" | "node_modules" | ".git")) {
                if name == ".local" && depth == 0 {
                    scan_stow_links(&path, home, roots, depth + 1, found);
                }
                continue;
            }
            if path == home.join(".config") || depth == 0 || depth < 2 {
                scan_stow_links(&path, home, roots, depth + 1, found);
            }
        }
    }
}
