use ratatui::{
    crossterm::event::{self, Event, KeyCode},
    widgets::{Block, Borders, List, ListItem, ListState},
    style::{Color, Modifier, Style},
};

struct App {
    files: Vec<String>,
    state: ListState,
}

impl App {
    fn new() -> App {
        let mut state = ListState::default();
        state.select(Some(0));
        App {
            files: vec![
                "~/.config/hypr/hyprland.conf".to_string(),
                "~/.config/kitty/kitty.conf".to_string(),
            ],
            state,
        }
    }

    fn next(&mut self) {
        let i = match self.state.selected() {
            Some(i) => (i + 1).min(self.files.len() - 1),
            None => 0,
        };
        self.state.select(Some(i));
    }

    fn prev(&mut self) {
        let i = match self.state.selected() {
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        self.state.select(Some(i));
    }
}

fn main() -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::new();

    loop {
        terminal.draw(|f| {
            let items: Vec<ListItem> = app.files
                .iter()
                .map(|f| ListItem::new(f.as_str()))
                .collect();

            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title("ruconfy"))
                .highlight_style(
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD)
                )
                .highlight_symbol(">> ");

            f.render_stateful_widget(list, f.area(), &mut app.state);
        })?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('j') | KeyCode::Down => app.next(),
                KeyCode::Char('k') | KeyCode::Up => app.prev(),
                _ => {}
            }
        }
    }

    ratatui::restore();
    Ok(())
}
