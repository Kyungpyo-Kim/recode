use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
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
    provider: recode_core::ProviderConfig,
    sessions: Vec<SessionRecord>,
    selected: usize,
    selected_task: usize,
    selected_step: usize,
    status: String,
    input_mode: InputMode,
    prompt_buffer: String,
    active_run: Option<ActiveRunState>,
    chat_focus: ChatPaneFocus,
    transcript_scroll: u16,
    composer_scroll: u16,
    context_scroll: u16,
    log_scroll: u16,
    detail_scroll: u16,
}

struct ActiveRunState {
    session_id: Uuid,
    task_id: Uuid,
    step_id: Uuid,
    receiver: Receiver<Result<recode_core::RunStepResult, String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Normal,
    EditingPrompt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatPaneFocus {
    Transcript,
    Composer,
    Context,
    Log,
}

impl ChatPaneFocus {
    fn next(self) -> Self {
        match self {
            Self::Transcript => Self::Composer,
            Self::Composer => Self::Context,
            Self::Context => Self::Log,
            Self::Log => Self::Transcript,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Transcript => Self::Log,
            Self::Composer => Self::Transcript,
            Self::Context => Self::Composer,
            Self::Log => Self::Context,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Transcript => "transcript",
            Self::Composer => "composer",
            Self::Context => "context",
            Self::Log => "log",
        }
    }
}

impl App {
    fn new(
        store: SessionStore,
        provider: recode_core::ProviderConfig,
        auto_bootstrap: bool,
    ) -> Result<Self> {
        let engine = WorkflowEngine::new(store.clone());
        let mut app = Self {
            engine,
            store,
            provider,
            sessions: Vec::new(),
            selected: 0,
            selected_task: 0,
            selected_step: 0,
            status: String::from(
                "chat-first: ↑/↓ session, ←/→ task, u/d step, Tab pane, PgUp/PgDn scroll, e edit, Enter save+run, n run-next, b background, A run-all, a approve, x cancel, r refresh, q quit",
            ),
            input_mode: InputMode::Normal,
            prompt_buffer: String::new(),
            active_run: None,
            chat_focus: ChatPaneFocus::Transcript,
            transcript_scroll: 0,
            composer_scroll: 0,
            context_scroll: 0,
            log_scroll: 0,
            detail_scroll: 0,
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
            self.reset_selection_scroll();
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
            self.reset_selection_scroll();
            self.clamp_task_step_selection();
        }
    }

    fn next_task(&mut self) {
        if let Some(session) = self.selected_session()
            && !session.tasks.is_empty()
        {
            self.selected_task = (self.selected_task + 1) % session.tasks.len();
            self.selected_step = 0;
            self.reset_selection_scroll();
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
            self.reset_selection_scroll();
            self.clamp_task_step_selection();
        }
    }

    fn next_step(&mut self) {
        if let Some(task) = self.selected_task_record()
            && !task.steps.is_empty()
        {
            self.selected_step = (self.selected_step + 1) % task.steps.len();
            self.reset_selection_scroll();
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
            self.reset_selection_scroll();
        }
    }

    fn reset_selection_scroll(&mut self) {
        self.transcript_scroll = 0;
        self.composer_scroll = 0;
        self.context_scroll = 0;
        self.log_scroll = 0;
        self.detail_scroll = 0;
    }

    fn focus_next_chat_pane(&mut self) {
        self.chat_focus = self.chat_focus.next();
        self.status = format!("chat pane focus: {}", self.chat_focus.label());
    }

    fn focus_previous_chat_pane(&mut self) {
        self.chat_focus = self.chat_focus.previous();
        self.status = format!("chat pane focus: {}", self.chat_focus.label());
    }

    fn scroll_down(&mut self, amount: u16) {
        let offset = self.active_scroll_offset_mut();
        *offset = offset.saturating_add(amount.max(1));
    }

    fn scroll_up(&mut self, amount: u16) {
        let offset = self.active_scroll_offset_mut();
        *offset = offset.saturating_sub(amount.max(1));
    }

    fn scroll_home(&mut self) {
        *self.active_scroll_offset_mut() = 0;
    }

    fn scroll_end(&mut self) {
        *self.active_scroll_offset_mut() = u16::MAX;
    }

