use std::env;
use std::fs;
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
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame, Terminal};
use recode_core::{
    ConfigLoader, ExecutorBridge, ExecutorOptions, PartialConfig, RunRecord, SessionRecord,
    SessionStore, StepRecord, WorkflowEngine,
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
    #[arg(long)]
    no_bootstrap: bool,
}

struct App {
    engine: WorkflowEngine,
    store: SessionStore,
    sessions: Vec<SessionRecord>,
    selected: usize,
    selected_task: usize,
    selected_step: usize,
    status: String,
}

impl App {
    fn new(store: SessionStore, auto_bootstrap: bool) -> Result<Self> {
        let engine = WorkflowEngine::new(store.clone());
        let mut app = Self {
            engine,
            store,
            sessions: Vec::new(),
            selected: 0,
            selected_task: 0,
            selected_step: 0,
            status: String::from(
                "r: reconcile+refresh, ↑/↓: session, ←/→: task, u/d: step, n: run-next, b: background, A: run-all, a: approve selected, x: cancel selected run, q: quit",
            ),
        };
        app.refresh(auto_bootstrap)?;
        Ok(app)
    }

    fn refresh(&mut self, auto_bootstrap: bool) -> Result<()> {
        self.sessions = self.store.list_sessions()?;
        if self.sessions.is_empty() && auto_bootstrap {
            let session = self.store.init_session("default")?;
            self.sessions = vec![session];
            self.selected = 0;
            self.selected_task = 0;
            self.selected_step = 0;
            self.status = String::from("Bootstrapped default session");
            return Ok(());
        }

        if self.sessions.is_empty() {
            self.selected = 0;
            self.selected_task = 0;
            self.selected_step = 0;
            self.status = String::from(
                "No sessions found. Re-run without --no-bootstrap to auto-create default session.",
            );
        } else {
            if self.selected >= self.sessions.len() {
                self.selected = self.sessions.len() - 1;
            }
            self.clamp_task_step_selection();
            self.status = format!("Loaded {} sessions", self.sessions.len());
        }
        Ok(())
    }

