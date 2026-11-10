use crate::app::App;
use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MAX_BODY: usize = 12 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub enum WebMode {
    Gui,
    Text,
}

pub struct LaunchOptions {
    pub mode: Option<WebMode>,
    pub ip: Option<String>,
    pub port: Option<u16>,
    pub password: Option<String>,
    pub password_overridden: bool,
    pub no_interactive: bool,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            mode: None,
            ip: None,
            port: None,
            password: None,
            password_overridden: false,
            no_interactive: false,
        }
    }
}

impl LaunchOptions {
    pub fn prepare(&mut self) -> Result<()> {
        if self.no_interactive {
            self.mode.get_or_insert(WebMode::Gui);
            self.ip.get_or_insert_with(|| "127.0.0.1".into());
            self.port.get_or_insert(3000);
            return Ok(());
        }
        if self.mode.is_none() {
            let answer = prompt("mode [gui/text] (gui): ")?;
            self.mode = Some(if answer.eq_ignore_ascii_case("text") {
                WebMode::Text
            } else {
                WebMode::Gui
            });
        }
        if self.ip.is_none() {
            let answer = prompt("ip address (127.0.0.1): ")?;
            self.ip = Some(if answer.is_empty() { "127.0.0.1".into() } else { answer });
        }
        if self.port.is_none() {
            let answer = prompt("port (3000): ")?;
            self.port = Some(if answer.is_empty() { 3000 } else { answer.parse()? });
        }
        if !self.password_overridden {
            let answer = prompt("password (leave blank for none): ")?;
            self.password = (!answer.is_empty()).then_some(answer);
        }
        Ok(())
    }
}

fn prompt(label: &str) -> Result<String> {
    use std::io::{self, Write};
    print!("{label}");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().to_string())
}

pub fn serve(options: LaunchOptions) -> Result<()> {
    let ip = options.ip.as_deref().unwrap_or("127.0.0.1");
    let port = options.port.unwrap_or(3000);
    let ip_for_bind = if ip.contains(':') && !ip.starts_with('[') {
        format!("[{ip}]")
    } else {
        ip.to_string()
    };
    let bind = format!("{ip_for_bind}:{port}");
    let password = options.password;
    let default_mode = options.mode.unwrap_or(WebMode::Gui);
    let listener =
        TcpListener::bind(&bind).with_context(|| format!("couldn't bind web server to {bind}"))?;
    let app = Arc::new(Mutex::new(App::new()));
    eprintln!("confy web server listening on http://{bind}");
    if password.is_some()
        && !bind.starts_with("127.")
        && !bind.starts_with("localhost")
        && bind != "[::1]:8080"
    {
        eprintln!(
                "warning: http does not encrypt traffic; put confy behind a tls reverse proxy when exposed to a network"
        );
    }
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let password = password.clone();
                let default_mode = default_mode;
                let app = Arc::clone(&app);
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(15)));
                    if let Err(e) = handle(&mut stream, password.as_deref(), default_mode, app) {
                        eprintln!("web request failed: {e:#}");
                    }
                });
            }
            Err(e) => eprintln!("web connection failed: {e}"),
        }
    }
    Ok(())
}

struct Request {
    method: String,
    path: String,
    headers: String,
    body: Vec<u8>,
}

