use std::io;
use std::sync::mpsc::{self, Sender, Receiver};
use std::thread;
use std::time::Duration;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, BorderType, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, KeyCode, KeyModifiers, KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use unicode_width::UnicodeWidthStr;
// Event enum to handle both keyboard/mouse events and backend worker responses
enum Event {
    Input(KeyEvent),
    Mouse(MouseEvent),
    Resize,
    Backend(AppResponse),
}

// Request sent to background worker thread
enum AppRequest {
    Fetch { repo: String, query: String },
    Close { repo: String, number: u32 },
    Reopen { repo: String, number: u32 },
}

// Response from background worker thread
enum AppResponse {
    FetchSuccess(Vec<Issue>),
    FetchError(String),
    CloseSuccess(u32),
    CloseError { number: u32, err: String },
    ReopenSuccess(u32),
    ReopenError { number: u32, err: String },
}

// Struct representing a GitHub Issue deserialized from gh JSON output
#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct Issue {
    number: u32,
    title: String,
    state: String,
    author: Author,
    labels: Vec<Label>,
    updated_at: String,
    body: String,
}

#[derive(serde::Deserialize, Clone, Debug)]
struct Author {
    login: String,
}

#[derive(serde::Deserialize, Clone, Debug)]
struct Label {
    name: String,
    color: Option<String>,
}

// Helper to parse hex colors from GitHub labels into Ratatui Colors
fn parse_hex_color(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(128);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(128);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(128);
        Color::Rgb(r, g, b)
    } else {
        Color::DarkGray
    }
}

// Helper to get text brightness to decide if labels should have white or black text
fn get_text_color_for_bg(bg_color: Color) -> Color {
    if let Color::Rgb(r, g, b) = bg_color {
        // Simple luminance formula
        let luma = 0.299 * (r as f32) + 0.587 * (g as f32) + 0.114 * (b as f32);
        if luma > 150.0 {
            Color::Black
        } else {
            Color::White
        }
    } else {
        Color::White
    }
}