    fn next(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = (self.selected + 1) % self.sessions.len();
            self.selected_task = 0;
            self.selected_step = 0;
            self.clamp_task_step_selection();
        }
    }

    fn previous(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = if self.selected == 0 {
                self.sessions.len() - 1
            } else {
                self.selected - 1
            };
            self.selected_task = 0;
            self.selected_step = 0;
            self.clamp_task_step_selection();
        }
    }

    fn next_task(&mut self) {
        if let Some(session) = self.selected_session()
            && !session.tasks.is_empty()
        {
            self.selected_task = (self.selected_task + 1) % session.tasks.len();
            self.selected_step = 0;
            self.clamp_task_step_selection();
        }
    }

    fn previous_task(&mut self) {
        if let Some(session) = self.selected_session()
            && !session.tasks.is_empty()
        {
            self.selected_task = if self.selected_task == 0 {
                session.tasks.len() - 1
            } else {
                self.selected_task - 1
            };
            self.selected_step = 0;
            self.clamp_task_step_selection();
        }
    }

    fn next_step(&mut self) {
        if let Some(task) = self.selected_task_record()
            && !task.steps.is_empty()
        {
            self.selected_step = (self.selected_step + 1) % task.steps.len();
        }
    }

    fn previous_step(&mut self) {
        if let Some(task) = self.selected_task_record()
            && !task.steps.is_empty()
        {
            self.selected_step = if self.selected_step == 0 {
                task.steps.len() - 1
            } else {
                self.selected_step - 1
            };
        }
    }

    fn clamp_task_step_selection(&mut self) {
        let task_len = self.selected_session().map(|s| s.tasks.len()).unwrap_or(0);
        if task_len == 0 {
            self.selected_task = 0;
            self.selected_step = 0;
            return;
        }
        if self.selected_task >= task_len {
            self.selected_task = task_len - 1;
        }
        let step_len = self
            .selected_task_record()
            .map(|task| task.steps.len())
            .unwrap_or(0);
        if step_len == 0 {
            self.selected_step = 0;
        } else if self.selected_step >= step_len {
            self.selected_step = step_len - 1;
        }
    }

    fn selected_session(&self) -> Option<&SessionRecord> {
        self.sessions.get(self.selected)
    }

    fn selected_task_record(&self) -> Option<&recode_core::TaskRecord> {
        self.selected_session()?.tasks.get(self.selected_task)
    }

    fn selected_step_record(&self) -> Option<&recode_core::StepRecord> {
        self.selected_task_record()?.steps.get(self.selected_step)
    }

    fn selected_run_record(&self) -> Option<RunRecord> {
        let attempt = self.selected_step_record()?.attempts.last()?;
        load_attempt_run(&self.store, attempt.run_id)
    }

    fn run_next(&mut self) -> Result<()> {
        self.run_next_with_options(ExecutorOptions::default())
    }

    fn run_next_background(&mut self) -> Result<()> {
        self.run_next_with_options(ExecutorOptions {
            background: true,
            ..ExecutorOptions::default()
        })
    }

    fn run_next_with_options(&mut self, options: ExecutorOptions) -> Result<()> {
        let session_id = self
            .selected_session()
            .map(|session| session.id)
            .ok_or_else(|| anyhow!("no session selected"))?;
        let mut runner = ExecutorBridge::with_options(options);
        let result = self.engine.run_next_step(session_id, &mut runner)?;
        self.status = format!(
            "run-next: {} {} run={}",
            result.step_title,
            serde_json::to_string(&result.disposition)?,
            result
                .run_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".into())
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
        let session = self
            .selected_session()
            .ok_or_else(|| anyhow!("no session selected"))?;
        let task = session
            .tasks
            .get(self.selected_task)
            .ok_or_else(|| anyhow!("no task selected"))?;
        let step = task
            .steps
            .get(self.selected_step)
            .ok_or_else(|| anyhow!("no step selected"))?;

        if step.status != recode_core::StepStatus::WaitingApproval {
            return Err(anyhow!("selected step is not waiting for approval"));
        }

        self.engine.approve_step(session.id, task.id, step.id)?;
        self.status = format!("approved: {}", step.title);
        self.refresh_preserving_message()
    }

    fn cancel_selected_run(&mut self) -> Result<()> {
        let run = self
            .selected_run_record()
            .ok_or_else(|| anyhow!("no run selected"))?;

        if run.status != recode_core::RunStatus::Running {
            return Err(anyhow!("selected run is not running"));
        }

        self.engine.cancel_run(run.id)?;
        self.status = format!("cancel requested: run={} (press r to reconcile)", run.id);
        self.refresh_preserving_message()
    }

    fn refresh_preserving_message(&mut self) -> Result<()> {
        self.reconcile_running_runs()?;
        let message = self.status.clone();
        self.sessions = self.store.list_sessions()?;
        if self.sessions.is_empty() {
            self.selected = 0;
            self.selected_task = 0;
            self.selected_step = 0;
        } else if self.selected >= self.sessions.len() {
            self.selected = self.sessions.len() - 1;
        }
        self.clamp_task_step_selection();
        self.status = message;
        Ok(())
    }

    fn reconcile_running_runs(&self) -> Result<usize> {
        let runs = self.store.list_runs()?;
        let mut reconciled = 0;
        for run in runs {
            if run.status == recode_core::RunStatus::Running {
                let updated = self.engine.reconcile_run(run.id)?;
                if updated.status != recode_core::RunStatus::Running {
                    reconciled += 1;
                }
            }
        }
        Ok(reconciled)
    }

    fn reconcile_and_refresh(&mut self, auto_bootstrap: bool) -> Result<()> {
        let reconciled = self.reconcile_running_runs()?;
        self.refresh(auto_bootstrap)?;
        if reconciled > 0 {
            self.status = format!("Reconciled {reconciled} finished run(s)");
        }
        Ok(())
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = env::current_dir()?;
    let config_loader = ConfigLoader::new(cwd);
    let config = config_loader.load(cli.config.clone(), cli_partial(&cli))?;
    let store = SessionStore::new(config.state_dir);

    if cli.dump {
        let app = App::new(store, !cli.no_bootstrap)?;
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

    let result = run_app(&mut terminal, App::new(store, !cli.no_bootstrap));

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
                KeyCode::Right | KeyCode::Char('l') => app.next_task(),
                KeyCode::Left | KeyCode::Char('h') => app.previous_task(),
                KeyCode::Char('d') => app.next_step(),
                KeyCode::Char('u') => app.previous_step(),
                KeyCode::Char('r') => {
                    if let Err(error) = app.reconcile_and_refresh(false) {
                        app.status = format!("refresh failed: {error}");
                    }
                }
                KeyCode::Char('n') => {
                    if let Err(error) = app.run_next() {
                        app.status = format!("run-next failed: {error}");
                    }
                }
                KeyCode::Char('b') => {
                    if let Err(error) = app.run_next_background() {
                        app.status = format!("background run failed: {error}");
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
                KeyCode::Char('x') => {
                    if let Err(error) = app.cancel_selected_run() {
                        app.status = format!("cancel failed: {error}");
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
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(frame.area());

    let title = Paragraph::new("Recode TUI, session/task parity view")
        .block(Block::default().borders(Borders::ALL).title("Overview"));
    frame.render_widget(title, vertical[0]);

    let banner = Paragraph::new(status_banner_lines(app))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Selected Status"),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(banner, vertical[1]);

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(32), Constraint::Percentage(68)])
        .split(vertical[2]);

    draw_sessions(frame, app, horizontal[0]);
    draw_details(frame, app, horizontal[1]);

    let footer = Paragraph::new(app.status.as_str())
        .style(Style::default().fg(Color::Yellow))
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    frame.render_widget(footer, vertical[3]);
}

fn draw_sessions(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let items: Vec<ListItem> = if app.sessions.is_empty() {
        vec![ListItem::new("No sessions")]
    } else {
        app.sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                let prefix = if index == app.selected { "▶" } else { " " };
                let line = Line::from(vec![
                    Span::raw(format!("{prefix} ")),
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

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title("Sessions"));

    frame.render_widget(list, area);
}

fn draw_details(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    let text = if let Some(session) = app.selected_session() {
        session_detail_lines(&app.store, session, app.selected_task, app.selected_step)
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
    frame.render_widget(paragraph, split[0]);

    let log_text = selected_log_lines(app);
    let log_paragraph = Paragraph::new(log_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Selected Step Log Tail"),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(log_paragraph, split[1]);
}

fn session_detail_lines(
    store: &SessionStore,
    session: &SessionRecord,
    selected_task: usize,
    selected_step: usize,
) -> Vec<Line<'static>> {
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
        Line::from(vec![
            Span::styled("Cursor: ", label_style()),
            Span::raw(format!(
                "task={} step={}",
                selected_task + 1,
                selected_step + 1
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

    for (task_index, task) in session.tasks.iter().enumerate() {
        let task_marker = if task_index == selected_task {
            ">"
        } else {
            " "
        };
        lines.push(Line::from(vec![
            Span::styled(task_marker, Style::default().fg(Color::Yellow)),
            Span::raw(" "),
            Span::styled("• ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{} [{:?}]", task.title, task.status)),
        ]));
        for (step_index, step) in task.steps.iter().enumerate() {
            lines.push(step_line(
                step,
                task_index == selected_task && step_index == selected_step,
            ));
            if let Some(last) = step.attempts.last() {
                lines.push(Line::from(format!(
                    "      attempt #{} {:?} {}",
                    last.number,
                    last.status,
                    last.summary.clone().unwrap_or_default()
                )));
                if let Some(run) = load_attempt_run(store, last.run_id) {
                    lines.extend(run_detail_lines(&run));
                }
            }
        }
        lines.push(Line::from(""));
    }

    lines
}

fn step_line(step: &StepRecord, selected: bool) -> Line<'static> {
    let approval = if step.requires_approval {
        if step.approval_granted {
            " approval=granted"
        } else {
            " approval=required"
        }
    } else {
        ""
    };
    let marker = if selected { ">" } else { " " };
    Line::from(format!(
        "   {marker} {} [{:?}] attempts={}{}",
        step.title,
        step.status,
        step.attempts.len(),
        approval
    ))
}

fn load_attempt_run(store: &SessionStore, run_id: Option<Uuid>) -> Option<RunRecord> {
    run_id.and_then(|id| store.load_run(id).ok())
}

fn run_detail_lines(run: &RunRecord) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(format!(
        "      run {} {:?} mode={:?} pid={}",
        run.id,
        run.status,
        run.mode,
        run.pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| "-".into())
    ))];

    if let Some(path) = &run.stdout_log_path {
        lines.push(Line::from(format!("        stdout {}", path)));
    }
    if let Some(path) = &run.stderr_log_path {
        lines.push(Line::from(format!("        stderr {}", path)));
    }
    if let Some(summary) = &run.summary {
        lines.push(Line::from(format!("        summary {}", summary)));
    }

    lines
}

fn selected_log_lines(app: &App) -> Vec<Line<'static>> {
    let Some(step) = app.selected_step_record() else {
        return vec![Line::from("No step selected")];
    };
    let Some(last_attempt) = step.attempts.last() else {
        return vec![Line::from("No attempts for selected step")];
    };
    let Some(run) = load_attempt_run(&app.store, last_attempt.run_id) else {
        return vec![Line::from("No run metadata for selected step")];
    };

    let mut lines = vec![Line::from(format!(
        "run={} status={:?} mode={:?}",
        run.id, run.status, run.mode
    ))];

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "stdout",
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    )));
    lines.extend(tail_file_lines(run.stdout_log_path.as_deref(), 8));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "stderr",
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    )));
    lines.extend(tail_file_lines(run.stderr_log_path.as_deref(), 8));

    lines
}

