use crate::app::App;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub enum Format {
    Image,
    Banner,
    Post,
}

pub fn write(app: &App, format: Format, transparent: bool, destination: Option<&Path>) -> Result<PathBuf> {
    let (width, height, name) = match format {
        Format::Image => (1200, 675, "img"),
        Format::Banner => (1600, 500, "banner"),
        Format::Post => (1080, 1350, "post"),
    };
    let fallback = PathBuf::from(format!("confy-stats-{name}{}.svg", if transparent { "-transparent" } else { "" }));
    let path = destination.map(Path::to_path_buf).unwrap_or(fallback);
    let svg = render(app, width, height, transparent);
    std::fs::write(&path, svg).with_context(|| format!("couldn't write {}", path.display()))?;
    Ok(path)
}

fn render(app: &App, width: u32, height: u32, transparent: bool) -> String {
    let colors = &app.cfg.settings.colors;
    let bg = color(&colors.bg, "#1e1e2e");
    let fg = color(&colors.fg, "#cdd6f4");
    let accent = color(&colors.highlight, "#cba6f7");
    let secondary = color(&colors.group, "#89b4fa");
    let border = color(&colors.border, "#45475a");
    let tracked: usize = app.cfg.groups.values().map(Vec::len).sum();
    let top_opened = app.cfg.stats.opened_files.iter()
        .max_by(|(path_a, count_a), (path_b, count_b)| count_a.cmp(count_b).then_with(|| path_b.cmp(path_a)))
        .map(|(path, count)| (Path::new(path).file_name().unwrap_or_default().to_string_lossy().into_owned(), *count));
    let top_edited = app.cfg.stats.edited_files.iter()
        .max_by(|(path_a, count_a), (path_b, count_b)| count_a.cmp(count_b).then_with(|| path_b.cmp(path_a)))
        .map(|(path, count)| (Path::new(path).file_name().unwrap_or_default().to_string_lossy().into_owned(), *count));
    let stats = [
        ("configs tracked", tracked.to_string()),
        ("edits made", app.cfg.stats.edits.to_string()),
        ("rollbacks", app.cfg.stats.rollbacks.to_string()),
        ("themes tried", app.cfg.stats.themes.len().to_string()),
    ];
    let margin = width as f32 * 0.06;
    let card_y = height as f32 * 0.34;
    let gap = width as f32 * 0.018;
    let card_w = (width as f32 - 2.0 * margin - 3.0 * gap) / 4.0;
    let card_h = height as f32 * 0.19;
    let card_fill_opacity = if transparent { "0" } else { "0.68" };
    let mut body = String::new();
    for (index, (label, value)) in stats.iter().enumerate() {
        let x = margin + index as f32 * (card_w + gap);
        body.push_str(&format!(
            "<rect x=\"{x:.1}\" y=\"{card_y:.1}\" width=\"{card_w:.1}\" height=\"{card_h:.1}\" rx=\"20\" fill=\"{bg}\" fill-opacity=\"{card_fill_opacity}\" stroke=\"{border}\" stroke-width=\"2\"/><text x=\"{:.1}\" y=\"{:.1}\" fill=\"{accent}\" font-size=\"{}\" font-weight=\"700\">{}</text><text x=\"{:.1}\" y=\"{:.1}\" fill=\"{fg}\" font-size=\"{}\">{}</text>",
            x + 26.0, card_y + card_h * 0.52, (height / 13).max(24), xml_escape(value),
            x + 26.0, card_y + card_h * 0.80, (height / 35).max(15), xml_escape(label),
        ));
    }
    let detail_y = card_y + card_h + height as f32 * 0.12;
    let detail_size = (height / 31).max(17);
    let opened = top_opened.map(|(name, count)| format!("most opened  ·  {name} ({count})"))
        .unwrap_or_else(|| "most opened  ·  none yet".into());
    let edited = top_edited.map(|(name, count)| format!("most edited  ·  {name} ({count})"))
        .unwrap_or_else(|| "most edited  ·  none yet".into());
    body.push_str(&format!(
        "<text x=\"{margin:.1}\" y=\"{detail_y:.1}\" fill=\"{secondary}\" font-size=\"{detail_size}\">{}</text><text x=\"{margin:.1}\" y=\"{:.1}\" fill=\"{fg}\" font-size=\"{detail_size}\">{}</text>",
        xml_escape(&opened), detail_y + height as f32 * 0.075, xml_escape(&edited),
    ));
    let badge_y = height as f32 * 0.88;
    body.push_str(&format!(
        "<text x=\"{margin:.1}\" y=\"{badge_y:.1}\" fill=\"{accent}\" font-size=\"{detail_size}\" font-weight=\"700\">{} milestones unlocked  ·  {} themes collected</text>",
        app.cfg.stats.milestones.len(), app.cfg.stats.themes.len(),
    ));
    let background = if transparent { String::new() } else {
        format!("<rect width=\"100%\" height=\"100%\" fill=\"{bg}\"/><circle cx=\"{}\" cy=\"-30\" r=\"{}\" fill=\"{accent}\" fill-opacity=\"0.12\"/><circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{secondary}\" fill-opacity=\"0.10\"/>",
            width as f32 * 0.82, width as f32 * 0.42, width as f32 * 0.08, height as f32 * 0.9, width as f32 * 0.25)
    };
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\"><defs><linearGradient id=\"shine\" x2=\"1\" y2=\"1\"><stop stop-color=\"{accent}\"/><stop offset=\"1\" stop-color=\"{secondary}\"/></linearGradient></defs>{background}<text x=\"{}\" y=\"{}\" fill=\"url(#shine)\" font-family=\"sans-serif\" font-size=\"{}\" font-weight=\"800\">confy / personal stats</text><text x=\"{}\" y=\"{}\" fill=\"{fg}\" font-family=\"sans-serif\" font-size=\"{}\">your configs, your little corner of the unix universe</text>{body}</svg>",
        margin, height as f32 * 0.16, (height / 8).max(36), margin, height as f32 * 0.235, (height / 38).max(16),
    )
}

fn color(value: &str, fallback: &str) -> String {
    let value = value.to_lowercase();
    if value.len() == 7 && value.starts_with('#') && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return value;
    }
    match value.as_str() {
        "black" => "#000000".into(), "white" => "#ffffff".into(), "red" => "#f38ba8".into(),
        "green" => "#a6e3a1".into(), "yellow" => "#f9e2af".into(), "blue" => "#89b4fa".into(),
        "magenta" | "purple" | "pink" => "#cba6f7".into(), "cyan" => "#94e2d5".into(),
        "default" => fallback.into(), _ => fallback.into(),
    }
}

fn xml_escape(input: &str) -> String {
    input.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}
