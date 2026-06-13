use std::env;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame, Terminal};
use recode_core::{
    ConfigLoader, ExecutorBridge, ExecutorOptions, PartialConfig, SessionRecord, SessionStore,
    StepRecord, WorkflowEngine,
};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(name = "recode-tui", version, about = "Recode operator TUI")]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    log_level: Option<String>,
    #[arg(long, global = true)]
    default_provider: Option<String>,
    #[arg(long, global = true)]
    default_timeout_secs: Option<u64>,
    #[arg(long, global = true)]
    default_max_attempts: Option<u32>,
    #[arg(long, global = true)]
    approval_policy: Option<String>,
    #[arg(long)]
    dump: bool,
}

struct App {
    engine: WorkflowEngine,
    store: SessionStore,
    sessions: Vec<SessionRecord>,
    selected: usize,
    status: String,
}

impl App {
    fn new(store: SessionStore) -> Result<Self> {
        let engine = WorkflowEngine::new(store.clone());
        let mut app = Self {
            engine,
            store,
            sessions: Vec::new(),
            selected: 0,
            status: String::from(
                "r: refresh, ↑/↓: move, n: run-next, A: run-all, a: approve, q: quit (cmd:/shell:/exec: shared shell bridge; CLI adds --stream/--pty/--cancel-file)",
            ),
        };
        app.refresh()?;
        Ok(app)
    }

    fn refresh(&mut self) -> Result<()> {
        self.sessions = self.store.list_sessions()?;
        if self.sessions.is_empty() {
            self.selected = 0;
            self.status =
                String::from("No sessions found. Create one with recode-cli session init.");
        } else if self.selected >= self.sessions.len() {
            self.selected = self.sessions.len() - 1;
            self.status = format!("Loaded {} sessions", self.sessions.len());
        } else {
            self.status = format!("Loaded {} sessions", self.sessions.len());
        }
        Ok(())
    }

    fn next(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = (self.selected + 1) % self.sessions.len();
        }
    }

    fn previous(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = if self.selected == 0 {
                self.sessions.len() - 1
            } else {
                self.selected - 1
            };
        }
    }

    fn selected_session(&self) -> Option<&SessionRecord> {
        self.sessions.get(self.selected)
    }

    fn run_next(&mut self) -> Result<()> {
        let session_id = self
            .selected_session()
            .map(|session| session.id)
            .ok_or_else(|| anyhow!("no session selected"))?;
        let mut runner = ExecutorBridge::with_options(ExecutorOptions::default());
        let result = self.engine.run_next_step(session_id, &mut runner)?;
        self.status = format!(
            "run-next: {} {}",
            result.step_title,
            serde_json::to_string(&result.disposition)?
        );
        self.refresh_preserving_message()
    }

    fn run_all(&mut self) -> Result<()> {
        let session_id = self
            .selected_session()
            .map(|session| session.id)
            .ok_or_else(|| anyhow!("no session selected"))?;
        let mut runner = ExecutorBridge::with_options(ExecutorOptions::default());
        let result = self.engine.run_all(session_id, &mut runner)?;
        self.status = format!("run-all: {} step(s)", result.runs.len());
        self.refresh_preserving_message()
    }

    fn approve_waiting_step(&mut self) -> Result<()> {
        let (session_id, task_id, step_id, step_title) = self
            .selected_session()
            .and_then(find_first_waiting_step)
            .ok_or_else(|| anyhow!("no waiting approval step in selected session"))?;
        self.engine.approve_step(session_id, task_id, step_id)?;
        self.status = format!("approved: {step_title}");
        self.refresh_preserving_message()
    }

    fn refresh_preserving_message(&mut self) -> Result<()> {
        let message = self.status.clone();
        self.sessions = self.store.list_sessions()?;
        if self.sessions.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.sessions.len() {
            self.selected = self.sessions.len() - 1;
        }
        self.status = message;
        Ok(())
    }
}

fn find_first_waiting_step(session: &SessionRecord) -> Option<(Uuid, Uuid, Uuid, String)> {
    for task in &session.tasks {
        for step in &task.steps {
            if step.status == recode_core::StepStatus::WaitingApproval {
                return Some((session.id, task.id, step.id, step.title.clone()));
            }
        }
    }
    None
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = env::current_dir()?;
    let config_loader = ConfigLoader::new(cwd);
    let config = config_loader.load(cli.config.clone(), cli_partial(&cli))?;
    let store = SessionStore::new(config.state_dir);

    if cli.dump {
        let app = App::new(store)?;
        for line in dump_lines(&app) {
            println!("{line}");
        }
        return Ok(());
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, App::new(store));

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_app(terminal: &mut DefaultTerminal, app_result: Result<App>) -> Result<()> {
    let mut app = app_result?;
    loop {
        terminal.draw(|frame| draw(frame, &app))?;

        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
        {
            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Down | KeyCode::Char('j') => app.next(),
                KeyCode::Up | KeyCode::Char('k') => app.previous(),
                KeyCode::Char('r') => {
                    if let Err(error) = app.refresh() {
                        app.status = format!("refresh failed: {error}");
                    }
                }
                KeyCode::Char('n') => {
                    if let Err(error) = app.run_next() {
                        app.status = format!("run-next failed: {error}");
                    }
                }
                KeyCode::Char('A') => {
                    if let Err(error) = app.run_all() {
                        app.status = format!("run-all failed: {error}");
                    }
                }
                KeyCode::Char('a') => {
                    if let Err(error) = app.approve_waiting_step() {
                        app.status = format!("approve failed: {error}");
                    }
                }
                _ => {}
            }
        }
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(frame.area());

    let title = Paragraph::new("Recode TUI, session/task parity view")
        .block(Block::default().borders(Borders::ALL).title("Overview"));
    frame.render_widget(title, vertical[0]);

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(32), Constraint::Percentage(68)])
        .split(vertical[1]);

    draw_sessions(frame, app, horizontal[0]);
    draw_details(frame, app, horizontal[1]);

    let footer = Paragraph::new(app.status.as_str())
        .style(Style::default().fg(Color::Yellow))
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    frame.render_widget(footer, vertical[2]);
}