    fn active_scroll_offset_mut(&mut self) -> &mut u16 {
        if selected_step_is_chat(self) {
            match self.chat_focus {
                ChatPaneFocus::Transcript => &mut self.transcript_scroll,
                ChatPaneFocus::Composer => &mut self.composer_scroll,
                ChatPaneFocus::Context => &mut self.context_scroll,
                ChatPaneFocus::Log => &mut self.log_scroll,
            }
        } else {
            &mut self.detail_scroll
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
        if let Some(run) = self.active_selected_run_record() {
            return Some(run);
        }
        let attempt = self.selected_step_record()?.attempts.last()?;
        load_attempt_run(&self.store, attempt.run_id)
    }

    fn active_selected_run_record(&self) -> Option<RunRecord> {
        let active = self.active_run.as_ref()?;
        let step = self.selected_step_record()?;
        let task = self.selected_task_record()?;
        let session = self.selected_session()?;
        if active.session_id != session.id || active.task_id != task.id || active.step_id != step.id
        {
            return None;
        }

        self.store.list_runs().ok()?.into_iter().find(|run| {
            run.session_id == active.session_id
                && run.task_id == active.task_id
                && run.step_id == active.step_id
                && run.status == recode_core::RunStatus::Running
        })
    }

    fn begin_prompt_edit(&mut self) -> Result<()> {
        let step = self
            .selected_step_record()
            .ok_or_else(|| anyhow!("no step selected"))?;
        if step.kind != recode_core::StepKind::LlmChat {
            return Err(anyhow!("selected step is not an llm_chat step"));
        }
        self.prompt_buffer = step.prompt.clone().unwrap_or_default();
        self.input_mode = InputMode::EditingPrompt;
        self.status = String::from("Editing prompt, Enter: save+run, Esc: cancel");
        Ok(())
    }

    fn handle_prompt_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                self.prompt_buffer.clear();
                self.status = String::from("Prompt edit cancelled");
            }
            KeyCode::Enter => {
                self.save_prompt_to_selected_step()?;
                self.input_mode = InputMode::Normal;
                self.status = String::from("Prompt saved, running selected chat step");
                self.run_next()?;
                self.prompt_buffer.clear();
            }
            KeyCode::Backspace => {
                self.prompt_buffer.pop();
            }
            KeyCode::Char(ch) => {
                self.prompt_buffer.push(ch);
            }
            _ => {}
        }
        Ok(())
    }

    fn save_prompt_to_selected_step(&mut self) -> Result<()> {
        let session_id = self
            .selected_session()
            .map(|session| session.id)
            .ok_or_else(|| anyhow!("no session selected"))?;
        let task_id = self
            .selected_task_record()
            .map(|task| task.id)
            .ok_or_else(|| anyhow!("no task selected"))?;
        let step_id = self
            .selected_step_record()
            .map(|step| step.id)
            .ok_or_else(|| anyhow!("no step selected"))?;

        let mut session = self.store.load_session(session_id)?;
        let task = session
            .tasks
            .iter_mut()
            .find(|task| task.id == task_id)
            .ok_or_else(|| anyhow!("selected task not found"))?;
        let step = task
            .steps
            .iter_mut()
            .find(|step| step.id == step_id)
            .ok_or_else(|| anyhow!("selected step not found"))?;

        if step.kind != recode_core::StepKind::LlmChat {
            return Err(anyhow!("selected step is not an llm_chat step"));
        }
        step.prompt = Some(self.prompt_buffer.clone());
        task.touch();
        session.touch();
        self.store.save_session(&session)?;
        self.refresh_preserving_message()?;
        Ok(())
    }

    fn run_next(&mut self) -> Result<()> {
        self.run_next_with_options(ExecutorOptions {
            provider: Some(self.provider.clone()),
            ..ExecutorOptions::default()
        })
    }

    fn run_next_background(&mut self) -> Result<()> {
        self.run_next_with_options(ExecutorOptions {
            background: true,
            provider: Some(self.provider.clone()),
            ..ExecutorOptions::default()
        })
    }

    fn run_next_with_options(&mut self, options: ExecutorOptions) -> Result<()> {
        if self.active_run.is_some() {
            return Err(anyhow!("another run is already in flight"));
        }
        let session_id = self
            .selected_session()
            .map(|session| session.id)
            .ok_or_else(|| anyhow!("no session selected"))?;
        let task_id = self
            .selected_task_record()
            .map(|task| task.id)
            .ok_or_else(|| anyhow!("no task selected"))?;
        let step = self
            .selected_step_record()
            .cloned()
            .ok_or_else(|| anyhow!("no step selected"))?;

        if step.kind == recode_core::StepKind::LlmChat && !options.background {
            let (tx, rx) = mpsc::channel();
            let engine = self.engine.clone();
            let provider = self.provider.clone();
            let step_id = step.id;
            thread::spawn(move || {
                let mut runner = ExecutorBridge::with_options(ExecutorOptions {
                    stream_output: true,
                    provider: Some(provider),
                    ..options
                });
                let result = engine
                    .run_next_step(session_id, &mut runner)
                    .map_err(|error| error.to_string());
                let _ = tx.send(result);
            });
            self.active_run = Some(ActiveRunState {
                session_id,
                task_id,
                step_id,
                receiver: rx,
            });
            self.status = format!("streaming run started: {}", step.title);
            return Ok(());
        }

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
        let mut runner = ExecutorBridge::with_options(ExecutorOptions {
            provider: Some(self.provider.clone()),
            ..ExecutorOptions::default()
        });
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

    fn poll_active_run(&mut self) -> Result<()> {
        let Some(active) = &self.active_run else {
            return Ok(());
        };

        match active.receiver.try_recv() {
            Ok(result) => {
                self.active_run = None;
                match result {
                    Ok(result) => {
                        self.status = format!(
                            "run-next: {} {} run={}",
                            result.step_title,
                            serde_json::to_string(&result.disposition)?,
                            result
                                .run_id
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| "-".into())
                        );
                    }
                    Err(error) => {
                        self.status = format!("streaming run failed: {error}");
                    }
                }
                self.refresh_preserving_message()?;
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.refresh_preserving_message()?;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.active_run = None;
                self.status = String::from("streaming run disconnected");
                self.refresh_preserving_message()?;
            }
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
        let app = App::new(store, config.provider.clone(), !cli.no_bootstrap)?;
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

    let result = run_app(
        &mut terminal,
        App::new(store, config.provider.clone(), !cli.no_bootstrap),
    );

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_app(terminal: &mut DefaultTerminal, app_result: Result<App>) -> Result<()> {
    let mut app = app_result?;
    loop {
        if let Err(error) = app.poll_active_run() {
            app.status = format!("streaming poll failed: {error}");
        }
        terminal.draw(|frame| draw(frame, &app))?;

        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
        {
            if app.input_mode == InputMode::EditingPrompt {
                if let Err(error) = app.handle_prompt_key(key) {
                    app.status = format!("prompt edit failed: {error}");
                    app.input_mode = InputMode::Normal;
                }
                continue;
            }

            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Tab => app.focus_next_chat_pane(),
                KeyCode::BackTab => app.focus_previous_chat_pane(),
                KeyCode::PageDown => app.scroll_down(8),
                KeyCode::PageUp => app.scroll_up(8),
                KeyCode::Home => app.scroll_home(),
                KeyCode::End => app.scroll_end(),
                KeyCode::Down | KeyCode::Char('j') => app.next(),
                KeyCode::Up | KeyCode::Char('k') => app.previous(),
                KeyCode::Right | KeyCode::Char('l') => app.next_task(),
                KeyCode::Left | KeyCode::Char('h') => app.previous_task(),
                KeyCode::Char('d') => app.next_step(),
                KeyCode::Char('u') => app.previous_step(),
                KeyCode::Char('e') => {
                    if let Err(error) = app.begin_prompt_edit() {
                        app.status = format!("edit prompt failed: {error}");
                    }
                }
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

    let title = Paragraph::new("Recode TUI, chat-first operator view")
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
        .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
        .split(vertical[2]);

    draw_sessions(frame, app, horizontal[0]);
    draw_details(frame, app, horizontal[1]);

    let footer = Paragraph::new(app.status.as_str())
        .style(Style::default().fg(Color::Yellow))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(match app.input_mode {
                    InputMode::Normal => "Controls",
                    InputMode::EditingPrompt => "Prompt Editor",
                }),
        );
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
    if selected_step_is_chat(app) {
        draw_chat_first_details(frame, app, area);
    } else {
        draw_default_details(frame, app, area);
    }
}

fn draw_default_details(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(46),
            Constraint::Percentage(24),
            Constraint::Percentage(30),
        ])
        .split(area);

    let text = if let Some(session) = app.selected_session() {
        session_detail_lines(&app.store, session, app.selected_task, app.selected_step)
    } else {
        vec![Line::from("No session selected")]
    };

    let paragraph = Paragraph::new(text.clone())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Session Detail"),
        )
        .scroll((
            clamp_scroll(app.detail_scroll, text.len(), split[0].height),
            0,
        ))
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

    let transcript_text = selected_transcript_lines(app);
    let transcript_paragraph = Paragraph::new(transcript_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Selected Chat Transcript"),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(transcript_paragraph, split[2]);
}

fn draw_chat_first_details(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(56),
            Constraint::Percentage(18),
            Constraint::Percentage(26),
        ])
        .split(area);

    let transcript_lines = selected_transcript_lines(app);
    let transcript = Paragraph::new(transcript_lines.clone())
        .block(focused_block(
            app,
            ChatPaneFocus::Transcript,
            "Chat Transcript",
        ))
        .scroll((
            clamp_scroll(
                app.transcript_scroll,
                transcript_lines.len(),
                split[0].height,
            ),
            0,
        ))
        .wrap(Wrap { trim: false });
    frame.render_widget(transcript, split[0]);

    let composer_lines = selected_chat_prompt_lines(app);
    let composer = Paragraph::new(composer_lines.clone())
        .block(focused_block(app, ChatPaneFocus::Composer, "Composer"))
        .scroll((
            clamp_scroll(app.composer_scroll, composer_lines.len(), split[1].height),
            0,
        ))
        .wrap(Wrap { trim: false });
    frame.render_widget(composer, split[1]);

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(split[2]);

    let context_lines = selected_chat_context_lines(&app.store, app);
    let context = Paragraph::new(context_lines.clone())
        .block(focused_block(app, ChatPaneFocus::Context, "Chat Context"))
        .scroll((
            clamp_scroll(app.context_scroll, context_lines.len(), bottom[0].height),
            0,
        ))
        .wrap(Wrap { trim: false });
    frame.render_widget(context, bottom[0]);

    let log_text = selected_log_lines(app);
    let log_paragraph = Paragraph::new(log_text.clone())
        .block(focused_block(app, ChatPaneFocus::Log, "Run Log Tail"))
        .scroll((
            clamp_scroll(app.log_scroll, log_text.len(), bottom[1].height),
            0,
        ))
        .wrap(Wrap { trim: false });
    frame.render_widget(log_paragraph, bottom[1]);
}