fn status_banner_lines(app: &App) -> Vec<Line<'static>> {
    let Some(session) = app.selected_session() else {
        return vec![Line::from("No session selected")];
    };

    let task = session.tasks.get(app.selected_task);
    let step = task.and_then(|task| task.steps.get(app.selected_step));
    let run = step
        .and_then(|step| step.attempts.last())
        .and_then(|attempt| load_attempt_run(&app.store, attempt.run_id));

    let session_style = status_style_for_label(session.status_label());
    let task_text = task
        .map(|task| format!("{} [{:?}]", task.title, task.status))
        .unwrap_or_else(|| "-".into());
    let step_text = step
        .map(|step| format!("{} [{:?}]", step.title, step.status))
        .unwrap_or_else(|| "-".into());
    let run_text = run
        .as_ref()
        .map(|run| {
            format!(
                "{:?} pid={} {}",
                run.status,
                run.pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "-".into()),
                run.summary.clone().unwrap_or_default()
            )
        })
        .unwrap_or_else(|| "-".into());

    vec![
        Line::from(vec![
            Span::styled("session ", label_style()),
            Span::styled(session.status_label().to_string(), session_style),
            Span::raw("  "),
            Span::styled("task ", label_style()),
            Span::raw(task_text),
        ]),
        Line::from(vec![
            Span::styled("step ", label_style()),
            Span::raw(step_text),
            Span::raw("  "),
            Span::styled("run ", label_style()),
            Span::raw(run_text),
        ]),
    ]
}