fn draw_sessions(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let items: Vec<ListItem> = if app.sessions.is_empty() {
        vec![ListItem::new("No sessions")]
    } else {
        app.sessions
            .iter()
            .map(|session| {
                let line = Line::from(vec![
                    Span::styled(
                        format!("{} ", session.status_label()),
                        Style::default()
                            .fg(status_color(session.status_label()))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(session.name.to_string()),
                ]);
                ListItem::new(line)
            })
            .collect()
    };

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Sessions"))
        .highlight_style(Style::default().bg(Color::Blue).fg(Color::Black))
        .highlight_symbol("▶ ");

    let mut state = ListState::default();
    if !app.sessions.is_empty() {
        state.select(Some(app.selected));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_details(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let text = if let Some(session) = app.selected_session() {
        session_detail_lines(session)
    } else {
        vec![Line::from("No session selected")]
    };

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Session Detail"),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn session_detail_lines(session: &SessionRecord) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Name: ", label_style()),
            Span::raw(session.name.clone()),
        ]),
        Line::from(vec![
            Span::styled("ID: ", label_style()),
            Span::raw(session.id.to_string()),
        ]),
        Line::from(vec![
            Span::styled("Status: ", label_style()),
            Span::styled(
                format!("{:?}", session.status),
                Style::default().fg(status_color(&format!("{:?}", session.status))),
            ),
        ]),
        Line::from(vec![
            Span::styled("Policy: ", label_style()),
            Span::raw(format!(
                "retry={} timeout={}s approval={:?}",
                session.policy.retry.max_attempts,
                session.policy.timeout.step_timeout_secs,
                session.policy.approval
            )),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Tasks",
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ];

    if session.tasks.is_empty() {
        lines.push(Line::from("  No tasks"));
        return lines;
    }

    for task in &session.tasks {
        lines.push(Line::from(vec![
            Span::styled("• ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{} [{:?}]", task.title, task.status)),
        ]));
        for step in &task.steps {
            lines.push(step_line(step));
            if let Some(last) = step.attempts.last() {
                lines.push(Line::from(format!(
                    "      attempt #{} {:?} {}",
                    last.number,
                    last.status,
                    last.summary.clone().unwrap_or_default()
                )));
            }
        }
        lines.push(Line::from(""));
    }

    lines
}

fn step_line(step: &StepRecord) -> Line<'static> {
    let approval = if step.requires_approval {
        if step.approval_granted {
            " approval=granted"
        } else {
            " approval=required"
        }
    } else {
        ""
    };
    Line::from(format!(
        "   - {} [{:?}] attempts={}{}",
        step.title,
        step.status,
        step.attempts.len(),
        approval
    ))
}

fn label_style() -> Style {
    Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD)
}

fn status_color(status: &str) -> Color {
    match status.to_ascii_lowercase().as_str() {
        "running" => Color::Yellow,
        "completed" => Color::Green,
        "failed" => Color::Red,
        "waitingapproval" | "waiting_approval" => Color::Magenta,
        _ => Color::Cyan,
    }
}

trait SessionStatusLabel {
    fn status_label(&self) -> &'static str;
}

impl SessionStatusLabel for SessionRecord {
    fn status_label(&self) -> &'static str {
        match self.status {
            recode_core::SessionStatus::Created => "created",
            recode_core::SessionStatus::Running => "running",
            recode_core::SessionStatus::WaitingApproval => "wait",
            recode_core::SessionStatus::Paused => "paused",
            recode_core::SessionStatus::Completed => "done",
            recode_core::SessionStatus::Failed => "failed",
            recode_core::SessionStatus::Cancelled => "cancel",
        }
    }
}

fn dump_lines(app: &App) -> Vec<String> {
    let mut lines = vec![String::from("Recode TUI dump")];
    if let Some(session) = app.selected_session() {
        lines.push(format!("session {} {:?}", session.name, session.status));
        lines.push(format!(
            "policy retry={} timeout={}s approval={:?}",
            session.policy.retry.max_attempts,
            session.policy.timeout.step_timeout_secs,
            session.policy.approval
        ));
        for task in &session.tasks {
            lines.push(format!("task {} {:?}", task.title, task.status));
            for step in &task.steps {
                lines.push(format!(
                    "  step {} {:?} attempts={} requires_approval={} granted={}",
                    step.title,
                    step.status,
                    step.attempts.len(),
                    step.requires_approval,
                    step.approval_granted
                ));
            }
        }
    } else {
        lines.push(String::from("no sessions"));
    }
    lines
}

fn cli_partial(cli: &Cli) -> PartialConfig {
    PartialConfig {
        state_dir: cli.state_dir.clone(),
        log_level: cli.log_level.clone(),
        default_provider: cli.default_provider.clone(),
        default_timeout_secs: cli.default_timeout_secs,
        default_max_attempts: cli.default_max_attempts,
        approval_policy: cli
            .approval_policy
            .as_deref()
            .and_then(recode_core::ApprovalPolicy::parse),
    }
}