fn focused_block(app: &App, pane: ChatPaneFocus, title: &'static str) -> Block<'static> {
    let block = Block::default().borders(Borders::ALL).title(title);
    if app.input_mode == InputMode::Normal && app.chat_focus == pane {
        block.border_style(Style::default().fg(Color::Yellow))
    } else {
        block
    }
}

fn clamp_scroll(offset: u16, total_lines: usize, area_height: u16) -> u16 {
    let visible = area_height.saturating_sub(2) as usize;
    if visible == 0 || total_lines <= visible {
        return 0;
    }
    let max_offset = total_lines.saturating_sub(visible);
    offset.min(max_offset.min(u16::MAX as usize) as u16)
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

    if let Some(llm_line) = llm_summary_line(run) {
        lines.push(Line::from(format!("        llm {llm_line}")));
    }

    lines
}

fn llm_summary_line(run: &RunRecord) -> Option<String> {
    let path = run.response_artifact_path.as_deref()?;
    let raw = fs::read_to_string(path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let provider = parsed
        .get("provider")
        .and_then(|v| v.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let model = parsed
        .get("provider")
        .and_then(|v| v.get("model"))
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let usage = parsed.get("usage");
    let prompt_tokens = usage
        .and_then(|v| v.get("prompt_tokens"))
        .and_then(|v| v.as_u64());
    let completion_tokens = usage
        .and_then(|v| v.get("completion_tokens"))
        .and_then(|v| v.as_u64());
    let total_tokens = usage
        .and_then(|v| v.get("total_tokens"))
        .and_then(|v| v.as_u64());

    Some(format!(
        "provider={} model={} prompt_tokens={} completion_tokens={} total_tokens={}",
        provider,
        model,
        prompt_tokens
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".into()),
        completion_tokens
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".into()),
        total_tokens
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".into())
    ))
}

fn selected_log_lines(app: &App) -> Vec<Line<'static>> {
    let Some(_step) = app.selected_step_record() else {
        return vec![Line::from("No step selected")];
    };
    let Some(run) = app.selected_run_record() else {
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

fn selected_step_is_chat(app: &App) -> bool {
    app.selected_step_record()
        .map(|step| step.kind == recode_core::StepKind::LlmChat)
        .unwrap_or(false)
}

fn selected_transcript_lines(app: &App) -> Vec<Line<'static>> {
    let Some(step) = app.selected_step_record() else {
        return vec![Line::from("No step selected")];
    };
    if step.kind != recode_core::StepKind::LlmChat {
        return vec![Line::from("Selected step is not an llm_chat step")];
    }

    let Some(last_attempt) = step.attempts.last() else {
        return vec![Line::from("No attempts for selected chat step")];
    };
    let Some(run) = load_attempt_run(&app.store, last_attempt.run_id) else {
        return vec![Line::from("No run metadata for selected chat step")];
    };
    let Some(path) = run.transcript_artifact_path.as_deref() else {
        return vec![Line::from("No transcript artifact path recorded")];
    };

    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => return vec![Line::from(format!("Transcript unreadable: {error}"))],
    };
    let parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(parsed) => parsed,
        Err(error) => return vec![Line::from(format!("Transcript JSON invalid: {error}"))],
    };

    let mut lines = vec![Line::from(format!("artifact {path}")), Line::from("")];
    let Some(messages) = parsed.get("messages").and_then(|v| v.as_array()) else {
        lines.push(Line::from("No messages found in transcript artifact"));
        return lines;
    };
    let streaming = parsed
        .get("streaming")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        && run.status == recode_core::RunStatus::Running;

    if messages.is_empty() {
        lines.push(Line::from("Transcript is empty"));
        return lines;
    }

    for (index, message) in messages.iter().enumerate() {
        let role = message
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let content = message
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let role_style = match role {
            "user" => Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            "assistant" => Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            _ => Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        };

        let mut role_label = format!("[{index}] {role}");
        if streaming && role == "assistant" && index + 1 == messages.len() {
            role_label.push_str(" (streaming)");
        }

        lines.push(Line::from(vec![Span::styled(role_label, role_style)]));

        if content.trim().is_empty() {
            lines.push(Line::from("  <empty>"));
        } else {
            for line in content.lines() {
                lines.push(Line::from(format!("  {line}")));
            }
        }
        lines.push(Line::from(""));
    }

    lines
}

fn selected_chat_prompt_lines(app: &App) -> Vec<Line<'static>> {
    let Some(step) = app.selected_step_record() else {
        return vec![Line::from("No step selected")];
    };

    let prompt = match app.input_mode {
        InputMode::Normal => step.prompt.clone().unwrap_or_default(),
        InputMode::EditingPrompt => app.prompt_buffer.clone(),
    };

    let helper = match app.input_mode {
        InputMode::Normal => {
            "Press e to edit the next user turn. Enter saves and runs. Tab changes pane, PgUp/PgDn scroll."
        }
        InputMode::EditingPrompt => "Editing prompt. Enter saves+runs, Esc cancels.",
    };

    let mut lines = vec![Line::from(vec![
        Span::styled("step ", label_style()),
        Span::raw(step.title.clone()),
    ])];
    lines.push(Line::from(vec![
        Span::styled("status ", label_style()),
        Span::raw(format!(
            "{:?} attempts={}",
            step.status,
            step.attempts.len()
        )),
    ]));
    lines.push(Line::from(vec![Span::styled(
        helper,
        Style::default().fg(Color::Yellow),
    )]));
    lines.push(Line::from(""));

    if prompt.trim().is_empty() {
        lines.push(Line::from("<empty prompt>"));
    } else {
        for line in prompt.lines() {
            lines.push(Line::from(line.to_string()));
        }
    }

    lines
}