// Detect current git repository via gh CLI
fn detect_repo() -> Option<String> {
    let output = std::process::Command::new("gh")
        .args(&["repo", "view", "--json", "owner,name"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct RepoInfo {
        owner: Owner,
        name: String,
    }
    #[derive(serde::Deserialize)]
    struct Owner {
        login: String,
    }
    let info: RepoInfo = serde_json::from_slice(&output.stdout).ok()?;
    Some(format!("{}/{}", info.owner.login, info.name))
}



fn convert_text(core_text: ratatui_core::text::Text<'_>) -> ratatui::text::Text<'static> {
    let mut lines = Vec::new();
    for core_line in core_text.lines {
        let mut spans = Vec::new();
        for core_span in core_line.spans {
            let mut style = ratatui::style::Style::default();
            if let Some(fg) = core_span.style.fg {
                style = style.fg(convert_color(fg));
            }
            if let Some(bg) = core_span.style.bg {
                style = style.bg(convert_color(bg));
            }
            style = style.add_modifier(convert_modifier(core_span.style.add_modifier));
            style = style.remove_modifier(convert_modifier(core_span.style.sub_modifier));
            
            let content = core_span.content.into_owned();
            spans.push(ratatui::text::Span::styled(content, style));
        }
        lines.push(ratatui::text::Line::from(spans));
    }
    ratatui::text::Text::from(lines)
}

fn convert_color(c: ratatui_core::style::Color) -> ratatui::style::Color {
    match c {
        ratatui_core::style::Color::Reset => ratatui::style::Color::Reset,
        ratatui_core::style::Color::Black => ratatui::style::Color::Black,
        ratatui_core::style::Color::Red => ratatui::style::Color::Red,
        ratatui_core::style::Color::Green => ratatui::style::Color::Green,
        ratatui_core::style::Color::Yellow => ratatui::style::Color::Yellow,
        ratatui_core::style::Color::Blue => ratatui::style::Color::Blue,
        ratatui_core::style::Color::Magenta => ratatui::style::Color::Magenta,
        ratatui_core::style::Color::Cyan => ratatui::style::Color::Cyan,
        ratatui_core::style::Color::Gray => ratatui::style::Color::Gray,
        ratatui_core::style::Color::DarkGray => ratatui::style::Color::DarkGray,
        ratatui_core::style::Color::LightRed => ratatui::style::Color::LightRed,
        ratatui_core::style::Color::LightGreen => ratatui::style::Color::LightGreen,
        ratatui_core::style::Color::LightYellow => ratatui::style::Color::LightYellow,
        ratatui_core::style::Color::LightBlue => ratatui::style::Color::LightBlue,
        ratatui_core::style::Color::LightMagenta => ratatui::style::Color::LightMagenta,
        ratatui_core::style::Color::LightCyan => ratatui::style::Color::LightCyan,
        ratatui_core::style::Color::White => ratatui::style::Color::White,
        ratatui_core::style::Color::Rgb(r, g, b) => ratatui::style::Color::Rgb(r, g, b),
        ratatui_core::style::Color::Indexed(i) => ratatui::style::Color::Indexed(i),
    }
}

fn convert_modifier(m: ratatui_core::style::Modifier) -> ratatui::style::Modifier {
    ratatui::style::Modifier::from_bits_truncate(m.bits())
}

fn clean_github_markdown(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    let mut in_comment = false;
    let mut i = 0;
    let chars: Vec<char> = text.chars().collect();
    
    while i < chars.len() {
        if in_comment {
            if i + 2 < chars.len() && chars[i] == '-' && chars[i+1] == '-' && chars[i+2] == '>' {
                in_comment = false;
                i += 3;
            } else {
                i += 1;
            }
        } else if i + 3 < chars.len() && chars[i] == '<' && chars[i+1] == '!' && chars[i+2] == '-' && chars[i+3] == '-' {
            in_comment = true;
            i += 4;
        } else {
            cleaned.push(chars[i]);
            i += 1;
        }
    }
    
    cleaned
        .replace("<sub>", "_")
        .replace("</sub>", "_")
        .replace("<sup>", "_")
        .replace("</sup>", "_")
        .replace("<b>", "**")
        .replace("</b>", "**")
        .replace("<strong>", "**")
        .replace("</strong>", "**")
        .replace("<i>", "_")
        .replace("</i>", "_")
        .replace("<em>", "_")
        .replace("</em>", "_")
        .replace("<code>", "`")
        .replace("</code>", "`")
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
}

fn truncate_str_by_width(s: &str, max_width: usize) -> String {
    let mut current_width = 0;
    let mut result = String::new();
    
    for c in s.chars() {
        let char_str = c.to_string();
        let char_width = char_str.width();
        if current_width + char_width > max_width {
            break;
        }
        result.push(c);
        current_width += char_width;
    }
    
    if result.len() < s.len() {
        if !result.is_empty() {
            result.pop();
            result.push('…');
        }
    }
    
    result
}

// App state management
struct App {
    repo: String,
    issues: Vec<Issue>,
    list_state: ListState,
    search_query: String,
    search_focused: bool,
    details_scroll: usize,
    status_log: String,
    is_loading: bool,
    closing_issues: std::collections::HashSet<u32>,
    reopening_issues: std::collections::HashSet<u32>,
    req_tx: Sender<AppRequest>,
}

impl App {
    fn new(repo: String, req_tx: Sender<AppRequest>) -> Self {
        Self {
            repo,
            issues: Vec::new(),
            list_state: ListState::default(),
            search_query: String::new(),
            search_focused: false,
            details_scroll: 0,
            status_log: String::from("Press '/' or click search to search. 'c' to close issues."),
            is_loading: false,
            closing_issues: std::collections::HashSet::new(),
            reopening_issues: std::collections::HashSet::new(),
            req_tx,
        }
    }

    fn selected_issue(&self) -> Option<&Issue> {
        self.list_state.selected().and_then(|i| self.issues.get(i))
    }

    fn select_next(&mut self) {
        if self.issues.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.issues.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
        self.details_scroll = 0;
    }

    fn select_prev(&mut self) {
        if self.issues.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.issues.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
        self.details_scroll = 0;
    }

    fn trigger_fetch(&mut self) {
        self.is_loading = true;
        self.status_log = format!("Loading issues for {}...", self.repo);
        let _ = self.req_tx.send(AppRequest::Fetch {
            repo: self.repo.clone(),
            query: self.search_query.clone(),
        });
    }

    fn trigger_close(&mut self) {
        if let Some(issue) = self.selected_issue() {
            let num = issue.number;
            if self.closing_issues.contains(&num) {
                return;
            }
            self.closing_issues.insert(num);
            self.status_log = format!("Closing issue #{}...", num);
            let _ = self.req_tx.send(AppRequest::Close {
                repo: self.repo.clone(),
                number: num,
            });
        }
    }

    fn trigger_reopen(&mut self) {
        if let Some(issue) = self.selected_issue() {
            let num = issue.number;
            if self.reopening_issues.contains(&num) {
                return;
            }
            self.reopening_issues.insert(num);
            self.status_log = format!("Reopening issue #{}...", num);
            let _ = self.req_tx.send(AppRequest::Reopen {
                repo: self.repo.clone(),
                number: num,
            });
        }
    }

    fn trigger_web(&self) {
        if let Some(issue) = self.selected_issue() {
            let num = issue.number;
            let repo = &self.repo;
            let _ = std::process::Command::new("gh")
                .args(&["issue", "view", &num.to_string(), "--repo", repo, "--web"])
                .spawn();
        }
    }
}

// Background thread worker loop
fn run_backend_worker(req_rx: Receiver<AppRequest>, event_tx: Sender<Event>) {
    loop {
        if let Ok(req) = req_rx.recv() {
            match req {
                AppRequest::Fetch { repo, query } => {
                    let query = query.trim();
                    let is_number_query = (query.starts_with('#') && query[1..].chars().all(|c| c.is_digit(10)) && !query[1..].is_empty())
                        || (query.chars().all(|c| c.is_digit(10)) && !query.is_empty());
                    
                    if is_number_query {
                        let number_str = if query.starts_with('#') { &query[1..] } else { query };
                        let output = std::process::Command::new("gh")
                            .args(&[
                                "issue",
                                "view",
                                number_str,
                                "--repo",
                                &repo,
                                "--json",
                                "number,title,state,author,labels,updatedAt,body",
                            ])
                            .output();
                        match output {
                            Ok(out) => {
                                if out.status.success() {
                                    match serde_json::from_slice::<Issue>(&out.stdout) {
                                        Ok(issue) => {
                                            let _ = event_tx.send(Event::Backend(AppResponse::FetchSuccess(vec![issue])));
                                        }
                                        Err(e) => {
                                            let _ = event_tx.send(Event::Backend(AppResponse::FetchError(format!("Parse error: {}", e))));
                                        }
                                    }
                                } else {
                                    // If not found, return empty list
                                    let _ = event_tx.send(Event::Backend(AppResponse::FetchSuccess(Vec::new())));
                                }
                            }
                            Err(e) => {
                                let _ = event_tx.send(Event::Backend(AppResponse::FetchError(e.to_string())));
                            }
                        }
                    } else {
                        let mut cmd = std::process::Command::new("gh");
                        cmd.args(&[
                            "issue",
                            "list",
                            "--repo",
                            &repo,
                            "--json",
                            "number,title,state,author,labels,updatedAt,body",
                            "--limit",
                            "1000",
                        ]);
                        if !query.is_empty() {
                            cmd.args(&["-S", query]);
                        }
                        match cmd.output() {
                            Ok(output) => {
                                if output.status.success() {
                                    match serde_json::from_slice::<Vec<Issue>>(&output.stdout) {
                                        Ok(issues) => {
                                            let _ = event_tx.send(Event::Backend(AppResponse::FetchSuccess(issues)));
                                        }
                                        Err(e) => {
                                            let _ = event_tx.send(Event::Backend(AppResponse::FetchError(format!("Parse error: {}", e))));
                                        }
                                    }
                                } else {
                                    let err_str = String::from_utf8_lossy(&output.stderr).to_string();
                                    let _ = event_tx.send(Event::Backend(AppResponse::FetchError(err_str)));
                                }
                            }
                            Err(e) => {
                                let _ = event_tx.send(Event::Backend(AppResponse::FetchError(e.to_string())));
                            }
                        }
                    }
                }
                AppRequest::Close { repo, number } => {
                    let output = std::process::Command::new("gh")
                        .args(&["issue", "close", &number.to_string(), "--repo", &repo])
                        .output();
                    match output {
                        Ok(out) => {
                            if out.status.success() {
                                let _ = event_tx.send(Event::Backend(AppResponse::CloseSuccess(number)));
                            } else {
                                let err_str = String::from_utf8_lossy(&out.stderr).to_string();
                                let _ = event_tx.send(Event::Backend(AppResponse::CloseError { number, err: err_str }));
                            }
                        }
                        Err(e) => {
                            let _ = event_tx.send(Event::Backend(AppResponse::CloseError { number, err: e.to_string() }));
                        }
                    }
                }
                AppRequest::Reopen { repo, number } => {
                    let output = std::process::Command::new("gh")
                        .args(&["issue", "reopen", &number.to_string(), "--repo", &repo])
                        .output();
                    match output {
                        Ok(out) => {
                            if out.status.success() {
                                let _ = event_tx.send(Event::Backend(AppResponse::ReopenSuccess(number)));
                            } else {
                                let err_str = String::from_utf8_lossy(&out.stderr).to_string();
                                let _ = event_tx.send(Event::Backend(AppResponse::ReopenError { number, err: err_str }));
                            }
                        }
                        Err(e) => {
                            let _ = event_tx.send(Event::Backend(AppResponse::ReopenError { number, err: e.to_string() }));
                        }
                    }
                }
            }
        }
    }
}

fn main() -> io::Result<()> {
    // Detect git repository, default to aimRPG/aRPG-client
    let repo = detect_repo().unwrap_or_else(|| String::from("aimRPG/aRPG-client"));

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Set up channels for thread communication
    let (event_tx, event_rx) = mpsc::channel::<Event>();
    let (req_tx, req_rx) = mpsc::channel::<AppRequest>();

    // Spawn Crossterm input thread
    let event_tx_clone = event_tx.clone();
    thread::spawn(move || {
        loop {
            if event::poll(Duration::from_millis(100)).unwrap() {
                match event::read().unwrap() {
                    event::Event::Key(key) => {
                        if key.kind == event::KeyEventKind::Press {
                            let _ = event_tx_clone.send(Event::Input(key));
                        }
                    }
                    event::Event::Mouse(mouse) => {
                        match mouse.kind {
                            MouseEventKind::Down(MouseButton::Left) |
                            MouseEventKind::ScrollDown |
                            MouseEventKind::ScrollUp => {
                                let _ = event_tx_clone.send(Event::Mouse(mouse));
                            }
                            _ => {}
                        }
                    }
                    event::Event::Resize(_, _) => {
                        let _ = event_tx_clone.send(Event::Resize);
                    }
                    _ => {}
                }
            }
        }
    });

    // Spawn backend worker thread
    let event_tx_backend = event_tx.clone();
    thread::spawn(move || {
        run_backend_worker(req_rx, event_tx_backend);
    });

    // Initialize App state
    let mut app = App::new(repo, req_tx);
    app.trigger_fetch();

    // Layout areas tracking for mouse clicks
    let mut search_rect = Rect::default();
    let mut issues_list_rect = Rect::default();
    let mut details_rect = Rect::default();

    loop {
        // Draw TUI
        terminal.draw(|f| {
            let size = f.area();

            // Vertical Layout: Title (1), Search (3), Main Body (Min 10), Status Log (1), Help (1)
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // Title
                    Constraint::Length(3), // Search bar
                    Constraint::Min(10),   // Main columns
                    Constraint::Length(1), // Status Log
                    Constraint::Length(1), // Help Bar
                ])
                .split(size);

            // Title Bar
            let state_info = if app.is_loading { " [Loading...]" } else { "" };
            let title = Line::from(vec![
                Span::styled(" GitHub Issues Dashboard ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {} ", app.repo), Style::default().fg(Color::DarkGray)),
                Span::styled(format!(" ({} issues found){}", app.issues.len(), state_info), Style::default().fg(Color::Gray)),
            ]);
            f.render_widget(Paragraph::new(title), chunks[0]);

            // Search Bar
            search_rect = chunks[1];
            let search_border_style = if app.search_focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let search_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(search_border_style)
                .title(Span::styled(" Search Query (Press '/' to type, Esc to exit) ", Style::default().fg(Color::Gray)));
            
            let search_text = if app.search_query.is_empty() && !app.search_focused {
                Span::styled("Type search criteria... (e.g. is:closed verdict)", Style::default().fg(Color::DarkGray))
            } else {
                Span::styled(&app.search_query, Style::default().fg(Color::White))
            };
            f.render_widget(Paragraph::new(search_text).block(search_block), chunks[1]);

            // Main View Area: Split dynamically based on aspect ratio
            let vertical_split = size.height > size.width;
            
            let main_chunks = if vertical_split {
                Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Percentage(40), // Issues List (Top)
                        Constraint::Percentage(60), // Issue Details (Bottom)
                    ])
                    .split(chunks[2])
            } else {
                Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(40), // Issues List (Left)
                        Constraint::Percentage(60), // Issue Details (Right)
                    ])
                    .split(chunks[2])
            };

            // Issues List Column
            issues_list_rect = main_chunks[0];
            details_rect = main_chunks[1];
            let list_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(" Issues ", Style::default().fg(Color::Gray)));

            let list_width = main_chunks[0].width as usize - 2;

            let items: Vec<ListItem> = app.issues.iter().map(|issue| {
                let state_color = if issue.state == "OPEN" { Color::Green } else { Color::Magenta };
                let state_str = if issue.state == "OPEN" { "" } else { "" };

                // 1. Build and truncate title line
                let prefix = format!(" {} #{} ", state_str, issue.number);
                let mut suffix = String::new();
                if app.closing_issues.contains(&issue.number) {
                    suffix.push_str(" [closing...]");
                } else if app.reopening_issues.contains(&issue.number) {
                    suffix.push_str(" [reopening...]");
                }
                
                let prefix_width = prefix.width();
                let suffix_width = suffix.width();
                let max_title_width = list_width.saturating_sub(prefix_width + suffix_width);
                let truncated_title = truncate_str_by_width(&issue.title, max_title_width);

                let mut title_spans = vec![
                    Span::styled(prefix, Style::default().fg(state_color).add_modifier(Modifier::BOLD)),
                    Span::styled(truncated_title, Style::default().fg(Color::White)),
                ];
                if !suffix.is_empty() {
                    title_spans.push(Span::styled(suffix, Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC)));
                }
                let title_line = Line::from(title_spans);

                // 2. Build and truncate metadata line
                let meta_prefix = format!("    @{} updated {} ", issue.author.login, issue.updated_at.split('T').next().unwrap_or(""));
                let prefix_width = meta_prefix.width();
                
                let mut meta_spans = vec![
                    Span::styled(meta_prefix, Style::default().fg(Color::DarkGray)),
                ];
                
                let mut current_width = prefix_width;
                for label in &issue.labels {
                    let label_str = format!(" {} ", label.name);
                    let label_width = label_str.width() + 1; // including trailing space
                    if current_width + label_width > list_width {
                        if current_width + 1 <= list_width {
                            meta_spans.push(Span::styled("…", Style::default().fg(Color::DarkGray)));
                        }
                        break;
                    }
                    let label_bg = label.color.as_ref().map(|c| parse_hex_color(c)).unwrap_or(Color::DarkGray);
                    let label_fg = get_text_color_for_bg(label_bg);
                    meta_spans.push(Span::styled(label_str, Style::default().bg(label_bg).fg(label_fg)));
                    meta_spans.push(Span::styled(" ", Style::default()));
                    current_width += label_width;
                }
                let meta_line = Line::from(meta_spans);

                ListItem::new(vec![
                    title_line,
                    meta_line,
                    Line::from(""), // separator line
                ])
            }).collect();

            let highlight_style = Style::default().bg(Color::Rgb(30, 30, 46)).add_modifier(Modifier::BOLD);
            let list = List::new(items)
                .block(list_block)
                .highlight_style(highlight_style);

            f.render_stateful_widget(list, main_chunks[0], &mut app.list_state);

            // Issue Details Column
            let details_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(6), // Header details
                    Constraint::Min(3),    // Body details
                ])
                .split(main_chunks[1]);

            if let Some(issue) = app.selected_issue().cloned() {
                let state_color = if issue.state == "OPEN" { Color::Green } else { Color::Magenta };
                
                let mut detail_lines = vec![
                    Line::from(vec![
                        Span::styled(format!("Issue #{} ({})", issue.number, issue.state), Style::default().fg(state_color).add_modifier(Modifier::BOLD)),
                    ]),
                    Line::from(vec![
                        Span::styled(&issue.title, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    ]),
                    Line::from(vec![
                        Span::styled("Author: ", Style::default().fg(Color::Gray)),
                        Span::styled(format!("@{}", issue.author.login), Style::default().fg(Color::White)),
                        Span::styled("  |  Updated: ", Style::default().fg(Color::Gray)),
                        Span::styled(&issue.updated_at, Style::default().fg(Color::White)),
                    ]),
                    Line::from(""),
                ];

                // Labels line
                let mut label_spans = vec![Span::styled("Labels: ", Style::default().fg(Color::Gray))];
                if issue.labels.is_empty() {
                    label_spans.push(Span::styled("none", Style::default().fg(Color::DarkGray)));
                } else {
                    for label in &issue.labels {
                        let label_bg = label.color.as_ref().map(|c| parse_hex_color(c)).unwrap_or(Color::DarkGray);
                        let label_fg = get_text_color_for_bg(label_bg);
                        label_spans.push(Span::styled(format!(" {} ", label.name), Style::default().bg(label_bg).fg(label_fg)));
                        label_spans.push(Span::styled("  ", Style::default()));
                    }
                }
                detail_lines.push(Line::from(label_spans));

                let header_block = Block::default()
                    .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(Span::styled(" Issue Details ", Style::default().fg(Color::Gray)));

                let header_text = Paragraph::new(detail_lines)
                    .block(header_block);
                f.render_widget(header_text, details_layout[0]);

                let body_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::DarkGray));

                let cleaned_body = clean_github_markdown(&issue.body);
                let core_markdown = tui_markdown::from_str(&cleaned_body);
                let markdown_text = convert_text(core_markdown);

                // Clamp scrolling
                let total_lines = markdown_text.lines.len();
                let display_height = details_layout[1].height.saturating_sub(2) as usize;
                if app.details_scroll + display_height > total_lines && total_lines > display_height {
                    app.details_scroll = total_lines - display_height;
                }

                let body_paragraph = Paragraph::new(markdown_text)
                    .block(body_block)
                    .wrap(Wrap { trim: false })
                    .scroll((app.details_scroll as u16, 0));
                f.render_widget(body_paragraph, details_layout[1]);
            } else {
                let details_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(Span::styled(" Issue Details ", Style::default().fg(Color::Gray)));
                let help_text = vec![
                    Line::from(""),
                    Line::from("Select an issue on the left panel to read details."),
                    Line::from(""),
                    Line::from("Keyboard keys:"),
                    Line::from("  j / k       : Navigate issues list"),
                    Line::from("  c           : Close selected issue"),
                    Line::from("  r           : Reopen selected issue"),
                    Line::from("  R / F5      : Reload issues list"),
                    Line::from("  e           : Open issue in browser"),
                    Line::from("  /           : Focus Search Query input field"),
                    Line::from("  Esc         : Exit search input / Clear filter"),
                    Line::from("  Shift-j/k   : Scroll description pane"),
                    Line::from("  q           : Quit application"),
                ];
                f.render_widget(Paragraph::new(help_text).block(details_block).alignment(ratatui::layout::Alignment::Center), main_chunks[1]);
            }

            // Status Log Bar
            let log_span = Span::styled(format!(" Log: {}", app.status_log), Style::default().fg(Color::Gray));
            f.render_widget(Paragraph::new(log_span), chunks[3]);

            // Help instruction bar
            let help_style = Style::default().bg(Color::Rgb(30, 30, 46)).fg(Color::White);
            let help_spans = Line::from(vec![
                Span::styled(" j/k: ", Style::default().fg(Color::Cyan)), Span::styled("Nav ", help_style),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled(" Shift-j/k: ", Style::default().fg(Color::Cyan)), Span::styled("Scroll details ", help_style),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled(" c: ", Style::default().fg(Color::Red)), Span::styled("Close issue ", help_style),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled(" r: ", Style::default().fg(Color::Green)), Span::styled("Reopen ", help_style),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled(" e: ", Style::default().fg(Color::Yellow)), Span::styled("Open Web ", help_style),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled(" /: ", Style::default().fg(Color::Cyan)), Span::styled("Search ", help_style),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled(" Esc: ", Style::default().fg(Color::Gray)), Span::styled("Reset ", help_style),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled(" R/F5: ", Style::default().fg(Color::Cyan)), Span::styled("Reload ", help_style),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled(" q: ", Style::default().fg(Color::Red)), Span::styled("Quit ", help_style),
            ]);
            f.render_widget(Paragraph::new(help_spans).alignment(ratatui::layout::Alignment::Center), chunks[4]);
        })?;

        // Read event
        match event_rx.recv().unwrap() {
            Event::Input(key) => {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    break;
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) {
                    continue;
                }
                if app.search_focused {
                    match key.code {
                        KeyCode::Enter => {
                            app.search_focused = false;
                            app.trigger_fetch();
                        }
                        KeyCode::Esc => {
                            app.search_focused = false;
                            app.search_query.clear();
                            app.trigger_fetch();
                        }
                        KeyCode::Backspace => {
                            app.search_query.pop();
                        }
                        KeyCode::Char(c) => {
                            app.search_query.push(c);
                        }
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('/') => {
                            app.search_focused = true;
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            app.select_next();
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            app.select_prev();
                        }
                        KeyCode::Char('J') => {
                            app.details_scroll = app.details_scroll.saturating_add(2);
                        }
                        KeyCode::Char('K') => {
                            app.details_scroll = app.details_scroll.saturating_sub(2);
                        }
                        KeyCode::PageDown => {
                            app.details_scroll = app.details_scroll.saturating_add(10);
                        }
                        KeyCode::PageUp => {
                            app.details_scroll = app.details_scroll.saturating_sub(10);
                        }
                        KeyCode::Char('c') | KeyCode::Char('x') => {
                            app.trigger_close();
                        }
                        KeyCode::Char('r') | KeyCode::Char('o') => {
                            app.trigger_reopen();
                        }
                        KeyCode::Char('R') | KeyCode::F(5) => {
                            app.trigger_fetch();
                        }
                        KeyCode::Char('e') => {
                            app.trigger_web();
                        }
                        KeyCode::Esc => {
                            if !app.search_query.is_empty() {
                                app.search_query.clear();
                                app.trigger_fetch();
                            }
                        }
                        _ => {}
                    }
                }
            }
            Event::Mouse(mouse) => {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    // Check if search bar was clicked
                    if mouse.row == search_rect.y || mouse.row == search_rect.y + 1 || mouse.row == search_rect.y + 2 {
                        if mouse.column >= search_rect.x && mouse.column < search_rect.x + search_rect.width {
                            app.search_focused = true;
                            app.status_log = String::from("Search input focused. Press Enter to search, Esc to cancel.");
                        }
                    } else if mouse.row >= issues_list_rect.y && mouse.row < issues_list_rect.y + issues_list_rect.height {
                        // Check if issues list was clicked
                        if mouse.column >= issues_list_rect.x && mouse.column < issues_list_rect.x + issues_list_rect.width {
                            app.search_focused = false;
                            // Calculate which item was clicked
                            let relative_y = (mouse.row - issues_list_rect.y) as usize;
                            if relative_y > 0 && relative_y < issues_list_rect.height as usize - 1 {
                                // Every list item spans 3 lines (title, metadata, empty separator)
                                let clicked_item_idx = app.list_state.offset() + (relative_y - 1) / 3;
                                if clicked_item_idx < app.issues.len() {
                                    app.list_state.select(Some(clicked_item_idx));
                                    app.details_scroll = 0;
                                }
                            }
                        }
                    }
                } else if mouse.kind == MouseEventKind::ScrollDown {
                    // Scroll issue details or list down based on hover bounds
                    if mouse.row >= details_rect.y && mouse.row < details_rect.y + details_rect.height
                        && mouse.column >= details_rect.x && mouse.column < details_rect.x + details_rect.width
                    {
                        app.details_scroll = app.details_scroll.saturating_add(2);
                    } else {
                        app.select_next();
                    }
                } else if mouse.kind == MouseEventKind::ScrollUp {
                    // Scroll issue details or list up based on hover bounds
                    if mouse.row >= details_rect.y && mouse.row < details_rect.y + details_rect.height
                        && mouse.column >= details_rect.x && mouse.column < details_rect.x + details_rect.width
                    {
                        app.details_scroll = app.details_scroll.saturating_sub(2);
                    } else {
                        app.select_prev();
                    }
                }
            }

            Event::Resize => {
                let _ = terminal.clear();
            }

            Event::Backend(resp) => {
                app.is_loading = false;
                match resp {
                    AppResponse::FetchSuccess(issues) => {
                        app.issues = issues;
                        if app.list_state.selected().is_none() && !app.issues.is_empty() {
                            app.list_state.select(Some(0));
                        } else if let Some(sel) = app.list_state.selected() {
                            if sel >= app.issues.len() {
                                if app.issues.is_empty() {
                                    app.list_state.select(None);
                                } else {
                                    app.list_state.select(Some(app.issues.len() - 1));
                                }
                            }
                        }
                        app.status_log = format!("Loaded {} issues successfully.", app.issues.len());
                    }
                    AppResponse::FetchError(e) => {
                        app.status_log = format!("Failed to fetch issues: {}", e);
                    }
                    AppResponse::CloseSuccess(num) => {
                        app.closing_issues.remove(&num);
                        app.status_log = format!("Issue #{} closed successfully.", num);
                        // Update state in local list
                        if let Some(pos) = app.issues.iter().position(|issue| issue.number == num) {
                            app.issues[pos].state = String::from("CLOSED");
                        }
                    }
                    AppResponse::CloseError { number, err } => {
                        app.closing_issues.remove(&number);
                        app.status_log = format!("Failed to close #{}: {}", number, err);
                    }
                    AppResponse::ReopenSuccess(num) => {
                        app.reopening_issues.remove(&num);
                        app.status_log = format!("Issue #{} reopened successfully.", num);
                        // Update state in local list
                        if let Some(pos) = app.issues.iter().position(|issue| issue.number == num) {
                            app.issues[pos].state = String::from("OPEN");
                        }
                    }
                    AppResponse::ReopenError { number, err } => {
                        app.reopening_issues.remove(&number);
                        app.status_log = format!("Failed to reopen #{}: {}", number, err);
                    }
                }
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}