fn handle(
    stream: &mut TcpStream,
    password: Option<&str>,
    default_mode: WebMode,
    shared_app: Arc<Mutex<App>>,
) -> Result<()> {
    let req = read_request(stream)?;
    if let Some(password) = password {
        let supplied = req
            .headers
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("authorization")
                    .then(|| value.trim())
            })
            .and_then(|v| {
                let (scheme, credentials) = v.split_once(' ')?;
                scheme.eq_ignore_ascii_case("basic").then_some(credentials)
            })
            .and_then(decode_basic);
        let accepted = supplied
            .as_deref()
            .and_then(|v| v.split_once(':').map(|(_, p)| p))
            .map(|p| constant_time_eq(p.as_bytes(), password.as_bytes()))
            .unwrap_or(false);
        if !accepted {
            return respond(
                stream,
                "401 unauthorized",
                "text/html; charset=utf-8",
                "<h1>authentication required</h1>",
                Some("www-authenticate: basic realm=\"confy\", charset=\"utf-8\"\r\n"),
            );
        }
    }
    let mut app = shared_app.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let files = tracked_files(&app);
    let (path, query) = req.path.split_once('?').unwrap_or((&req.path, ""));
    let query_fields = parse_form(query);
    let fields = if req.method == "POST" {
        parse_form(&String::from_utf8_lossy(&req.body))
    } else {
        std::collections::HashMap::new()
    };
    if req.method == "POST" {
        let host = req
            .headers
            .lines()
            .find_map(|line| {
                let (k, v) = line.split_once(':')?;
                k.eq_ignore_ascii_case("host").then(|| v.trim())
            })
            .unwrap_or("");
        if let Some(origin) = req.headers.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.eq_ignore_ascii_case("origin").then(|| v.trim())
        }) {
            let expected_http = format!("http://{host}");
            let expected_https = format!("https://{host}");
            if origin != expected_http && origin != expected_https {
                return respond(
                    stream,
                    "403 forbidden",
                    "text/plain",
                    "cross-origin write rejected",
                    None,
                );
            }
        }
    }
    let mode = if path.starts_with("/text") {
        WebMode::Text
    } else if path.starts_with("/gui") {
        WebMode::Gui
    } else if fields.get("mode").map(String::as_str) == Some("text") {
        WebMode::Text
    } else if fields.get("mode").map(String::as_str) == Some("gui") {
        WebMode::Gui
    } else {
        default_mode
    };
    if req.method == "GET" && (path == "/" || path == "/gui" || path == "/text") {
        let selected = query
            .split('&')
            .find_map(|p| p.strip_prefix("selected="))
            .and_then(|v| v.parse::<usize>().ok());
        return respond(
            stream,
            "200 ok",
            "text/html; charset=utf-8",
            &index_page(&app, &files, mode, selected),
            None,
        );
    }
    if let Some(rest) = path.strip_prefix("/gui/edit/").or_else(|| path.strip_prefix("/text/edit/")) {
        let editor_mode = if path.starts_with("/gui/") { "gui" } else { "text" };
        let Ok(id) = rest.parse::<usize>() else {
            return not_found(stream);
        };
        let Some(tracked) = files.get(id) else {
            return not_found(stream);
        };
        let fullpath = app.local_path(tracked);
        let content = match std::fs::read_to_string(&fullpath) {
            Ok(content) => content,
            Err(e) => {
                return respond(
                    stream,
                    "500 internal server error",
                    "text/plain",
                    &format!("couldn't read {}: {e}", fullpath.display()),
                    None,
                );
            }
        };
        return respond(
            stream,
            "200 ok",
            "text/html; charset=utf-8",
            &editor_page(tracked, id, editor_mode, &content, &app.cfg.settings.colors),
            None,
        );
    }
    if req.method == "GET" && path == "/pick" {
        let group = query_fields.get("group").map(String::as_str).unwrap_or("ungrouped");
        let pick_mode = if query_fields.get("mode").map(String::as_str) == Some("text") { WebMode::Text } else { WebMode::Gui };
        return respond(stream, "200 ok", "text/html; charset=utf-8", &picker_page(&app, group, pick_mode), None);
    }
    if req.method == "POST" {
        if path == "/command" {
            let selected = fields.get("selected").and_then(|v| v.parse::<usize>().ok());
            if let Some(id) = selected {
                if let Some(tracked) = files.get(id) {
                    select_tracked(&mut app, tracked);
                }
            }
            let command = fields
                .get("command")
                .map(String::as_str)
                .unwrap_or("")
                .trim_start_matches(':');
            if command == "help" || command == "h" {
                return respond(stream, "200 ok", "text/html; charset=utf-8", &help_page(&app, mode), None);
            }
            if command == "l" || command == "su" {
                let tracked = if command == "l" {
                    app.cfg.last_opened.as_deref()
                } else {
                    selected.and_then(|id| files.get(id).map(String::as_str))
                };
                if let Some(tracked) = tracked {
                    if let Some(id) = files.iter().position(|p| p == tracked) {
                        let fullpath = app.local_path(tracked);
                        match std::fs::read_to_string(&fullpath) {
                            Ok(content) => return respond(stream, "200 ok", "text/html; charset=utf-8", &editor_page(tracked, id, if matches!(mode, WebMode::Text) { "text" } else { "gui" }, &content, &app.cfg.settings.colors), None),
                            Err(e) => return respond(stream, "500 internal server error", "text/plain", &format!("couldn't read {}: {e}", fullpath.display()), None),
                        }
                    }
                }
                let message = if command == "l" { "last opened file is not tracked" } else { "select a tracked file first" };
                return respond(stream, "200 ok", "text/html; charset=utf-8", &notice_page(&app, mode, message), None);
            }
            let result = crate::commands::handle(&mut app, command);
            let message = match result {
                crate::commands::CmdResult::NeedPicker { group } => {
                    return respond(stream, "200 ok", "text/html; charset=utf-8", &picker_page(&app, &group, mode), None);
                }
                crate::commands::CmdResult::NeedCdPicker => return respond(stream, "200 ok", "text/html; charset=utf-8", &command_page(&app, mode, selected, "config directory", &format!("cd {}", app.cfg.config_dir)), None),
                crate::commands::CmdResult::NeedGitDirPicker => return respond(stream, "200 ok", "text/html; charset=utf-8", &command_page(&app, mode, selected, "git directory", "git dir "), None),
                crate::commands::CmdResult::NeedThemeWizard => return respond(stream, "200 ok", "text/html; charset=utf-8", &theme_wizard_page(&app, mode), None),
                crate::commands::CmdResult::NeedThemeBrowser => return theme_browser(stream, &app, mode),
                crate::commands::CmdResult::NeedStats => app.stats_summary(),
                crate::commands::CmdResult::NeedDiff { .. } => "diff review and restore are available in the terminal tui".into(),
                crate::commands::CmdResult::NeedHooks => "edit hooks.toml from the terminal tui with :hooks".into(),
                crate::commands::CmdResult::NeedImport { .. } => "import review is available in the terminal: run `confy import`".into(),
                crate::commands::CmdResult::NeedCommitMsg { then_push } => {
                    let msg = fields.get("message").map(String::as_str).unwrap_or("web commit");
                    crate::commands::git_commit_and_maybe_push(&mut app, msg, then_push);
                    app.popup.take().unwrap_or_else(|| "git operation complete".into())
                }
                crate::commands::CmdResult::NeedBouncer => return respond(stream, "200 ok", "text/html; charset=utf-8", &bouncer_page(&app, mode), None),
                crate::commands::CmdResult::Quit => "the web server remains running; stop it from the terminal".into(),
                crate::commands::CmdResult::Ok => app.popup.take().unwrap_or_else(|| "command complete".into()),
            };
            return respond(stream, "200 ok", "text/html; charset=utf-8", &notice_page(&app, mode, &message), None);
        }
        if path == "/bouncer-save" {
            let chosen: std::collections::HashSet<usize> = fields.get("items").into_iter().flat_map(|v| v.split(',')).filter_map(|v| v.parse().ok()).collect();
            let mut count = 0;
            for (index, (is_group, key)) in blacklist_entries(&app).iter().enumerate() {
                if *is_group {
                    if chosen.contains(&index) { crate::git::blacklist_group(&mut app, key); count += 1; } else { crate::git::unblacklist_group(&mut app, key); }
                } else if chosen.contains(&index) {
                    crate::git::blacklist_file(&mut app, key);
                    count += 1;
                } else {
                    crate::git::unblacklist_file(&mut app, key);
                }
            }
            return respond(stream, "200 ok", "text/html; charset=utf-8", &notice_page(&app, mode, &format!("git blacklist updated · {count} excluded")), None);
        }
        if path == "/create-theme" {
            let name = fields.get("name").map(String::as_str).unwrap_or("");
            let colors = crate::config::ColorSettings {
                bg: fields.get("bg").cloned().unwrap_or_default(),
                fg: fields.get("fg").cloned().unwrap_or_default(),
                highlight: fields.get("highlight").cloned().unwrap_or_default(),
                group: fields.get("group").cloned().unwrap_or_default(),
                border: fields.get("border").cloned().unwrap_or_default(),
            };
            let message = match crate::config::save_custom_theme(name, &colors) {
                Ok(()) => match app.set_theme(name) {
                    Ok(()) => format!("theme '{name}' saved and applied"),
                    Err(e) => format!("theme saved, but couldn't apply it: {e}"),
                },
                Err(e) => format!("couldn't save theme: {e}"),
            };
            return respond(stream, "200 ok", "text/html; charset=utf-8", &notice_page(&app, mode, &message), None);
        }
        if path == "/install-theme" {
            let Some(id) = fields.get("id").and_then(|v| v.parse::<usize>().ok()) else { return not_found(stream); };
            let themes = match crate::themenet::fetch_themes() {
                Ok(themes) => themes,
                Err(e) => return respond(stream, "502 bad gateway", "text/plain", &format!("couldn't load themes: {e}"), None),
            };
            let Some(theme) = themes.get(id) else { return not_found(stream); };
            let message = match crate::config::install_custom_theme(&theme.id, &theme.theme_toml).and_then(|_| app.set_theme(&theme.id)) {
                Ok(()) => format!("installed and applied '{}'", theme.meta.name),
                Err(e) => format!("couldn't install theme: {e}"),
            };
            return respond(stream, "200 ok", "text/html; charset=utf-8", &notice_page(&app, mode, &message), None);
        }
        if path == "/add" {
            let Some(file) = fields.get("path") else { return not_found(stream); };
            let group = fields.get("group").map(String::as_str).unwrap_or("ungrouped");
            let path = Path::new(file);
            if !path.is_file() {
                return respond(stream, "400 bad request", "text/plain", "file not found", None);
            }
            app.track_file(path, group);
            let selected_mode = if fields.get("mode").map(String::as_str) == Some("text") { WebMode::Text } else { WebMode::Gui };
            let files = tracked_files(&app);
            return respond(stream, "200 ok", "text/html; charset=utf-8", &index_page(&app, &files, selected_mode, None), None);
        }
        if let Some(rest) = path.strip_prefix("/save/") {
            let Ok(id) = rest.parse::<usize>() else {
                return not_found(stream);
            };
            let Some(tracked) = files.get(id) else {
                return not_found(stream);
            };
            let Some(content) = fields.get("content") else {
                return respond(
                    stream,
                    "400 bad request",
                    "text/plain",
                    "missing content",
                    None,
                );
            };
            if let Err(e) = app.backup_before_write(tracked) {
                return respond(
                    stream,
                    "500 internal server error",
                    "text/plain",
                    &format!("couldn't create backup: {e}"),
                    None,
                );
            }
            let fullpath = app.local_path(tracked);
            if let Err(e) = std::fs::write(&fullpath, content) {
                return respond(
                    stream,
                    "500 internal server error",
                    "text/plain",
                    &format!("couldn't write {}: {e}", fullpath.display()),
                    None,
                );
            }
            let edit_mode = if fields.get("mode").map(String::as_str) == Some("text") {
                "text"
            } else {
                "gui"
            };
            return respond(
                stream,
                "303 see other",
                "text/plain",
                "saved",
                Some(&format!(
                    "location: /{edit_mode}?selected={id}\r\n"
                )),
            );
        }
    }
    not_found(stream)
}