fn selected_chat_context_lines(store: &SessionStore, app: &App) -> Vec<Line<'static>> {
    let Some(session) = app.selected_session() else {
        return vec![Line::from("No session selected")];
    };
    let Some(task) = app.selected_task_record() else {
        return vec![Line::from("No task selected")];
    };
    let Some(step) = app.selected_step_record() else {
        return vec![Line::from("No step selected")];
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("session ", label_style()),
            Span::raw(session.name.clone()),
        ]),
        Line::from(vec![
            Span::styled("task ", label_style()),
            Span::raw(format!("{} [{:?}]", task.title, task.status)),
        ]),
        Line::from(vec![
            Span::styled("step ", label_style()),
            Span::raw(format!("{} [{:?}]", step.title, step.status)),
        ]),
    ];

    if let Some(last) = step.attempts.last() {
        lines.push(Line::from(vec![
            Span::styled("attempt ", label_style()),
            Span::raw(format!("#{} {:?}", last.number, last.status)),
        ]));
        if let Some(run) = load_attempt_run(store, last.run_id) {
            lines.push(Line::from(vec![
                Span::styled("run ", label_style()),
                Span::raw(format!("{:?} {:?}", run.status, run.mode)),
            ]));
            if let Some(llm_line) = llm_summary_line(&run) {
                lines.push(Line::from(llm_line));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Task steps",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    for (index, task_step) in task.steps.iter().enumerate() {
        let marker = if index == app.selected_step {
            "▶"
        } else {
            " "
        };
        lines.push(Line::from(format!(
            "{marker} {} [{:?}] attempts={}",
            task_step.title,
            task_step.status,
            task_step.attempts.len()
        )));
    }

    lines
}

fn status_banner_lines(app: &App) -> Vec<Line<'static>> {
    let Some(session) = app.selected_session() else {
        return vec![Line::from("No session selected")];
    };

    let task = session.tasks.get(app.selected_task);
    let step = task.and_then(|task| task.steps.get(app.selected_step));
    let run = app.selected_run_record();

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
            let llm_suffix = llm_summary_line(run)
                .map(|summary| format!(" | {summary}"))
                .unwrap_or_default();
            format!(
                "{:?} pid={} {}{}",
                run.status,
                run.pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "-".into()),
                run.summary.clone().unwrap_or_default(),
                llm_suffix,
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
        Line::from(vec![
            Span::styled("prompt ", label_style()),
            Span::raw(match app.input_mode {
                InputMode::Normal => step
                    .and_then(|step| step.prompt.clone())
                    .unwrap_or_else(|| "-".into()),
                InputMode::EditingPrompt => format!("{}█", app.prompt_buffer),
            }),
        ]),
        Line::from(vec![
            Span::styled("pane ", label_style()),
            Span::raw(if selected_step_is_chat(app) {
                app.chat_focus.label().to_string()
            } else {
                format!("detail scroll={}", app.detail_scroll)
            }),
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
                            "      stdout={} stderr={} exit_code={} transcript={}",
                            run.stdout_log_path.unwrap_or_else(|| "-".into()),
                            run.stderr_log_path.unwrap_or_else(|| "-".into()),
                            run.exit_code_path.unwrap_or_else(|| "-".into()),
                            run.transcript_artifact_path.unwrap_or_else(|| "-".into())
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
        provider_mode: None,
        provider_base_url: None,
        provider_api_key_env: None,
        provider_model: None,
        default_timeout_secs: cli.default_timeout_secs,
        default_max_attempts: cli.default_max_attempts,
        approval_policy: cli
            .approval_policy
            .as_deref()
            .and_then(recode_core::ApprovalPolicy::parse),
    }
}

#[cfg(test)]
mod tests {
    use super::{ChatPaneFocus, clamp_scroll};

    #[test]
    fn clamp_scroll_limits_to_visible_window() {
        assert_eq!(clamp_scroll(0, 3, 10), 0);
        assert_eq!(clamp_scroll(50, 20, 6), 16);
        assert_eq!(clamp_scroll(u16::MAX, 20, 6), 16);
    }

    #[test]
    fn chat_pane_focus_cycles() {
        assert_eq!(ChatPaneFocus::Transcript.next(), ChatPaneFocus::Composer);
        assert_eq!(ChatPaneFocus::Log.next(), ChatPaneFocus::Transcript);
        assert_eq!(ChatPaneFocus::Transcript.previous(), ChatPaneFocus::Log);
    }
}
