use serde::Deserialize;
use std::process::{Command, Stdio};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct HooksFile {
    hooks: HookCommands,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct HookCommands {
    opened: Vec<String>,
    changed: Vec<String>,
    rolled_back: Vec<String>,
    restored: Vec<String>,
    tracked: Vec<String>,
    untracked: Vec<String>,
}

pub fn run(event: &str, tracked: &str) {
    let path = crate::config::hooks_file();
    let Ok(raw) = std::fs::read_to_string(path) else { return; };
    let hooks: HooksFile = match toml::from_str(&raw) {
        Ok(hooks) => hooks,
        Err(error) => {
            eprintln!("warn: couldn't parse hooks.toml: {error}");
            return;
        }
    };
    let commands = match event {
        "opened" => &hooks.hooks.opened,
        "changed" => &hooks.hooks.changed,
        "rolled_back" => &hooks.hooks.rolled_back,
        "restored" => &hooks.hooks.restored,
        "tracked" => &hooks.hooks.tracked,
        "untracked" => &hooks.hooks.untracked,
        _ => return,
    };
    for command in commands {
        let _ = Command::new("sh")
            .args(["-c", command])
            .env("CONFY_EVENT", event)
            .env("CONFY_FILE", tracked)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

pub fn ensure_file() -> anyhow::Result<std::path::PathBuf> {
    let path = crate::config::hooks_file();
    if !path.exists() {
        std::fs::create_dir_all(crate::config::confy_config_dir())?;
        std::fs::write(&path, TEMPLATE)?;
    }
    Ok(path)
}

const TEMPLATE: &str = r#"# commands run through sh -c after the matching event.
# commands receive $CONFY_EVENT and $CONFY_FILE.

[hooks]
opened = []
changed = []
rolled_back = []
restored = []
tracked = []
untracked = []

# example:
# changed = ["notify-send $CONFY_FILE"]
"#;