fn tracked_files(app: &App) -> Vec<String> {
    let mut groups: Vec<_> = app.cfg.groups.iter().collect();
    groups.sort_by_key(|(name, _)| *name);
    groups
        .into_iter()
        .flat_map(|(_, files)| files.iter().cloned())
        .collect()
}

fn select_tracked(app: &mut App, tracked: &str) {
    for group in app.cfg.groups.keys() {
        if app.cfg.groups[group].iter().any(|path| path == tracked) {
            app.cfg.collapsed_groups.remove(group);
            break;
        }
    }
    app.rebuild_flat_view();
    if let Some(index) = app.flat_view.iter().position(|item| {
        matches!(item, crate::app::Item::File { path, .. } if path == tracked)
    }) {
        app.selected = index;
    }
}

fn blacklist_entries(app: &App) -> Vec<(bool, String)> {
    let mut groups: Vec<_> = app.cfg.groups.iter().collect();
    groups.sort_by_key(|(name, _)| *name);
    let mut out = Vec::new();
    for (group, files) in groups {
        if group != "ungrouped" {
            out.push((true, group.clone()));
        }
        for path in files {
            out.push((false, path.clone()));
        }
    }
    out
}

fn picker_page(app: &App, group: &str, mode: WebMode) -> String {
    let mode_value = if matches!(mode, WebMode::Text) { "text" } else { "gui" };
    let back = if matches!(mode, WebMode::Text) { "/text" } else { "/gui" };
    format!(
        "{}<header><a href=\"{back}\">← files</a><h1>add a tracked file</h1></header><main><form method=\"post\" action=\"/add\"><label>file path<input name=\"path\" required placeholder=\"/home/user/.config/example.conf\"></label><label>group<input name=\"group\" value=\"{}\"></label><input type=\"hidden\" name=\"mode\" value=\"{mode_value}\"><button>add file</button></form></main></body></html>",
        page_start("add file", &app.cfg.settings.colors),
        html(group)
    )
}