fn tail_file_lines(path: Option<&str>, max_lines: usize) -> Vec<Line<'static>> {
    let Some(path) = path else {
        return vec![Line::from("  -")];
    };

    match fs::read_to_string(path) {
        Ok(content) => {
            let collected: Vec<String> = content
                .lines()
                .rev()
                .take(max_lines)
                .map(|line| format!("  {line}"))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            if collected.is_empty() {
                vec![Line::from("  <empty>")]
            } else {
                collected.into_iter().map(Line::from).collect()
            }
        }
        Err(error) => vec![Line::from(format!("  <unreadable: {error}>"))],
    }
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
        "cancelled" | "cancel" => Color::LightRed,
        "waitingapproval" | "waiting_approval" => Color::Magenta,
        _ => Color::Cyan,
    }
}

fn status_style_for_label(status: &str) -> Style {
    Style::default()
        .fg(status_color(status))
        .add_modifier(Modifier::BOLD)
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
            "cursor session={} task={} step={}",
            app.selected + 1,
            app.selected_task + 1,
            app.selected_step + 1
        ));
        lines.push(format!(
            "policy retry={} timeout={}s approval={:?}",
            session.policy.retry.max_attempts,
            session.policy.timeout.step_timeout_secs,
            session.policy.approval
        ));
        for (task_index, task) in session.tasks.iter().enumerate() {
            lines.push(format!(
                "task {} {:?} selected={}",
                task.title,
                task.status,
                task_index == app.selected_task
            ));
            for (step_index, step) in task.steps.iter().enumerate() {
                lines.push(format!(
                    "  step {} {:?} attempts={} requires_approval={} granted={} selected={}",
                    step.title,
                    step.status,
                    step.attempts.len(),
                    step.requires_approval,
                    step.approval_granted,
                    task_index == app.selected_task && step_index == app.selected_step
                ));
                if let Some(last) = step.attempts.last() {
                    lines.push(format!(
                        "    attempt #{} {:?} run_id={} summary={}",
                        last.number,
                        last.status,
                        last.run_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "-".into()),
                        last.summary.clone().unwrap_or_default()
                    ));
                    if let Some(run) = load_attempt_run(&app.store, last.run_id) {
                        lines.push(format!(
                            "    run {} {:?} mode={:?} pid={}",
                            run.id,
                            run.status,
                            run.mode,
                            run.pid
                                .map(|pid| pid.to_string())
                                .unwrap_or_else(|| "-".into())
                        ));
                        lines.push(format!(
                            "      stdout={} stderr={} exit_code={}",
                            run.stdout_log_path.unwrap_or_else(|| "-".into()),
                            run.stderr_log_path.unwrap_or_else(|| "-".into()),
                            run.exit_code_path.unwrap_or_else(|| "-".into())
                        ));
                    }
                }
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