fn notice_page(app: &App, mode: WebMode, message: &str) -> String {
    let href = if matches!(mode, WebMode::Text) { "/text" } else { "/gui" };
    format!(
        "{}<header><h1>confy</h1></header><main><p class=\"notice\">{}</p><a href=\"{href}\">continue</a></main></body></html>",
        page_start("confy", &app.cfg.settings.colors),
        html(message)
    )
}

fn command_page(app: &App, mode: WebMode, selected: Option<usize>, label: &str, command: &str) -> String {
    let back = if matches!(mode, WebMode::Text) { "/text" } else { "/gui" };
    let mode_value = if matches!(mode, WebMode::Text) { "text" } else { "gui" };
    format!(
        "{}<header><a href=\"{back}\">← files</a><h1>set {}</h1></header><main><form method=\"post\" action=\"/command\"><label>command<input name=\"command\" value=\"{}\" autofocus></label><input type=\"hidden\" name=\"selected\" value=\"{}\"><input type=\"hidden\" name=\"mode\" value=\"{mode_value}\"><button>run</button></form></main></body></html>",
        page_start(label, &app.cfg.settings.colors), html(label), html(command), selected.map(|v| v.to_string()).unwrap_or_default()
    )
}

fn help_page(app: &App, mode: WebMode) -> String {
    let back = if matches!(mode, WebMode::Text) { "/text" } else { "/gui" };
    format!(
        "{}<header><a href=\"{back}\">← files</a><h1>command reference</h1></header><main><pre>:ac [group]     add a tracked file
:rm             remove the selected file
:l              edit the last opened file
:rb             restore the selected file's latest backup
:su             edit the selected file in the browser
:ag <name>       add a group
:rg <name>       remove a group
:mg <name>       move a file into a group
:sort name|date|size
:reverse        reverse the file order
:cd <path>      change the config directory
:theme <name>   switch theme
:theme list     list installed themes
:theme new      create a custom theme
:theme delete <name>
:themenet       browse community themes
:bl             toggle the selected file/group git blacklist
:bouncer        manage the full git blacklist
:git sync|commit|push
:git dir <path> | :git reset
:device <host>|local
:streamer       toggle streamer mode
:h              show this reference
:q              return to the running server</pre></main></body></html>",
        page_start("command reference", &app.cfg.settings.colors)
    )
}

fn theme_wizard_page(app: &App, mode: WebMode) -> String {
    let c = &app.cfg.settings.colors;
    let from = if matches!(mode, WebMode::Text) { "/text" } else { "/gui" };
    format!(
        "{}<header><a href=\"{from}\">← back</a><h1>create a theme</h1></header><main><form method=\"post\" action=\"/create-theme\"><input type=\"hidden\" name=\"mode\" value=\"{}\"><label>theme name<input name=\"name\" required></label><label>background<input name=\"bg\" value=\"{}\"></label><label>foreground<input name=\"fg\" value=\"{}\"></label><label>highlight<input name=\"highlight\" value=\"{}\"></label><label>group color<input name=\"group\" value=\"{}\"></label><label>border<input name=\"border\" value=\"{}\"></label><button>save and apply</button></form></main></body></html>",
        page_start("create theme", &app.cfg.settings.colors),
        if matches!(mode, WebMode::Text) { "text" } else { "gui" },
        html(&c.bg), html(&c.fg), html(&c.highlight), html(&c.group), html(&c.border)
    )
}

fn theme_browser(stream: &mut TcpStream, app: &App, mode: WebMode) -> Result<()> {
    let themes = match crate::themenet::fetch_themes() {
        Ok(themes) => themes,
        Err(e) => return respond(stream, "502 bad gateway", "text/plain", &format!("couldn't load community themes: {e}"), None),
    };
    let mut out = page_start("community themes", &app.cfg.settings.colors);
    let back = if matches!(mode, WebMode::Text) { "/text" } else { "/gui" };
    let mode_value = if matches!(mode, WebMode::Text) { "text" } else { "gui" };
    out.push_str(&format!("<header><a href=\"{back}\">← files</a><h1>community themes</h1></header><main>"));
    for (i, theme) in themes.iter().enumerate() {
        out.push_str(&format!("<section><h2>{}</h2><p>by {}</p><p>tags: {}</p><p>{}</p><details><summary>theme.toml</summary><pre>{}</pre></details><details><summary>meta.toml</summary><pre>{}</pre></details><form method=\"post\" action=\"/install-theme\"><input type=\"hidden\" name=\"id\" value=\"{i}\"><input type=\"hidden\" name=\"mode\" value=\"{mode_value}\"><button>install and apply</button></form></section>", html(&theme.meta.name), html(&theme.meta.author), html(&theme.meta.tags.join(", ")), html(&theme.meta.description), html(&theme.theme_toml), html(&theme.meta_toml)));
    }
    out.push_str("</main></body></html>");
    respond(stream, "200 ok", "text/html; charset=utf-8", &out, None)
}

fn bouncer_page(app: &App, mode: WebMode) -> String {
    let back = if matches!(mode, WebMode::Text) { "/text" } else { "/gui" };
    let mode_value = if matches!(mode, WebMode::Text) { "text" } else { "gui" };
    let mut out = page_start("git blacklist", &app.cfg.settings.colors);
    out.push_str(&format!("<header><a href=\"{back}\">← files</a><h1>git blacklist</h1><p>choose files or groups to exclude from git sync</p></header><main><form method=\"post\" action=\"/bouncer-save\"><input type=\"hidden\" name=\"mode\" value=\"{mode_value}\"><ul>"));
    for (i, (is_group, key)) in blacklist_entries(app).iter().enumerate() {
        let (label, checked) = if *is_group {
            (format!("group · {key}/"), crate::git::is_group_blacklisted(app, key))
        } else {
            let name = Path::new(key).file_name().unwrap_or_default().to_string_lossy();
            (format!("file · {name}"), crate::git::is_blacklisted(app, key))
        };
        out.push_str(&format!("<li><label><input type=\"checkbox\" class=\"blacklist-item\" value=\"{i}\" {}> {}</label></li>", if checked { "checked" } else { "" }, html(&label)));
    }
    out.push_str("</ul><input type=\"hidden\" name=\"items\" id=\"items\"><button>save blacklist</button></form><script>document.querySelector('form').addEventListener('submit',()=>document.querySelector('#items').value=[...document.querySelectorAll('.blacklist-item:checked')].map(x=>x.value).join(','))</script></main></body></html>");
    out
}

fn index_page(app: &App, files: &[String], mode: WebMode, selected: Option<usize>) -> String {
    let mut out = page_start("confy", &app.cfg.settings.colors);
    out.push_str(&format!("<header><h1>confy</h1><p>configuration manager</p><nav><a href=\"/gui\">gui mode</a> · <a href=\"/text\">text mode</a></nav></header><main>"));
    if matches!(mode, WebMode::Gui) {
        out.push_str("<div class=\"toolbar\"><a class=\"button\" href=\"/pick?group=ungrouped&mode=gui\">add file</a><a class=\"button\" id=\"edit-selected\" href=\"/gui/edit/0\">edit selected</a><span>theme: ");
        out.push_str(&html(&app.cfg.settings.theme));
        out.push_str("</span></div><form class=\"command\" method=\"post\" action=\"/command\"><label for=\"cmd\">command</label><input id=\"cmd\" name=\"command\" placeholder=\":theme list, :rb, :sort name, :git sync, :help\"><input name=\"message\" placeholder=\"commit message\"><input type=\"hidden\" name=\"mode\" value=\"gui\"><input type=\"hidden\" name=\"selected\" id=\"selected\"><button>run</button></form>");
        let mut groups: Vec<_> = app.cfg.groups.iter().collect();
        groups.sort_by_key(|(name, _)| *name);
        let mut i = 0;
        for (group, paths) in groups {
            out.push_str(&format!("<section><h2>{}</h2><ul>", html(group)));
            for path in paths {
                let file_name = Path::new(path).file_name().unwrap_or_default().to_string_lossy();
                out.push_str(&format!("<li class=\"{}\" onclick=\"selectfile({i},this)\"><strong>{}</strong><small>{}</small><div><a href=\"/gui/edit/{i}\">edit</a> · <a href=\"/text/edit/{i}\">text edit</a></div></li>", if selected == Some(i) { "selected" } else { "" }, html(&file_name), html(path)));
                i += 1;
            }
            out.push_str("</ul></section>");
        }
        if let Some(path) = selected.and_then(|id| files.get(id)) {
            if let Ok(content) = std::fs::read_to_string(app.local_path(path)) {
                let preview = content.lines().take(40).collect::<Vec<_>>().join("\n");
                out.push_str(&format!("<section class=\"preview\"><h2>preview · {}</h2><pre>{}</pre></section>", html(Path::new(path).file_name().unwrap_or_default().to_string_lossy().as_ref()), html(&preview)));
            }
        }
        out.push_str("<script>function selectfile(i,e){document.querySelectorAll('li').forEach(x=>x.classList.remove('selected'));e.classList.add('selected');document.querySelector('#selected').value=i;document.querySelector('#edit-selected').href='/gui/edit/'+i}const q=new URLSearchParams(location.search).get('selected');if(q!==null){const x=document.querySelectorAll('li')[+q];if(x)selectfile(+q,x)}</script>");
    } else {
        out.push_str("<div class=\"terminal\" tabindex=\"0\" id=\"terminal\"><div>confy · text mode</div><div>theme: ");
        out.push_str(&html(&app.cfg.settings.theme));
        out.push_str("</div><div>j/k move · enter edit · : command · q return to gui</div><pre id=\"screen\">");
        let mut groups: Vec<_> = app.cfg.groups.iter().collect();
        groups.sort_by_key(|(name, _)| *name);
        let mut i = 0;
        for (group, paths) in groups {
            out.push_str(&format!("\n{}/\n", html(group)));
            for path in paths {
                let name = Path::new(path).file_name().unwrap_or_default().to_string_lossy();
                out.push_str(&format!("{}  {}\n", if selected == Some(i) { "▶" } else { " " }, html(&name)));
                i += 1;
            }
        }
        if files.is_empty() { out.push_str("\nno tracked files. add files in gui mode or with :ac in the terminal.\n"); }
        out.push_str("</pre><form method=\"post\" action=\"/command\" id=\"cmdform\"><input name=\"command\" id=\"cmd\" autocomplete=\"off\" placeholder=\":\"><input name=\"message\" placeholder=\"commit message\"><input type=\"hidden\" name=\"mode\" value=\"text\"><input type=\"hidden\" name=\"selected\" id=\"selected\"><button>run</button></form></div><script>");
        out.push_str(&format!("let selected={};document.addEventListener('keydown',e=>{{const c=document.querySelector('#cmd');if(document.activeElement===c&&e.key==='Enter')return;if(document.activeElement===c)return;if(e.key==='j'||e.key==='ArrowDown'){{selected=Math.min({},selected+1);location.replace('/text?selected='+selected)}}else if(e.key==='k'||e.key==='ArrowUp'){{selected=Math.max(0,selected-1);location.replace('/text?selected='+selected)}}else if(e.key==='Enter'){{location.href='/text/edit/'+selected}}else if(e.key===':'){{e.preventDefault();c.value=':';c.focus()}}else if(e.key==='q'){{location.href='/gui'}}}});document.querySelector('#selected').value=selected;", selected.unwrap_or(0), files.len().saturating_sub(1)));
        out.push_str("</script>");
    }
    out.push_str("</main></body></html>");
    out
}

fn editor_page(
    path: &str,
    id: usize,
    mode: &str,
    content: &str,
    colors: &crate::config::ColorSettings,
) -> String {
    let title = Path::new(path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let mut out = page_start(&title, colors);
    let base = if mode == "text" { "/text" } else { "/gui" };
    out.push_str(&format!("<header><a href=\"{base}\">← files</a><h1>"));
    out.push_str(&html(&title));
    out.push_str("</h1><p>");
    out.push_str(&html(path));
    out.push_str("</p><nav>");
    out.push_str(&format!("<a href=\"/gui\">gui mode</a> · <a href=\"/text\">text mode</a></nav></header><main>"));
    if mode == "text" {
        out.push_str(
            "<div class=\"hint\" id=\"hint\">normal · press i to insert, :wq to save</div>",
        );
    }
    out.push_str(&format!("<form method=\"post\" action=\"/save/{id}\"><input type=\"hidden\" name=\"mode\" value=\"{mode}\"><textarea name=\"content\" id=\"editor\" spellcheck=\"false\">{}</textarea><button>save</button></form>", html(content)));
    if mode == "text" {
        out.push_str(r#"<script>
const e=document.querySelector('#editor'),h=document.querySelector('#hint');let insert=false,command='';
e.addEventListener('keydown',ev=>{if(ev.ctrlKey&&ev.key.toLowerCase()==='s'){ev.preventDefault();e.form.requestSubmit();return}if(insert){if(ev.key==='Escape'){insert=false;h.textContent='normal'}else if(ev.key==='Tab'){ev.preventDefault();e.setRangeText('    ',e.selectionStart,e.selectionEnd,'end')}return}ev.preventDefault();if(ev.key==='i'){insert=true;h.textContent='insert'}else if(ev.key==='Escape'){command='';h.textContent='normal'}else if(command.startsWith(':')){if(ev.key==='Enter'){const cmd=command;command='';h.textContent='normal';if(cmd===':wq')e.form.requestSubmit();else if(cmd===':q!')location.href='/text'}else if(ev.key==='Backspace'){command=command.slice(0,-1);h.textContent=command||'normal'}else if(ev.key.length===1){command+=ev.key;h.textContent=command}}else if(ev.key===':'){command=':';h.textContent=command}else if(ev.key==='h'||ev.key==='ArrowLeft'){e.setSelectionRange(Math.max(0,e.selectionStart-1),Math.max(0,e.selectionStart-1))}else if(ev.key==='l'||ev.key==='ArrowRight'){e.setSelectionRange(Math.min(e.value.length,e.selectionStart+1),Math.min(e.value.length,e.selectionStart+1))}else if(ev.key==='j'||ev.key==='ArrowDown'){const p=Math.min(e.value.length,e.selectionStart+e.cols);e.setSelectionRange(p,p)}else if(ev.key==='k'||ev.key==='ArrowUp'){const p=Math.max(0,e.selectionStart-e.cols);e.setSelectionRange(p,p)}});
</script>"#);
    } else {
        out.push_str(r#"<script>
const e=document.querySelector('#editor');e.addEventListener('keydown',ev=>{if(ev.ctrlKey&&ev.key.toLowerCase()==='s'){ev.preventDefault();e.form.requestSubmit()}else if(ev.key==='Tab'){ev.preventDefault();e.setRangeText('    ',e.selectionStart,e.selectionEnd,'end')}});
</script>"#);
    }
    out.push_str("</main></body></html>");
    out
}

fn page_start(title: &str, colors: &crate::config::ColorSettings) -> String {
    let bg = css_color(&colors.bg, "#111318");
    let fg = css_color(&colors.fg, "#e6e8ef");
    let hi = css_color(&colors.highlight, "#8da2ff");
    let group = css_color(&colors.group, "#a9b8ff");
    let border = css_color(&colors.border, "#555b6b");
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{}</title><link rel=\"preconnect\" href=\"https://fonts.googleapis.com\"><link href=\"https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;600&display=swap\" rel=\"stylesheet\"><style>body{{margin:0;background:{bg};color:{fg};font:15px system-ui,sans-serif}}header,main{{max-width:1100px;margin:auto;padding:1rem 1.5rem}}header{{border-bottom:1px solid {border}}}h1,h2{{margin:.3em 0;color:{group}}}a{{color:{hi}}}section{{margin:1.5rem 0}}ul{{list-style:none;padding:0}}li{{padding:1rem;border-bottom:1px solid {border};cursor:pointer}}li.selected{{outline:2px solid {hi}}}small{{display:block;color:{fg};opacity:.75;margin:.3rem 0}}textarea{{box-sizing:border-box;width:100%;height:70vh;background:{bg};color:{fg};border:1px solid {border};padding:1rem;font:14px 'JetBrains Mono',monospace;line-height:1.5;tab-size:4;white-space:pre;overflow:auto}}input{{box-sizing:border-box;background:{bg};color:{fg};border:1px solid {border};padding:.65rem;font:inherit}}label{{display:block;margin:.8rem 0}}button,.button{{display:inline-block;margin:.4rem;padding:.7rem 1.3rem;background:{hi};color:{bg};border:0;font-weight:600;cursor:pointer;text-decoration:none}}.toolbar,.command{{display:flex;align-items:center;gap:.7rem;margin:1rem 0}}.command input[name=command]{{flex:1}}.hint,.terminal{{font:14px 'JetBrains Mono',monospace;padding:.8rem;border:1px solid {border}}}.terminal pre{{white-space:pre-wrap}}.notice{{padding:1rem;border-left:3px solid {hi}}}pre{{overflow:auto;white-space:pre-wrap}}</style></head><body>",
        html(title),
        bg = bg,
        fg = fg,
        hi = hi,
        group = group,
        border = border
    )
}

fn css_color(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return value.to_string();
    }
    if matches!(
        value.to_ascii_lowercase().as_str(),
        "black"
            | "white"
            | "red"
            | "green"
            | "yellow"
            | "blue"
            | "magenta"
            | "cyan"
            | "gray"
            | "grey"
            | "orange"
            | "purple"
            | "pink"
            | "teal"
            | "navy"
    ) {
        return value.to_ascii_lowercase();
    }
    fallback.to_string()
}

fn read_request(stream: &mut TcpStream) -> Result<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end;
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            bail!("incomplete http request");
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            header_end = i + 4;
            break;
        }
        if buf.len() > 32768 {
            bail!("http headers too large");
        }
    }
    let headers = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut first = headers.lines().next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("").to_owned();
    let path = first.next().unwrap_or("/").to_owned();
    let length = headers
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    if length > MAX_BODY {
        bail!("request body too large");
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            bail!("incomplete body");
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Ok(Request {
        method,
        path,
        headers,
        body,
    })
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
    extra: Option<&str>,
) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\nx-content-type-options: nosniff\r\ncontent-security-policy: default-src 'self'; font-src https://fonts.gstatic.com; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; script-src 'unsafe-inline'\r\n{}\r\n{}",
        body.len(),
        extra.unwrap_or(""),
        body
    )?;
    Ok(())
}
fn not_found(stream: &mut TcpStream) -> Result<()> {
    respond(stream, "404 not found", "text/plain", "not found", None)
}
fn html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1
    }
    String::from_utf8_lossy(&out).into_owned()
}
fn parse_form(s: &str) -> std::collections::HashMap<String, String> {
    s.split('&')
        .filter_map(|part| {
            let (k, v) = part.split_once('=')?;
            Some((decode(k), decode(v)))
        })
        .collect()
}
fn decode_basic(s: &str) -> Option<String> {
    const ALPH: &[u8] = b"abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for b in s.bytes() {
        if b == b'=' {
            break;
        }
        let v = ALPH.iter().position(|x| *x == b)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8)
        }
    }
    String::from_utf8(out).ok()
}
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0)
    }
    diff == 0
}
