//! Main TUI application shell.
//!
//! Owns the terminal state machine, drives the render loop, dispatches
//! [`TuiAction`]s, and coordinates background data refreshes.
//!
//! # Frame budget (target 60 fps, flush every 6 frames → ~10 fps effective)
//!
//! | Phase            | Budget    |
//! |------------------|-----------|
//! | Input poll       | ~0.1 ms   |
//! | Data refresh     | ~2.0 ms   |
//! | Render           | ~4.0 ms   |
//! | Terminal flush   | ~2.0 ms   |
//! | **Total**        | **~8 ms** |

use std::io::Stdout;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use crossterm::{
    cursor::Show,
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream,
    },
    execute,
    terminal::{enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::{future::BoxFuture, FutureExt as _, Stream, StreamExt as _};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    widgets::Block,
    Frame, Terminal,
};

use polkagent_interaction::{ApprovalDecision, InteractionErrorCode};
use polkagent_runtime::PolkagentRuntime;
use polkagent_store_sqlite::SqlitePool;

use crate::tui::{
    input::{key_to_action, InputMode, TuiAction},
    interaction::{
        ApprovalDecisionRequest, ApprovalListRequest, ConsoleCommandSubmission, ControllerEvent,
        ControllerUpdate, RunController,
    },
    state::{ApprovalActionTarget, ApprovalQueueStatus, ScrollState, TuiState},
    theme::Theme,
    views,
    widgets::{header_bar, status_bar},
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Internal tick rate.
pub const TARGET_FPS: u64 = 60;
/// Duration of one frame.
pub const FRAME_DURATION: Duration = Duration::from_micros(1_000_000 / TARGET_FPS);
/// Flush the terminal every N frames (10 fps effective).
pub const FLUSH_DIVISOR: u64 = 6;
/// Flush divisor used when the TUI has been idle for 5 seconds (5 fps).
pub const FLUSH_DIVISOR_IDLE: u64 = 12;
/// Refresh database data every N seconds.
pub const REFRESH_INTERVAL_SECS: u64 = 5;
/// Bounded terminal/tick queue. Key and paste events apply backpressure while
/// ticks and resize notifications coalesce independently.
const EVENT_CHANNEL_CAPACITY: usize = 64;
const BACKGROUND_RESULT_CAPACITY: usize = 4;
/// Covers the five sequential one-second chain RPC exchanges plus teardown
/// overhead. `SQLite` readers use a shorter busy timeout.
const BACKGROUND_SHUTDOWN_GRACE: Duration = Duration::from_secs(6);
const BACKGROUND_ABORT_REAP_GRACE: Duration = Duration::from_millis(25);
#[cfg(test)]
const TEST_BACKGROUND_SHUTDOWN_GRACE: Duration = Duration::from_millis(25);

// ---------------------------------------------------------------------------
// Event pump
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum PumpSignal {
    Terminal(Event),
    Tick,
    InputClosed(Option<String>),
}

#[derive(Debug)]
enum EventLoopEvent {
    Terminal(Event),
    Tick,
    Background(Box<BackgroundEvent>),
}

#[derive(Debug)]
enum BackgroundEvent {
    Controller(Box<ControllerUpdate>),
    Projection(BackgroundResult),
}

#[derive(Debug)]
struct RefreshRequest {
    generation: u64,
    pool: SqlitePool,
    selected_run: Option<String>,
}

#[derive(Debug)]
struct ChainRequest {
    poller: crate::tui::db::ChainPoller,
}

#[derive(Debug)]
struct ChainJobOutput {
    poller: crate::tui::db::ChainPoller,
    result: Option<std::result::Result<crate::tui::db::ChainStatus, String>>,
}

#[derive(Debug)]
enum BackgroundResult {
    Refresh {
        generation: u64,
        result: Box<std::result::Result<crate::tui::db::TuiProjectionSnapshot, String>>,
    },
    Chain {
        generation: u64,
        result: std::result::Result<ChainJobOutput, String>,
    },
}

enum ProjectionUpdate {
    Refresh(Box<crate::tui::db::TuiProjectionSnapshot>),
    Chain(std::result::Result<crate::tui::db::ChainStatus, String>),
    Failure(String),
}

type RefreshWorker = Arc<
    dyn Fn(
            RefreshRequest,
        )
            -> BoxFuture<'static, std::result::Result<crate::tui::db::TuiProjectionSnapshot, String>>
        + Send
        + Sync,
>;
type ChainWorker = Arc<
    dyn Fn(ChainRequest) -> BoxFuture<'static, std::result::Result<ChainJobOutput, String>>
        + Send
        + Sync,
>;

/// Schedules bounded monitoring refreshes without touching terminal state.
struct BackgroundJobs {
    pool: SqlitePool,
    result_tx: tokio::sync::mpsc::Sender<BackgroundResult>,
    result_rx: tokio::sync::mpsc::Receiver<BackgroundResult>,
    /// Injected async workers are used by deterministic headless tests.
    /// Production uses directly tracked `spawn_blocking` handles instead of
    /// nesting them below an abortable async task.
    refresh_worker: Option<RefreshWorker>,
    chain_worker: Option<ChainWorker>,
    refresh_generation: u64,
    refresh_in_flight: Option<u64>,
    pending_refresh: Option<RefreshRequest>,
    chain_generation: u64,
    chain_in_flight: Option<u64>,
    chain_poller: Option<crate::tui::db::ChainPoller>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    shutdown_grace: Duration,
}

impl BackgroundJobs {
    fn new(pool: SqlitePool, chain_poller: crate::tui::db::ChainPoller) -> Self {
        Self::build(pool, chain_poller, None, None, BACKGROUND_SHUTDOWN_GRACE)
    }

    #[cfg(test)]
    fn with_workers(
        pool: SqlitePool,
        chain_poller: crate::tui::db::ChainPoller,
        refresh_worker: RefreshWorker,
        chain_worker: ChainWorker,
    ) -> Self {
        Self::build(
            pool,
            chain_poller,
            Some(refresh_worker),
            Some(chain_worker),
            TEST_BACKGROUND_SHUTDOWN_GRACE,
        )
    }

    fn build(
        pool: SqlitePool,
        chain_poller: crate::tui::db::ChainPoller,
        refresh_worker: Option<RefreshWorker>,
        chain_worker: Option<ChainWorker>,
        shutdown_grace: Duration,
    ) -> Self {
        let (result_tx, result_rx) = tokio::sync::mpsc::channel(BACKGROUND_RESULT_CAPACITY);
        Self {
            pool,
            result_tx,
            result_rx,
            refresh_worker,
            chain_worker,
            refresh_generation: 0,
            refresh_in_flight: None,
            pending_refresh: None,
            chain_generation: 0,
            chain_in_flight: None,
            chain_poller: Some(chain_poller),
            tasks: Vec::new(),
            shutdown_grace,
        }
    }

    fn request_refresh(&mut self, selected_run: Option<String>) {
        self.refresh_generation = self.refresh_generation.wrapping_add(1);
        let request = RefreshRequest {
            generation: self.refresh_generation,
            pool: self.pool.clone(),
            selected_run,
        };
        if self.refresh_in_flight.is_some() {
            self.pending_refresh = Some(request);
        } else {
            self.spawn_refresh(request);
        }
    }

    fn spawn_refresh(&mut self, request: RefreshRequest) {
        let generation = request.generation;
        self.refresh_in_flight = Some(generation);
        let result_tx = self.result_tx.clone();
        let task = if let Some(worker) = self.refresh_worker.clone() {
            tokio::spawn(async move {
                let result = std::panic::AssertUnwindSafe(worker(request))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| Err("monitoring refresh worker panicked".to_owned()));
                let _ = result_tx
                    .send(BackgroundResult::Refresh {
                        generation,
                        result: Box::new(result),
                    })
                    .await;
            })
        } else {
            tokio::task::spawn_blocking(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::tui::db::load_projection_snapshot(
                        &request.pool,
                        request.selected_run.as_deref(),
                    )
                }))
                .map_err(|_| "monitoring refresh worker panicked".to_owned());
                let _ = result_tx.blocking_send(BackgroundResult::Refresh {
                    generation,
                    result: Box::new(result),
                });
            })
        };
        self.track_task(task);
    }

    fn request_chain_poll(&mut self) {
        if self.chain_in_flight.is_some() {
            // The in-flight result satisfies every request made while it was
            // running. The normal interval will schedule the next poll.
            return;
        }
        let Some(poller) = self.chain_poller.take() else {
            return;
        };
        if !poller.should_poll() {
            self.chain_poller = Some(poller);
            return;
        }
        self.chain_generation = self.chain_generation.wrapping_add(1);
        let generation = self.chain_generation;
        self.chain_in_flight = Some(generation);
        let request = ChainRequest { poller };
        let result_tx = self.result_tx.clone();
        let task = if let Some(worker) = self.chain_worker.clone() {
            tokio::spawn(async move {
                let result = std::panic::AssertUnwindSafe(worker(request))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| Err("chain polling worker panicked".to_owned()));
                let _ = result_tx
                    .send(BackgroundResult::Chain { generation, result })
                    .await;
            })
        } else {
            tokio::task::spawn_blocking(move || {
                let mut poller = request.poller;
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| poller.poll_status()))
                        .unwrap_or_else(|_| Some(Err("chain polling worker panicked".to_owned())));
                let result = Ok(ChainJobOutput { poller, result });
                let _ = result_tx.blocking_send(BackgroundResult::Chain { generation, result });
            })
        };
        self.track_task(task);
    }

    fn track_task(&mut self, task: tokio::task::JoinHandle<()>) {
        self.tasks.retain(|task| !task.is_finished());
        self.tasks.push(task);
    }

    async fn recv(&mut self) -> Option<BackgroundResult> {
        self.result_rx.recv().await
    }

    fn complete(&mut self, result: BackgroundResult) -> Option<ProjectionUpdate> {
        match result {
            BackgroundResult::Refresh { generation, result } => {
                if self.refresh_in_flight != Some(generation) {
                    return None;
                }
                self.refresh_in_flight = None;
                if let Some(pending) = self.pending_refresh.take() {
                    self.spawn_refresh(pending);
                    return None;
                }
                if generation != self.refresh_generation {
                    return None;
                }
                Some(match *result {
                    Ok(snapshot) => ProjectionUpdate::Refresh(Box::new(snapshot)),
                    Err(error) => ProjectionUpdate::Failure(error),
                })
            }
            BackgroundResult::Chain { generation, result } => {
                if self.chain_in_flight != Some(generation) {
                    return None;
                }
                self.chain_in_flight = None;
                match result {
                    Ok(output) => {
                        self.chain_poller = Some(output.poller);
                        if generation != self.chain_generation {
                            return None;
                        }
                        output.result.map(ProjectionUpdate::Chain)
                    }
                    Err(error) => Some(ProjectionUpdate::Failure(error)),
                }
            }
        }
    }

    /// Wait for the actual tracked jobs, including production blocking jobs.
    /// A job that exceeds the finite I/O ceiling is aborted when possible and
    /// reported as detached if the blocking OS thread cannot be reaped.
    async fn shutdown(&mut self) -> usize {
        self.refresh_in_flight = None;
        self.pending_refresh = None;
        self.chain_in_flight = None;

        let mut tasks = std::mem::take(&mut self.tasks);
        let completed = tokio::time::timeout(self.shutdown_grace, async {
            for task in &mut tasks {
                let _ = task.await;
            }
        })
        .await
        .is_ok();
        if completed {
            return 0;
        }

        let mut unfinished = tasks
            .into_iter()
            .filter(|task| !task.is_finished())
            .collect::<Vec<_>>();
        for task in &unfinished {
            task.abort();
        }
        let _ = tokio::time::timeout(BACKGROUND_ABORT_REAP_GRACE, async {
            for task in &mut unfinished {
                let _ = task.await;
            }
        })
        .await;
        unfinished.iter().filter(|task| !task.is_finished()).count()
    }
}

impl Drop for BackgroundJobs {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Owns the terminal-input and clock producers for the main event loop.
///
/// Key/paste/mouse events use a bounded queue and await capacity. Resize uses
/// a watch channel so a resize storm retains only the newest dimensions. Tick
/// delivery is best-effort because one queued tick is enough to drive refresh
/// and rendering after a delayed frame.
struct EventPump {
    signal_rx: tokio::sync::mpsc::Receiver<PumpSignal>,
    resize_rx: tokio::sync::watch::Receiver<Option<(u16, u16)>>,
    _resize_tx: tokio::sync::watch::Sender<Option<(u16, u16)>>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl EventPump {
    fn start() -> Self {
        Self::start_with_input(EventStream::new(), FRAME_DURATION, EVENT_CHANNEL_CAPACITY)
    }

    fn start_with_input<S>(input: S, tick_period: Duration, capacity: usize) -> Self
    where
        S: Stream<Item = std::io::Result<Event>> + Send + Unpin + 'static,
    {
        let (signal_tx, signal_rx) = tokio::sync::mpsc::channel(capacity);
        let (resize_tx, resize_rx) = tokio::sync::watch::channel(None);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        let input_task = tokio::spawn(run_input_source(
            input,
            signal_tx.clone(),
            resize_tx.clone(),
            shutdown_rx.clone(),
        ));
        let tick_task = tokio::spawn(run_tick_source(signal_tx, shutdown_rx, tick_period));

        Self {
            signal_rx,
            resize_rx,
            _resize_tx: resize_tx,
            shutdown_tx,
            tasks: vec![input_task, tick_task],
        }
    }

    async fn next_event<F>(&mut self, background: F) -> Result<EventLoopEvent>
    where
        F: std::future::Future<Output = Option<BackgroundEvent>>,
    {
        tokio::pin!(background);
        tokio::select! {
            resize = self.resize_rx.changed() => {
                resize.map_err(|_| anyhow!("terminal resize source closed"))?;
                let Some((width, height)) = *self.resize_rx.borrow_and_update() else {
                    return Err(anyhow!("terminal resize source changed without dimensions"));
                };
                Ok(EventLoopEvent::Terminal(Event::Resize(width, height)))
            }
            signal = self.signal_rx.recv() => match signal {
                Some(PumpSignal::Terminal(event)) => Ok(EventLoopEvent::Terminal(event)),
                Some(PumpSignal::Tick) => Ok(EventLoopEvent::Tick),
                Some(PumpSignal::InputClosed(Some(error))) => {
                    Err(anyhow!("terminal input failed: {error}"))
                }
                Some(PumpSignal::InputClosed(None)) => Err(anyhow!("terminal input closed")),
                None => Err(anyhow!("terminal event pump stopped unexpectedly")),
            },
            event = &mut background => event
                .map(Box::new)
                .map(EventLoopEvent::Background)
                .ok_or_else(|| anyhow!("TUI background event channel closed")),
        }
    }

    async fn shutdown(&mut self) -> Result<()> {
        let _ = self.shutdown_tx.send(true);
        let mut errors = Vec::new();
        for task in self.tasks.drain(..) {
            if let Err(error) = task.await {
                errors.push(error.to_string());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(anyhow!(
                "TUI event-pump shutdown failed: {}",
                errors.join("; ")
            ))
        }
    }
}

impl Drop for EventPump {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn run_input_source<S>(
    mut input: S,
    signal_tx: tokio::sync::mpsc::Sender<PumpSignal>,
    resize_tx: tokio::sync::watch::Sender<Option<(u16, u16)>>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) where
    S: Stream<Item = std::io::Result<Event>> + Send + Unpin,
{
    loop {
        let next = tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    return;
                }
                continue;
            }
            next = input.next() => next,
        };

        let signal = match next {
            Some(Ok(Event::Resize(width, height))) => {
                resize_tx.send_replace(Some((width, height)));
                continue;
            }
            Some(Ok(event)) => PumpSignal::Terminal(event),
            Some(Err(error)) => PumpSignal::InputClosed(Some(error.to_string())),
            None => PumpSignal::InputClosed(None),
        };
        let terminal = matches!(signal, PumpSignal::InputClosed(_));
        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    return;
                }
            }
            result = signal_tx.send(signal) => {
                if result.is_err() || terminal {
                    return;
                }
            }
        }
    }
}

async fn run_tick_source(
    signal_tx: tokio::sync::mpsc::Sender<PumpSignal>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    tick_period: Duration,
) {
    let start = tokio::time::Instant::now() + tick_period;
    let mut ticks = tokio::time::interval_at(start, tick_period);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    return;
                }
            }
            _ = ticks.tick() => match signal_tx.try_send(PumpSignal::Tick) {
                Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tab
// ---------------------------------------------------------------------------

/// Top-level region tab (F1–F9) plus pseudo-tabs for drill-down views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// F1 — Overview: agent grid, run summary, system health.
    #[default]
    Dashboard,
    /// F2 — Agent list and detail.
    Agents,
    /// F3 — Run list and detail.
    Runs,
    /// F4 — System health and configuration.
    System,
    /// F5 — Event timeline for selected run.
    Timeline,
    /// F6 — Conversation-scoped durable approval queue.
    Approvals,
    /// F7 — Memory browser.
    Memory,
    /// F8 — Audit log.
    Audit,
    /// F9 — Interactive agent console.
    Console,
    /// Run detail (entered from Runs via Enter; not a top-level F-key tab).
    RunDetail,
}

#[allow(dead_code)]
impl Tab {
    /// All tabs in display order (excludes pseudo-tabs like `RunDetail`).
    pub const ALL: [Tab; 9] = [
        Tab::Dashboard,
        Tab::Agents,
        Tab::Runs,
        Tab::System,
        Tab::Timeline,
        Tab::Approvals,
        Tab::Memory,
        Tab::Audit,
        Tab::Console,
    ];

    /// Display name used in the header bar and status bar.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "DASHBOARD",
            Self::Agents => "AGENTS",
            Self::Runs => "RUNS",
            Self::System => "SYSTEM",
            Self::Timeline => "TIMELINE",
            Self::Approvals => "APPROVALS",
            Self::Memory => "MEMORY",
            Self::Audit => "AUDIT",
            Self::Console => "CONSOLE",
            Self::RunDetail => "RUN DETAIL",
        }
    }

    /// F-key indicator shown next to the tab name.
    #[must_use]
    pub fn fkey_label(self) -> &'static str {
        match self {
            Self::Dashboard => "[F1]",
            Self::Agents => "[F2]",
            Self::Runs => "[F3]",
            Self::System => "[F4]",
            Self::Timeline => "[F5]",
            Self::Approvals => "[F6]",
            Self::Memory => "[F7]",
            Self::Audit => "[F8]",
            Self::Console => "[F9]",
            Self::RunDetail => "[--]",
        }
    }

    /// Return the next tab in display order (wraps around).
    /// `RunDetail` maps to its parent (Runs).
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Dashboard => Self::Agents,
            Self::Agents => Self::Runs,
            Self::Runs | Self::RunDetail => Self::System,
            Self::System => Self::Timeline,
            Self::Timeline => Self::Approvals,
            Self::Approvals => Self::Memory,
            Self::Memory => Self::Audit,
            Self::Audit => Self::Console,
            Self::Console => Self::Dashboard,
        }
    }

    /// Parse a tab name from the `--tab` CLI flag value.
    ///
    /// Accepted values match the flag's documented names (case-insensitive).
    /// Unknown names fall back to [`Tab::Dashboard`].
    #[must_use]
    pub fn from_cli_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "agents" => Self::Agents,
            "runs" => Self::Runs,
            "system" => Self::System,
            "timeline" => Self::Timeline,
            "approvals" => Self::Approvals,
            "memory" => Self::Memory,
            "audit" => Self::Audit,
            "console" | "chat" => Self::Console,
            _ => Self::Dashboard,
        }
    }

    /// Return the previous tab in display order (wraps around).
    /// `RunDetail` maps to its parent (Runs).
    #[must_use]
    pub fn prev(self) -> Self {
        match self {
            Self::Dashboard => Self::Console,
            Self::Agents => Self::Dashboard,
            Self::Runs => Self::Agents,
            Self::System | Self::RunDetail => Self::Runs,
            Self::Timeline => Self::System,
            Self::Approvals => Self::Timeline,
            Self::Memory => Self::Approvals,
            Self::Audit => Self::Memory,
            Self::Console => Self::Audit,
        }
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

/// Primary TUI application shell.
///
/// Owns terminal state, navigation, theme, and all mutable data. The render
/// path reads only `&self` — zero I/O in the hot path.
pub struct App {
    // -- Navigation ----------------------------------------------------------
    pub active_tab: Tab,

    // -- State ---------------------------------------------------------------
    pub tui_state: TuiState,
    pub theme: Theme,

    // -- Input ---------------------------------------------------------------
    pub input_mode: InputMode,

    // -- Loop control --------------------------------------------------------
    pub running: bool,
    pub frame_counter: u64,
    pub last_input: Instant,
    pub last_refresh: Instant,

    // -- Data source ---------------------------------------------------------
    pub pool: SqlitePool,

    // -- Background monitoring projections ---------------------------------
    background_jobs: BackgroundJobs,

    // -- Interactive execution ---------------------------------------------
    pub run_controller: RunController,
}

impl App {
    /// Create an `App` over one process-wide runtime and its durable pool.
    pub fn new(theme: Theme, runtime: PolkagentRuntime, initial_tab: Tab) -> Self {
        let pool = runtime.pool().clone();
        let run_controller = RunController::new(runtime);
        let background_jobs = BackgroundJobs::new(pool.clone(), crate::tui::db::ChainPoller::new());
        Self {
            active_tab: initial_tab,
            tui_state: TuiState::default(),
            theme,
            input_mode: InputMode::default(),
            running: true,
            frame_counter: 0,
            last_input: Instant::now(),
            last_refresh: Instant::now()
                .checked_sub(Duration::from_secs(REFRESH_INTERVAL_SECS + 1))
                .unwrap_or_else(Instant::now),
            pool,
            background_jobs,
            run_controller,
        }
    }

    // ── Event loop ──────────────────────────────────────────────────────────

    /// Run the main event loop until `self.running` becomes `false`.
    pub async fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        self.request_data_refresh();
        terminal.draw(|frame| self.render(frame))?;
        self.tui_state.dirty = false;

        let mut pump = EventPump::start();
        let run_result = self.run_with_pump(terminal, &mut pump).await;
        let shutdown_result = pump.shutdown().await;
        let worker_shutdown_result = self.shutdown_workers().await;
        run_result.and(shutdown_result).and(worker_shutdown_result)
    }

    /// Stop all work owned by the application. This is idempotent so the
    /// launcher can call it after catching an event-loop panic as well as the
    /// normal event-loop path.
    pub async fn shutdown_workers(&mut self) -> Result<()> {
        let detached = self.background_jobs.shutdown().await;
        self.run_controller.shutdown().await;
        if detached == 0 {
            Ok(())
        } else {
            Err(anyhow!(
                "TUI shutdown deadline expired with {detached} blocking monitoring job(s) still running"
            ))
        }
    }

    async fn run_with_pump(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        pump: &mut EventPump,
    ) -> Result<()> {
        while self.running {
            let background = async {
                tokio::select! {
                    event = self.run_controller.recv_update() => {
                        event.map(|event| BackgroundEvent::Controller(Box::new(event)))
                    }
                    result = self.background_jobs.recv() => {
                        result.map(BackgroundEvent::Projection)
                    }
                }
            };
            match pump.next_event(background).await? {
                EventLoopEvent::Terminal(event) => {
                    if matches!(event, Event::Key(_) | Event::Paste(_)) {
                        self.last_input = Instant::now();
                    }
                    if let Some(action) = terminal_event_to_action(event, self.input_mode) {
                        self.apply_action(action);
                    }
                }
                EventLoopEvent::Background(event) => match *event {
                    BackgroundEvent::Controller(event) => self.apply_controller_event(*event),
                    BackgroundEvent::Projection(result) => self.apply_background_result(result),
                },
                EventLoopEvent::Tick => self.tick(terminal)?,
            }
        }

        Ok(())
    }

    fn tick(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        if self.last_refresh.elapsed().as_secs() >= REFRESH_INTERVAL_SECS {
            self.request_data_refresh();
            if self.active_tab == Tab::Approvals && !self.run_controller.is_control_active() {
                self.refresh_approvals();
            }
        }

        self.background_jobs.request_chain_poll();

        self.frame_counter = self.frame_counter.wrapping_add(1);
        let idle = self.last_input.elapsed().as_secs() > 5;
        let divisor = if idle {
            FLUSH_DIVISOR_IDLE
        } else {
            FLUSH_DIVISOR
        };
        if self.frame_counter.is_multiple_of(divisor) || self.tui_state.dirty {
            terminal.draw(|frame| self.render(frame))?;
            self.tui_state.dirty = false;
        }

        Ok(())
    }

    // ── Action dispatch ─────────────────────────────────────────────────────

    /// Apply a [`TuiAction`] to `self`, mutating `TuiState` as needed.
    #[allow(
        clippy::too_many_lines,
        reason = "the exhaustive TUI state-machine dispatcher keeps action ordering and shared dirty-state updates in one auditable boundary"
    )]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "the event loop transfers ownership of each short-lived action into the state-machine dispatch boundary"
    )]
    pub fn apply_action(&mut self, action: TuiAction) {
        match action {
            TuiAction::NavigateTab(tab) => {
                self.active_tab = tab;
                if tab == Tab::Console {
                    self.ensure_console_agent();
                }
                // When switching to timeline, refresh events for the selected run.
                if tab == Tab::Timeline {
                    self.refresh_run_events();
                }
                // When switching to approvals, refresh the approval queue.
                if tab == Tab::Approvals {
                    self.refresh_approvals();
                }
                // When switching to memory, refresh memory entries.
                if tab == Tab::Memory {
                    self.refresh_memory();
                }
                // When switching to audit, refresh the audit log.
                if tab == Tab::Audit {
                    self.refresh_audit_log();
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::NavigateUp => match self.active_tab {
                Tab::Agents => {
                    self.tui_state.agents_scroll.up();
                    self.tui_state.mark_dirty();
                }
                Tab::Runs => {
                    self.tui_state.runs_scroll.up();
                    self.tui_state.mark_dirty();
                }
                Tab::Timeline => {
                    self.tui_state.timeline_scroll.up();
                    self.tui_state.mark_dirty();
                }
                Tab::Approvals => {
                    self.tui_state.approvals_scroll.up();
                    self.tui_state.mark_dirty();
                }
                Tab::Memory => {
                    self.tui_state.memory_scroll.up();
                    self.tui_state.mark_dirty();
                }
                Tab::Audit => {
                    self.tui_state.audit_scroll.up();
                    self.tui_state.mark_dirty();
                }
                _ => {}
            },

            TuiAction::NavigateDown => {
                let visible = Self::visible_rows();
                match self.active_tab {
                    Tab::Agents => {
                        let total = self.tui_state.agents.len();
                        self.tui_state.agents_scroll.down(total, visible);
                        self.tui_state.mark_dirty();
                    }
                    Tab::Runs => {
                        let total = self.tui_state.runs.len();
                        self.tui_state.runs_scroll.down(total, visible);
                        self.tui_state.mark_dirty();
                    }
                    Tab::Timeline => {
                        let total = self.tui_state.run_events.len();
                        self.tui_state.timeline_scroll.down(total, visible);
                        self.tui_state.mark_dirty();
                    }
                    Tab::Approvals => {
                        let total = self.tui_state.pending_approvals.len();
                        self.tui_state.approvals_scroll.down(total, visible);
                        self.tui_state.mark_dirty();
                    }
                    Tab::Memory => {
                        let total = self.tui_state.memory_entries.len();
                        self.tui_state.memory_scroll.down(total, visible);
                        self.tui_state.mark_dirty();
                    }
                    Tab::Audit => {
                        let total = self.tui_state.audit_log.len();
                        self.tui_state.audit_scroll.down(total, visible);
                        self.tui_state.mark_dirty();
                    }
                    _ => {}
                }
            }

            TuiAction::ScrollUp(n) => {
                for _ in 0..n {
                    match self.active_tab {
                        Tab::Agents => self.tui_state.agents_scroll.up(),
                        Tab::Runs => self.tui_state.runs_scroll.up(),
                        Tab::Timeline => self.tui_state.timeline_scroll.up(),
                        Tab::Approvals => self.tui_state.approvals_scroll.up(),
                        Tab::Memory => self.tui_state.memory_scroll.up(),
                        Tab::Audit => self.tui_state.audit_scroll.up(),
                        _ => {}
                    }
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::ScrollDown(n) => {
                let visible = Self::visible_rows();
                for _ in 0..n {
                    match self.active_tab {
                        Tab::Agents => {
                            let t = self.tui_state.agents.len();
                            self.tui_state.agents_scroll.down(t, visible);
                        }
                        Tab::Runs => {
                            let t = self.tui_state.runs.len();
                            self.tui_state.runs_scroll.down(t, visible);
                        }
                        Tab::Timeline => {
                            let t = self.tui_state.run_events.len();
                            self.tui_state.timeline_scroll.down(t, visible);
                        }
                        Tab::Approvals => {
                            let t = self.tui_state.pending_approvals.len();
                            self.tui_state.approvals_scroll.down(t, visible);
                        }
                        Tab::Memory => {
                            let t = self.tui_state.memory_entries.len();
                            self.tui_state.memory_scroll.down(t, visible);
                        }
                        Tab::Audit => {
                            let t = self.tui_state.audit_log.len();
                            self.tui_state.audit_scroll.down(t, visible);
                        }
                        _ => {}
                    }
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::Select => {
                match self.active_tab {
                    Tab::Agents => {
                        if self.tui_state.agents_scroll.selected.is_none() {
                            self.tui_state.agents_scroll.selected = Some(0);
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Runs => {
                        // Select a run and drill into run detail.
                        let sel = self.tui_state.runs_scroll.selected.unwrap_or(0);
                        self.tui_state.runs_scroll.selected = Some(sel);
                        if let Some(run) = self.tui_state.runs.get(sel) {
                            self.tui_state.selected_run = Some(run.id.clone());
                            self.refresh_run_detail();
                            self.refresh_run_events();
                            self.active_tab = Tab::RunDetail;
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Timeline => {
                        // Ensure an event is selected for the detail panel.
                        if self.tui_state.timeline_scroll.selected.is_none()
                            && !self.tui_state.run_events.is_empty()
                        {
                            self.tui_state.timeline_scroll.selected = Some(0);
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Approvals => {
                        // Ensure an approval item is selected for the detail panel.
                        if self.tui_state.approvals_scroll.selected.is_none()
                            && !self.tui_state.pending_approvals.is_empty()
                        {
                            self.tui_state.approvals_scroll.selected = Some(0);
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Memory => {
                        // Ensure a memory entry is selected for the detail panel.
                        if self.tui_state.memory_scroll.selected.is_none()
                            && !self.tui_state.memory_entries.is_empty()
                        {
                            self.tui_state.memory_scroll.selected = Some(0);
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Audit => {
                        // Ensure an audit event is selected.
                        if self.tui_state.audit_scroll.selected.is_none()
                            && !self.tui_state.audit_log.is_empty()
                        {
                            self.tui_state.audit_scroll.selected = Some(0);
                        }
                        self.tui_state.mark_dirty();
                    }
                    _ => {}
                }
            }

            TuiAction::Back => {
                if self.input_mode == InputMode::Prompt {
                    let approval_context = self.tui_state.console_approval_context();
                    if self
                        .tui_state
                        .interaction
                        .dismiss_slash_completion_with_approval_context(approval_context.as_ref())
                    {
                        self.tui_state.mark_dirty();
                        return;
                    }
                    self.input_mode = InputMode::Normal;
                    self.tui_state.interaction.clear_prompt();
                    self.tui_state.mark_dirty();
                    return;
                }
                match self.active_tab {
                    Tab::Agents => {
                        self.tui_state.agents_scroll.selected = None;
                        self.tui_state.mark_dirty();
                    }
                    Tab::Runs => {
                        self.tui_state.runs_scroll.selected = None;
                        self.tui_state.mark_dirty();
                    }
                    Tab::RunDetail => {
                        // Go back to runs list.
                        self.active_tab = Tab::Runs;
                        self.tui_state.mark_dirty();
                    }
                    Tab::Timeline => {
                        if self.tui_state.timeline_scroll.selected.is_some() {
                            self.tui_state.timeline_scroll.selected = None;
                        } else {
                            self.active_tab = Tab::Dashboard;
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Approvals => {
                        // Cancel any open confirmation dialog first.
                        if !matches!(
                            self.tui_state.confirm_dialog,
                            crate::tui::state::ConfirmDialog::None
                        ) {
                            self.tui_state.confirm_dialog = crate::tui::state::ConfirmDialog::None;
                        } else if self.tui_state.approvals_scroll.selected.is_some() {
                            self.tui_state.approvals_scroll.selected = None;
                        } else {
                            self.active_tab = Tab::Dashboard;
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Memory => {
                        if self.input_mode == InputMode::Insert {
                            self.input_mode = InputMode::Normal;
                            self.tui_state.memory_search_query.clear();
                            self.refresh_memory();
                        } else if self.tui_state.memory_scroll.selected.is_some() {
                            self.tui_state.memory_scroll.selected = None;
                        } else {
                            self.active_tab = Tab::Dashboard;
                        }
                        self.tui_state.mark_dirty();
                    }
                    Tab::Audit => {
                        if self.tui_state.audit_scroll.selected.is_some() {
                            self.tui_state.audit_scroll.selected = None;
                        } else {
                            self.active_tab = Tab::Dashboard;
                        }
                        self.tui_state.mark_dirty();
                    }
                    _ => self.active_tab = Tab::Dashboard,
                }
            }

            TuiAction::SelectRun => {
                // Same as Select when on the Runs tab.
                if self.active_tab == Tab::Runs {
                    self.apply_action(TuiAction::Select);
                }
            }

            TuiAction::ViewTimeline => {
                self.active_tab = Tab::Timeline;
                self.refresh_run_events();
                self.tui_state.mark_dirty();
            }

            TuiAction::ViewApprovals => {
                self.active_tab = Tab::Approvals;
                self.refresh_approvals();
                self.tui_state.mark_dirty();
            }

            TuiAction::ApproveEffect => {
                use crate::tui::state::ConfirmDialog;
                if self.active_tab == Tab::Approvals {
                    match &self.tui_state.confirm_dialog {
                        ConfirmDialog::ConfirmApprove(target) => {
                            // User confirmed — execute approval.
                            let target = target.clone();
                            self.execute_approval(&target, ApprovalDecision::Approve);
                            self.tui_state.confirm_dialog = ConfirmDialog::None;
                        }
                        ConfirmDialog::None => {
                            // First press — show confirmation dialog.
                            if let Some(sel) = self.tui_state.approvals_scroll.selected {
                                if let Some(item) = self.tui_state.pending_approvals.get(sel) {
                                    self.tui_state.confirm_dialog =
                                        ConfirmDialog::ConfirmApprove(ApprovalActionTarget {
                                            conversation_id: item.conversation_id.clone(),
                                            approval_id: item.approval_id.clone(),
                                        });
                                }
                            }
                        }
                        ConfirmDialog::ConfirmDeny(_) => {
                            // A deny dialog is open; cancel it, then set approve.
                            self.tui_state.confirm_dialog = ConfirmDialog::None;
                        }
                    }
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::DenyEffect => {
                use crate::tui::state::ConfirmDialog;
                if self.active_tab == Tab::Approvals {
                    match &self.tui_state.confirm_dialog {
                        ConfirmDialog::ConfirmDeny(target) => {
                            // User confirmed denial.
                            let target = target.clone();
                            self.execute_approval(&target, ApprovalDecision::Deny { reason: None });
                            self.tui_state.confirm_dialog = ConfirmDialog::None;
                        }
                        ConfirmDialog::None => {
                            // First press — show confirmation dialog.
                            if let Some(sel) = self.tui_state.approvals_scroll.selected {
                                if let Some(item) = self.tui_state.pending_approvals.get(sel) {
                                    self.tui_state.confirm_dialog =
                                        ConfirmDialog::ConfirmDeny(ApprovalActionTarget {
                                            conversation_id: item.conversation_id.clone(),
                                            approval_id: item.approval_id.clone(),
                                        });
                                }
                            }
                        }
                        ConfirmDialog::ConfirmApprove(_) => {
                            // An approve dialog is open; cancel it, then set deny.
                            self.tui_state.confirm_dialog = ConfirmDialog::None;
                        }
                    }
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::ScrollToBottom => {
                let visible = Self::visible_rows();
                match self.active_tab {
                    Tab::Audit => {
                        let total = self.tui_state.audit_log.len();
                        if total > 0 {
                            self.tui_state.audit_scroll.selected = Some(total - 1);
                            self.tui_state.audit_scroll.offset = total.saturating_sub(visible);
                        }
                    }
                    Tab::Timeline => {
                        let total = self.tui_state.run_events.len();
                        if total > 0 {
                            self.tui_state.timeline_scroll.selected = Some(total - 1);
                            self.tui_state.timeline_scroll.offset = total.saturating_sub(visible);
                        }
                    }
                    _ => {}
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::ScrollToTop => {
                match self.active_tab {
                    Tab::Audit => {
                        self.tui_state.audit_scroll.selected = Some(0);
                        self.tui_state.audit_scroll.offset = 0;
                    }
                    Tab::Timeline => {
                        self.tui_state.timeline_scroll.selected = Some(0);
                        self.tui_state.timeline_scroll.offset = 0;
                    }
                    _ => {}
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::DeleteEntry => {
                if self.active_tab == Tab::Memory {
                    if let Some(sel) = self.tui_state.memory_scroll.selected {
                        if sel < self.tui_state.memory_entries.len() {
                            let entry_id = self.tui_state.memory_entries[sel].id.clone();
                            self.execute_delete_memory(&entry_id);
                            // Adjust selection after deletion.
                            let new_len = self.tui_state.memory_entries.len();
                            if new_len == 0 {
                                self.tui_state.memory_scroll.selected = None;
                            } else {
                                let new_sel = sel.min(new_len - 1);
                                self.tui_state.memory_scroll.selected = Some(new_sel);
                            }
                        }
                    }
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::CycleFilter => {
                if self.active_tab == Tab::Audit {
                    use crate::tui::views::audit::AuditFilter;
                    // Cycle through: None -> BySeverity("warn") -> BySeverity("error") -> None.
                    self.tui_state.audit_filter = match &self.tui_state.audit_filter {
                        AuditFilter::None => AuditFilter::BySeverity("warn".to_owned()),
                        AuditFilter::BySeverity(s) if s == "warn" => {
                            AuditFilter::BySeverity("error".to_owned())
                        }
                        _ => AuditFilter::None,
                    };
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::Search => {
                // Activate the memory search bar. Switch to the Memory tab
                // if not already there, then enter Insert mode so subsequent
                // keystrokes are captured as search input.
                if self.active_tab != Tab::Memory {
                    self.active_tab = Tab::Memory;
                    self.refresh_memory();
                }
                self.input_mode = InputMode::Insert;
                self.tui_state.mark_dirty();
            }

            TuiAction::SearchInput(c) => {
                self.tui_state.memory_search_query.push(c);
                self.refresh_memory();
                self.tui_state.memory_scroll.offset = 0;
                self.tui_state.memory_scroll.selected = None;
                self.tui_state.mark_dirty();
            }

            TuiAction::SearchBackspace => {
                self.tui_state.memory_search_query.pop();
                self.refresh_memory();
                self.tui_state.memory_scroll.offset = 0;
                self.tui_state.memory_scroll.selected = None;
                self.tui_state.mark_dirty();
            }

            TuiAction::SearchSubmit => {
                self.input_mode = InputMode::Normal;
                self.tui_state.mark_dirty();
            }

            TuiAction::TogglePanel => {
                if self.active_tab == Tab::RunDetail {
                    self.tui_state.detail_panel_index = (self.tui_state.detail_panel_index + 1) % 2;
                    self.tui_state.mark_dirty();
                }
            }

            TuiAction::OpenPrompt => {
                if !self.ensure_console_agent() {
                    self.tui_state.mark_dirty();
                    return;
                }
                if self.run_controller.is_control_active() {
                    self.tui_state.last_error =
                        Some("a Console command is still running".to_owned());
                } else {
                    self.active_tab = Tab::Console;
                    self.input_mode = InputMode::Prompt;
                    self.tui_state.last_error = None;
                    if self.run_controller.approval_authority_bound()
                        && self.tui_state.interaction.conversation_id.is_some()
                    {
                        self.refresh_approvals();
                    }
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::OpenSessionPicker => {
                if self.active_tab != Tab::Console {
                    return;
                }
                if self.run_controller.is_control_active() {
                    self.tui_state.last_error = Some(
                        "wait for the active Console action before switching sessions".to_owned(),
                    );
                } else if self.ensure_console_agent() {
                    match self.tui_state.interaction.begin_session_picker() {
                        Ok(request) => {
                            let request_id = request.request_id.clone();
                            self.input_mode = InputMode::SessionPicker;
                            self.tui_state.last_error = None;
                            if let Err(error) = self.run_controller.list_sessions(request) {
                                self.tui_state
                                    .interaction
                                    .fail_session_picker_request(&request_id, error);
                            }
                        }
                        Err(error) => self.tui_state.last_error = Some(error.to_owned()),
                    }
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::SessionPickerUp => {
                self.tui_state.interaction.session_picker_up();
                self.tui_state.mark_dirty();
            }

            TuiAction::SessionPickerDown => {
                self.tui_state.interaction.session_picker_down();
                self.tui_state.mark_dirty();
            }

            TuiAction::SessionPickerConfirm => {
                if self.run_controller.is_control_active() {
                    if let Some(request_id) = self
                        .tui_state
                        .interaction
                        .session_picker
                        .as_ref()
                        .map(|picker| picker.request_id.clone())
                    {
                        self.tui_state.interaction.fail_session_picker_request(
                            &request_id,
                            "wait for the active Console action before selecting a session",
                        );
                    }
                } else if let Ok(request) = self.tui_state.interaction.begin_session_selection() {
                    let request_id = request.request_id.clone();
                    if let Err(error) = self.run_controller.load_session(request) {
                        self.tui_state
                            .interaction
                            .fail_session_picker_request(&request_id, error);
                    }
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::SessionPickerClose => {
                self.tui_state.interaction.close_session_picker();
                self.input_mode = InputMode::Normal;
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptInput(c) => {
                if !self.tui_state.interaction.push_char(c) {
                    self.tui_state.last_error = Some(format!(
                        "Console composer is limited to {} KiB",
                        crate::tui::interaction::MAX_COMPOSER_BYTES / 1024
                    ));
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptPaste(pasted) => {
                let outcome = self.tui_state.interaction.insert_paste(&pasted);
                self.tui_state.last_error = outcome.truncated.then(|| {
                    format!(
                        "paste truncated at the {} KiB Console composer limit",
                        crate::tui::interaction::MAX_COMPOSER_BYTES / 1024
                    )
                });
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptNewline => {
                if !self.tui_state.interaction.insert_newline() {
                    self.tui_state.last_error = Some(format!(
                        "Console composer is limited to {} KiB",
                        crate::tui::interaction::MAX_COMPOSER_BYTES / 1024
                    ));
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptBackspace => {
                self.tui_state.interaction.backspace();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptDelete => {
                self.tui_state.interaction.delete();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptMoveLeft => {
                self.tui_state.interaction.move_left();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptMoveRight => {
                self.tui_state.interaction.move_right();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptMoveUp => {
                let approval_context = self.tui_state.console_approval_context();
                self.tui_state
                    .interaction
                    .move_up_with_approval_context(approval_context.as_ref());
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptMoveDown => {
                let approval_context = self.tui_state.console_approval_context();
                self.tui_state
                    .interaction
                    .move_down_with_approval_context(approval_context.as_ref());
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptMoveHome => {
                self.tui_state.interaction.move_home();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptMoveEnd => {
                self.tui_state.interaction.move_end();
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptAcceptCompletion => {
                let approval_context = self.tui_state.console_approval_context();
                self.tui_state
                    .interaction
                    .accept_slash_completion_with_approval_context(approval_context.as_ref());
                self.tui_state.mark_dirty();
            }

            TuiAction::PromptSubmit => {
                if self.tui_state.interaction.is_command_input() {
                    let approval_context = self.tui_state.console_approval_context();
                    match self
                        .tui_state
                        .interaction
                        .submit_command_with_approval_context(approval_context.as_ref())
                    {
                        Ok(ConsoleCommandSubmission::Execute(request)) => {
                            self.input_mode = InputMode::Normal;
                            if let Err(error) = self.run_controller.execute_command(request) {
                                self.tui_state.interaction.fail_pending_command(error);
                                self.tui_state.last_error = Some(error.to_owned());
                            } else {
                                self.tui_state.last_error = None;
                            }
                        }
                        Ok(ConsoleCommandSubmission::Rejected) => {
                            self.input_mode = InputMode::Normal;
                            self.tui_state.last_error = None;
                        }
                        Err(error) => self.tui_state.last_error = Some(error.to_owned()),
                    }
                } else {
                    match self.tui_state.interaction.submit() {
                        Ok(request) => {
                            self.input_mode = InputMode::Normal;
                            match self.run_controller.start(request) {
                                Ok(activity_id) => {
                                    if let Err(error) = self
                                        .tui_state
                                        .interaction
                                        .bind_activity(activity_id.clone())
                                    {
                                        let _ = self.run_controller.cancel_activity(&activity_id);
                                        self.tui_state
                                            .interaction
                                            .apply(ControllerEvent::Failed(error.to_owned()));
                                        self.tui_state.last_error = Some(error.to_owned());
                                    } else {
                                        self.tui_state.last_error = None;
                                    }
                                }
                                Err(error) => {
                                    self.tui_state
                                        .interaction
                                        .apply(ControllerEvent::Failed(error.to_owned()));
                                    self.tui_state.last_error = Some(error.to_owned());
                                }
                            }
                        }
                        Err(error) => self.tui_state.last_error = Some(error.to_owned()),
                    }
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::CancelActiveRun => {
                let selected = self
                    .tui_state
                    .interaction
                    .selected_activity_id()
                    .map(str::to_owned);
                if selected
                    .as_deref()
                    .is_some_and(|activity_id| self.run_controller.cancel_activity(activity_id))
                {
                    self.tui_state.interaction.mark_cancelling();
                    self.tui_state.last_error = None;
                } else {
                    self.tui_state.last_error = Some("no console run is active".to_owned());
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::Quit => {
                self.running = false;
            }

            TuiAction::SelectPreviousActivity => {
                if self.tui_state.interaction.select_activity_relative(-1) {
                    self.active_tab = Tab::Console;
                    self.input_mode = InputMode::Normal;
                    self.tui_state.last_error = None;
                } else {
                    self.tui_state.last_error = Some("no Console activity is retained".to_owned());
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::SelectNextActivity => {
                if self.tui_state.interaction.select_activity_relative(1) {
                    self.active_tab = Tab::Console;
                    self.input_mode = InputMode::Normal;
                    self.tui_state.last_error = None;
                } else {
                    self.tui_state.last_error = Some("no Console activity is retained".to_owned());
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::Refresh => {
                self.request_data_refresh();
                if self.active_tab == Tab::Approvals {
                    self.refresh_approvals();
                }
                self.tui_state.mark_dirty();
            }

            TuiAction::Resize(_w, _h) => {
                self.tui_state.mark_dirty();
            }
        }
    }

    /// Select the highlighted active agent, retain the console target when it
    /// is still active, or fall back to the first active agent.
    fn ensure_console_agent(&mut self) -> bool {
        let current = || {
            let current = self.tui_state.interaction.agent_id.as_deref();
            self.tui_state
                .agents
                .iter()
                .find(|agent| Some(agent.id.as_str()) == current && agent.state == "active")
        };
        let highlighted = || {
            self.tui_state
                .agents_scroll
                .selected
                .and_then(|index| self.tui_state.agents.get(index))
                .filter(|agent| agent.state == "active")
        };
        let selected = if self.active_tab == Tab::Agents {
            highlighted().or_else(current)
        } else {
            current().or_else(highlighted)
        }
        .or_else(|| {
            self.tui_state
                .agents
                .iter()
                .find(|agent| agent.state == "active")
        })
        .map(|agent| (agent.id.clone(), agent.name.clone()));

        if let Some((id, name)) = selected {
            let changed = self.tui_state.interaction.agent_id.as_deref() != Some(id.as_str());
            if changed {
                self.tui_state
                    .interaction
                    .select_agent(id.clone(), name.clone());
                if let Err(error) = self.run_controller.load_history(id) {
                    self.tui_state.last_error = Some(error.to_owned());
                }
            }
            true
        } else {
            self.tui_state.last_error =
                Some("no active agent is available; activate an agent before prompting".to_owned());
            false
        }
    }

    /// Apply one background completion on the terminal thread and immediately
    /// refresh durable run projections when lifecycle state changes.
    fn apply_controller_event(&mut self, update: ControllerUpdate) {
        let event = update.event();
        self.apply_approval_controller_event(event);
        if let ControllerEvent::HistoryFailed { reason, .. } = event {
            self.tui_state.last_error = Some(reason.clone());
        }
        let refresh = matches!(
            event,
            ControllerEvent::Started { .. }
                | ControllerEvent::Completed { .. }
                | ControllerEvent::Failed(_)
                | ControllerEvent::Cancelled(_)
                | ControllerEvent::TimedOut
        );
        let selected_activity = update.activity_id().is_some_and(|activity_id| {
            self.tui_state.interaction.selected_activity_id() == Some(activity_id)
        });
        let refresh_scoped_approvals = selected_activity
            && matches!(
                event,
                ControllerEvent::TurnApprovalRequested { .. }
                    | ControllerEvent::TurnApprovalResolved { .. }
            );
        let refresh_resolved_command_approval = matches!(
            event,
            ControllerEvent::CommandCompleted {
                conversation_id: Some(conversation_id),
                result,
                ..
            } if result.approval_resolution.is_some()
                && self.tui_state.interaction.conversation_id.as_deref()
                    == Some(conversation_id.as_str())
        );
        if selected_activity {
            if let ControllerEvent::Started { run_id, .. } = event {
                self.tui_state.selected_run = Some(run_id.clone());
            }
        }
        let selector_event = matches!(
            event,
            ControllerEvent::SessionListLoaded { .. }
                | ControllerEvent::SessionListFailed { .. }
                | ControllerEvent::SessionSelected { .. }
                | ControllerEvent::SessionSelectionFailed { .. }
        );
        self.tui_state.interaction.apply_update(update);
        if (refresh_scoped_approvals || refresh_resolved_command_approval)
            && self.run_controller.approval_authority_bound()
            && !self.run_controller.is_control_active()
        {
            self.refresh_approvals();
        }
        if selector_event
            && self.input_mode == InputMode::SessionPicker
            && self.tui_state.interaction.session_picker.is_none()
        {
            self.input_mode = InputMode::Normal;
        }
        self.tui_state.mark_dirty();
        if refresh {
            self.request_data_refresh();
        }
    }

    fn apply_approval_controller_event(&mut self, event: &ControllerEvent) {
        match event {
            ControllerEvent::ApprovalListLoaded {
                request_id,
                conversation_id,
                approvals,
                total_count,
            } => {
                if !self.approval_event_is_current(*request_id, *conversation_id) {
                    return;
                }
                let conversation = conversation_id.to_string();
                self.tui_state
                    .replace_scoped_approvals(&conversation, approvals.clone());
                self.tui_state.approval_queue.status = ApprovalQueueStatus::Ready;
                self.tui_state.approval_queue.message = (*total_count > approvals.len()).then(|| {
                    format!(
                        "Showing the first {} of {total_count} pending approvals; resolve visible requests, then refresh to reveal the remainder.",
                        approvals.len()
                    )
                });
                self.tui_state.last_error = None;
            }
            ControllerEvent::ApprovalListFailed {
                request_id,
                conversation_id,
                code,
                reason,
            } => {
                if self.approval_event_is_current(*request_id, *conversation_id) {
                    self.tui_state.pending_approvals.clear();
                    self.tui_state.approvals_scroll = ScrollState::default();
                    self.set_approval_failure(*code, reason);
                }
            }
            ControllerEvent::ApprovalDecisionCompleted {
                request_id,
                conversation_id,
                approval_id,
                decision,
                approval,
            } => {
                if !self.approval_event_is_current(*request_id, *conversation_id) {
                    return;
                }
                if approval.approval_id != *approval_id {
                    self.fail_approval_surface(
                        "approval service returned a mismatched durable approval identity",
                    );
                    return;
                }
                self.tui_state.pending_approvals.retain(|item| {
                    item.conversation_id != conversation_id.to_string()
                        || item.approval_id != approval_id.to_string()
                });
                let len = self.tui_state.pending_approvals.len();
                self.tui_state.approvals_scroll.selected = (len > 0).then(|| {
                    self.tui_state
                        .approvals_scroll
                        .selected
                        .unwrap_or(0)
                        .min(len - 1)
                });
                self.tui_state.approval_queue.status = ApprovalQueueStatus::Ready;
                let verb = match decision {
                    ApprovalDecision::Approve => "Approved",
                    ApprovalDecision::Deny { .. } => "Denied",
                };
                self.tui_state.approval_queue.message = Some(format!(
                    "{verb} approval {}.",
                    short_id(&approval_id.to_string())
                ));
                self.tui_state.last_error = None;
            }
            ControllerEvent::ApprovalDecisionFailed {
                request_id,
                conversation_id,
                code,
                reason,
                ..
            } if self.approval_event_is_current(*request_id, *conversation_id) => {
                self.set_approval_failure(*code, reason);
            }
            _ => {}
        }
    }

    fn approval_event_is_current(
        &self,
        request_id: uuid::Uuid,
        conversation_id: polkagent_core::ConversationId,
    ) -> bool {
        self.tui_state.approval_queue.is_current(
            &request_id.to_string(),
            &conversation_id.to_string(),
            self.tui_state.interaction.conversation_id.as_deref(),
        )
    }

    fn set_approval_failure(&mut self, code: InteractionErrorCode, reason: &str) {
        let unavailable = matches!(
            code,
            InteractionErrorCode::Unavailable | InteractionErrorCode::Unsupported
        );
        self.tui_state.approval_queue.status = if unavailable {
            ApprovalQueueStatus::Unavailable
        } else {
            ApprovalQueueStatus::Failed
        };
        let guidance = if unavailable {
            " Approval authority is not composed for this process; configure an authenticated durable approval surface and restart."
        } else {
            " Refresh and verify the selected durable conversation before retrying."
        };
        self.tui_state.approval_queue.message = Some(format!("{code}: {reason}.{guidance}"));
        self.tui_state.last_error = Some(format!("approvals: {code}: {reason}"));
    }

    fn fail_approval_surface(&mut self, reason: &str) {
        self.tui_state.approval_queue.status = ApprovalQueueStatus::Failed;
        self.tui_state.approval_queue.message = Some(reason.to_owned());
        self.tui_state.last_error = Some(format!("approvals: {reason}"));
    }

    // ── Data refresh ────────────────────────────────────────────────────────

    /// Queue the newest monitoring projection. Repeated requests while one is
    /// running collapse to one latest-generation follow-up.
    fn request_data_refresh(&mut self) {
        self.background_jobs
            .request_refresh(self.tui_state.selected_run.clone());
        self.last_refresh = Instant::now();
    }

    fn apply_background_result(&mut self, result: BackgroundResult) {
        let Some(update) = self.background_jobs.complete(result) else {
            return;
        };
        match update {
            ProjectionUpdate::Refresh(snapshot) => self.apply_projection_snapshot(*snapshot),
            ProjectionUpdate::Chain(result) => match result {
                Ok(status) => {
                    self.tui_state.chain_connected = true;
                    self.tui_state.chain_name = status.chain_name;
                    self.tui_state.node_version = status.node_version;
                    self.tui_state.best_block = status.best_block;
                    self.tui_state.finalized_block = status.finalized_block;
                    if self
                        .tui_state
                        .last_error
                        .as_deref()
                        .is_some_and(|error| error.starts_with("chain: "))
                    {
                        self.tui_state.last_error = None;
                    }
                }
                Err(error) => {
                    self.tui_state.chain_connected = false;
                    self.tui_state.last_error = Some(format!("chain: {error}"));
                }
            },
            ProjectionUpdate::Failure(error) => {
                let error = polkagent_telemetry::redact_string(&error);
                self.tui_state.last_error = Some(error);
            }
        }
        self.tui_state.mark_dirty();
    }

    fn apply_projection_snapshot(&mut self, snapshot: crate::tui::db::TuiProjectionSnapshot) {
        if let Some(agents) = snapshot.agents {
            self.tui_state.agents = agents;
        }
        if let Some(runs) = snapshot.runs {
            self.tui_state.runs = runs;
        }
        if let Some(health) = snapshot.health {
            self.tui_state.health = health;
        }
        if snapshot.selected_run == self.tui_state.selected_run {
            if let crate::tui::db::ProjectionValue::Value(detail) = snapshot.run_detail {
                self.tui_state.run_detail = detail;
            }
            if let Some(events) = snapshot.run_events {
                self.tui_state.run_events = events;
            }
        }
        if let Some(error_count) = snapshot.error_count {
            self.tui_state.error_count = error_count;
        }
        if let Some(budget_remaining) = snapshot.budget_remaining {
            self.tui_state.budget_remaining = budget_remaining;
        }
        self.tui_state.last_error = snapshot.error;
        self.tui_state.last_refresh = Some(snapshot.sampled_at);
        self.tui_state.recompute_widget_data();
        if self.active_tab == Tab::Console && self.tui_state.interaction.agent_id.is_none() {
            self.ensure_console_agent();
        }
    }

    /// Refresh only the run detail for the currently selected run.
    fn refresh_run_detail(&mut self) {
        use crate::tui::db::TuiDb;

        let Some(run_id) = &self.tui_state.selected_run.clone() else {
            return;
        };
        if let Ok(db) = TuiDb::from_pool(&self.pool) {
            match db.run_detail(run_id) {
                Ok(detail) => self.tui_state.run_detail = detail,
                Err(e) => {
                    self.tui_state.last_error = Some(format!("run_detail: {e}"));
                }
            }
        }
    }

    /// Refresh only the events for the currently selected run.
    fn refresh_run_events(&mut self) {
        use crate::tui::db::TuiDb;

        let Some(run_id) = &self.tui_state.selected_run.clone() else {
            return;
        };
        if let Ok(db) = TuiDb::from_pool(&self.pool) {
            match db.run_events(run_id, 500) {
                Ok(events) => self.tui_state.run_events = events,
                Err(e) => {
                    self.tui_state.last_error = Some(format!("run_events: {e}"));
                }
            }
        }
    }

    /// Refresh only the approval queue.
    fn refresh_approvals(&mut self) {
        let Some(conversation) = self.tui_state.interaction.conversation_id.clone() else {
            self.tui_state.pending_approvals.clear();
            self.tui_state.approvals_scroll = ScrollState::default();
            self.tui_state.approval_queue.conversation_id = None;
            self.tui_state.approval_queue.request_id = None;
            self.tui_state.approval_queue.status = ApprovalQueueStatus::Unscoped;
            self.tui_state.approval_queue.message = Some(
                "Select, create, or resume a durable Console conversation in F9 first.".to_owned(),
            );
            return;
        };
        let Ok(conversation_id) = conversation.parse() else {
            self.fail_approval_surface("the selected Console conversation ID is invalid");
            return;
        };
        let request_id = uuid::Uuid::now_v7();
        if self.tui_state.approval_queue.conversation_id.as_deref() != Some(&conversation) {
            self.tui_state.pending_approvals.clear();
            self.tui_state.approvals_scroll = ScrollState::default();
        }
        self.tui_state.approval_queue.conversation_id = Some(conversation);
        self.tui_state.approval_queue.request_id = Some(request_id.to_string());
        self.tui_state.approval_queue.status = ApprovalQueueStatus::Loading;
        self.tui_state.approval_queue.message = None;
        if let Err(error) = self.run_controller.list_approvals(ApprovalListRequest {
            request_id,
            conversation_id,
        }) {
            self.fail_approval_surface(error);
        }
    }

    /// Refresh the memory browser entries.
    fn refresh_memory(&mut self) {
        use crate::tui::db::TuiDb;

        if let Ok(_db) = TuiDb::from_pool(&self.pool) {
            let query = if self.tui_state.memory_search_query.is_empty() {
                None
            } else {
                Some(self.tui_state.memory_search_query.as_str())
            };
            match TuiDb::memory_entries(query, 200) {
                Ok(entries) => self.tui_state.memory_entries = entries,
                Err(e) => {
                    self.tui_state.last_error = Some(format!("memory: {e}"));
                }
            }
        }
    }

    /// Refresh the audit log.
    fn refresh_audit_log(&mut self) {
        use crate::tui::db::TuiDb;

        if let Ok(db) = TuiDb::from_pool(&self.pool) {
            match db.audit_events(500) {
                Ok(events) => self.tui_state.audit_log = events,
                Err(e) => {
                    self.tui_state.last_error = Some(format!("audit: {e}"));
                }
            }
        }
    }

    /// Resolve one exact, currently visible approval request through the
    /// authenticated shared service. A changed scope or stale row fails closed.
    fn execute_approval(&mut self, target: &ApprovalActionTarget, decision: ApprovalDecision) {
        if self.tui_state.interaction.conversation_id.as_deref()
            != Some(target.conversation_id.as_str())
            || !self.tui_state.pending_approvals.iter().any(|approval| {
                approval.conversation_id == target.conversation_id
                    && approval.approval_id == target.approval_id
                    && approval.status == "pending"
            })
        {
            self.fail_approval_surface(
                "the selected approval changed or is no longer pending; refresh before deciding",
            );
            return;
        }
        let Ok(conversation_id) = target.conversation_id.parse() else {
            self.fail_approval_surface("the approval conversation ID is invalid");
            return;
        };
        let Ok(approval_id) = target.approval_id.parse() else {
            self.fail_approval_surface("the durable approval ID is invalid");
            return;
        };
        let request_id = uuid::Uuid::now_v7();
        self.tui_state.approval_queue.request_id = Some(request_id.to_string());
        self.tui_state.approval_queue.status = ApprovalQueueStatus::Resolving;
        self.tui_state.approval_queue.message = Some(format!(
            "Resolving approval {}…",
            short_id(&target.approval_id)
        ));
        if let Err(error) = self
            .run_controller
            .decide_approval(ApprovalDecisionRequest {
                request_id,
                conversation_id,
                approval_id,
                decision,
            })
        {
            self.fail_approval_surface(error);
        }
    }

    /// Execute a delete action on a memory entry.
    fn execute_delete_memory(&mut self, entry_id: &str) {
        use crate::tui::db::TuiDb;

        if let Ok(_db) = TuiDb::from_pool(&self.pool) {
            match TuiDb::delete_memory_entry(entry_id) {
                Ok(()) => {
                    self.tui_state.memory_entries.retain(|m| m.id != entry_id);
                    self.tui_state.last_error = None;
                }
                Err(e) => {
                    self.tui_state.last_error = Some(format!("delete_memory: {e}"));
                }
            }
        }
    }

    // ── Layout helpers ──────────────────────────────────────────────────────

    /// Estimate the number of visible data rows in the main content area.
    ///
    /// Uses the current terminal height minus chrome (header, tab bar, status
    /// bar, block borders, table header).
    fn visible_rows() -> usize {
        let h = crossterm::terminal::size().map_or(24, |(_, h)| h) as usize;
        // 3 chrome rows (header + tab_bar + status_bar) + 2 border + 1 table header
        h.saturating_sub(6)
    }

    // ── Render pipeline ─────────────────────────────────────────────────────

    /// Render the full TUI frame.
    ///
    /// Called from `terminal.draw(|frame| self.render(frame))`. Zero I/O.
    fn render(&self, frame: &mut Frame) {
        let size = frame.area();

        // Fill the entire terminal with the void background.
        let bg = Block::default().style(Style::default().bg(self.theme.bg_void));
        frame.render_widget(bg, size);

        // Compute layout regions.
        let layout = compute_layout(size);

        // Chrome.
        header_bar::render(frame, layout.header, self.active_tab, &self.theme);
        status_bar::render(
            frame,
            layout.status,
            self.active_tab,
            self.input_mode,
            self.tui_state.last_error.as_deref(),
            &self.theme,
        );

        // Tab bar (F1–F9 indicators at the very bottom of the header area).
        self.render_tab_bar(frame, layout.tab_bar);

        // Active view.
        match self.active_tab {
            Tab::Dashboard => {
                views::dashboard::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::Agents => {
                views::agents::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::Runs => {
                views::runs::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::System => {
                views::system::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::RunDetail => {
                views::run_detail::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::Timeline => {
                views::timeline::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::Approvals => {
                views::approvals::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::Memory => {
                views::memory::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::Audit => {
                views::audit::render(frame, layout.main, &self.tui_state, &self.theme);
            }
            Tab::Console => {
                views::console::render(
                    frame,
                    layout.main,
                    &self.tui_state,
                    self.input_mode,
                    &self.theme,
                );
            }
        }
    }

    /// Render the horizontal tab indicator row.
    fn render_tab_bar(&self, frame: &mut Frame, area: Rect) {
        use ratatui::{
            style::Modifier,
            text::{Line, Span},
            widgets::Paragraph,
        };

        let tabs = [
            (Tab::Dashboard, "F1 Dashboard"),
            (Tab::Agents, "F2 Agents"),
            (Tab::Runs, "F3 Runs"),
            (Tab::System, "F4 System"),
            (Tab::Timeline, "F5 Timeline"),
            (Tab::Approvals, "F6 Approvals"),
            (Tab::Memory, "F7 Memory"),
            (Tab::Audit, "F8 Audit"),
            (Tab::Console, "F9 Console"),
        ];

        let mut spans = Vec::with_capacity(tabs.len() * 2);
        for (tab, label) in &tabs {
            let style = if *tab == self.active_tab {
                Style::default()
                    .fg(self.theme.rose_bright)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(self.theme.text_dim)
            };
            spans.push(Span::styled(format!("  {label}  "), style));
        }

        let bar_bg = Block::default().style(Style::default().bg(self.theme.bg_raised));
        frame.render_widget(bar_bg, area);
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}

/// Translate one already-read terminal event into an application action.
///
/// Keeping event acquisition outside this function ensures each successful
/// poll consumes exactly one event. In particular, resize events must not
/// trigger a second blocking read while the first event is discarded.
fn terminal_event_to_action(event: Event, input_mode: InputMode) -> Option<TuiAction> {
    match event {
        Event::Key(key) => key_to_action(key, input_mode),
        Event::Paste(pasted) if input_mode == InputMode::Prompt => {
            Some(TuiAction::PromptPaste(pasted))
        }
        Event::Resize(width, height) => Some(TuiAction::Resize(width, height)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Layout computation
// ---------------------------------------------------------------------------

/// Computed layout regions for the TUI frame.
struct TuiLayout {
    header: Rect,
    tab_bar: Rect,
    main: Rect,
    status: Rect,
}

fn compute_layout(area: Rect) -> TuiLayout {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header bar
            Constraint::Length(1), // tab bar
            Constraint::Min(5),    // main content
            Constraint::Length(1), // status bar
        ])
        .split(area);

    TuiLayout {
        header: rows[0],
        tab_bar: rows[1],
        main: rows[2],
        status: rows[3],
    }
}

// ---------------------------------------------------------------------------
// Terminal init / teardown
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreStep {
    RawMode,
    AlternateScreen,
    MouseCapture,
    BracketedPaste,
    Cursor,
}

impl RestoreStep {
    const ALL: [Self; 5] = [
        Self::RawMode,
        Self::AlternateScreen,
        Self::MouseCapture,
        Self::BracketedPaste,
        Self::Cursor,
    ];

    const fn mask(self) -> u8 {
        match self {
            Self::RawMode => 1 << 0,
            Self::AlternateScreen => 1 << 1,
            Self::MouseCapture => 1 << 2,
            Self::BracketedPaste => 1 << 3,
            Self::Cursor => 1 << 4,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::RawMode => "disable raw mode",
            Self::AlternateScreen => "leave alternate screen",
            Self::MouseCapture => "disable mouse capture",
            Self::BracketedPaste => "disable bracketed paste",
            Self::Cursor => "show cursor",
        }
    }
}

trait RestoreActions {
    fn restore(&mut self, step: RestoreStep) -> std::io::Result<()>;
}

struct CrosstermRestoreActions;

impl RestoreActions for CrosstermRestoreActions {
    fn restore(&mut self, step: RestoreStep) -> std::io::Result<()> {
        match step {
            RestoreStep::RawMode => crossterm::terminal::disable_raw_mode(),
            RestoreStep::AlternateScreen => {
                let mut stdout = std::io::stdout();
                execute!(stdout, LeaveAlternateScreen)
            }
            RestoreStep::MouseCapture => {
                let mut stdout = std::io::stdout();
                execute!(stdout, DisableMouseCapture)
            }
            RestoreStep::BracketedPaste => {
                let mut stdout = std::io::stdout();
                execute!(stdout, DisableBracketedPaste)
            }
            RestoreStep::Cursor => {
                let mut stdout = std::io::stdout();
                execute!(stdout, Show)
            }
        }
    }
}

struct RestoreProgress(u8);

impl RestoreProgress {
    const fn pending() -> Self {
        Self((1 << RestoreStep::ALL.len()) - 1)
    }

    const fn contains(&self, step: RestoreStep) -> bool {
        self.0 & step.mask() != 0
    }

    fn complete(&mut self, step: RestoreStep) {
        self.0 &= !step.mask();
    }
}

struct RestorationGuard<A: RestoreActions> {
    actions: A,
    pending: RestoreProgress,
}

impl<A: RestoreActions> RestorationGuard<A> {
    const fn new(actions: A) -> Self {
        Self {
            actions,
            pending: RestoreProgress::pending(),
        }
    }

    fn restore(&mut self) -> Result<()> {
        let mut errors = Vec::new();

        for step in RestoreStep::ALL {
            if !self.pending.contains(step) {
                continue;
            }
            match self.actions.restore(step) {
                Ok(()) => self.pending.complete(step),
                Err(error) => errors.push(format!("{}: {error}", step.label())),
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(anyhow!(
                "terminal restoration failed: {}",
                errors.join("; ")
            ))
        }
    }
}

impl<A: RestoreActions> Drop for RestorationGuard<A> {
    fn drop(&mut self) {
        // A best-effort retry covers setup failures, early returns, and
        // unwinding. Explicit teardown still reports its first-pass errors.
        drop(self.restore());
    }
}

/// A configured terminal paired with a drop-backed restoration guard.
///
/// The wrapper dereferences to ratatui's terminal so callers can render as
/// usual. If setup, the event loop, or explicit teardown returns an error (or
/// unwinds), the guard still attempts every terminal restoration action.
pub struct TuiTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restoration: RestorationGuard<CrosstermRestoreActions>,
}

impl Deref for TuiTerminal {
    type Target = Terminal<CrosstermBackend<Stdout>>;

    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

impl DerefMut for TuiTerminal {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.terminal
    }
}

fn short_id(value: &str) -> &str {
    &value[..value.len().min(8)]
}

/// Enter alternate screen mode and return a guarded, configured terminal.
pub fn enter_tui() -> Result<TuiTerminal> {
    let restoration = RestorationGuard::new(CrosstermRestoreActions);
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(TuiTerminal {
        terminal,
        restoration,
    })
}

/// Restore the terminal to its previous state, attempting every action.
pub fn exit_tui(terminal: &mut TuiTerminal) -> Result<()> {
    terminal.restoration.restore()
}

#[cfg(test)]
mod terminal_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use futures::stream;
    use polkagent_core::{AgentId, ApprovalId, PrincipalId};
    use polkagent_interaction::{
        ClientContext, CreateInteractionRequest, InteractionApprovalAuthority, InteractionConfig,
        InteractionService as _, InteractionTarget,
    };
    use polkagent_runtime::RuntimeFactory;
    use polkagent_store_sqlite::{migrations, SqliteRunStore};

    use super::*;

    fn empty_projection(generation: u64) -> crate::tui::db::TuiProjectionSnapshot {
        crate::tui::db::TuiProjectionSnapshot {
            selected_run: Some(format!("run-{generation}")),
            agents: None,
            runs: None,
            health: None,
            run_detail: crate::tui::db::ProjectionValue::Unchanged,
            run_events: None,
            error_count: None,
            budget_remaining: None,
            sampled_at: chrono::Utc::now(),
            error: Some(format!("generation-{generation}")),
        }
    }

    fn immediate_chain_worker() -> ChainWorker {
        Arc::new(|request| {
            Box::pin(async move {
                Ok(ChainJobOutput {
                    poller: request.poller,
                    result: None,
                })
            })
        })
    }

    #[tokio::test(flavor = "current_thread")]
    async fn successful_console_approval_refreshes_the_single_f6_queue() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let database_path = temp.path().join("app-approval-refresh.db");
        let pool = SqlitePool::open(&database_path).expect("open database");
        migrations::migrate(&pool.writer()).expect("migrate database");
        let store = SqliteRunStore::new(pool.clone());
        let agent = store
            .create_agent(
                "approval-agent",
                None,
                &serde_json::json!({
                    "name": "approval-agent",
                    "description": null,
                    "model": "fake/default",
                    "tools": [],
                    "system_prompt": null,
                    "autonomy_level": "supervised",
                })
                .to_string(),
            )
            .expect("create approval agent");
        let authority = InteractionApprovalAuthority {
            tenant_id: "tenant-a".to_owned(),
            workspace_id: "workspace-a".to_owned(),
            principal_id: PrincipalId::new(),
            surface: "tui".to_owned(),
        };
        let mut options = crate::tui::interaction::tui_runtime_options_with_approval(
            &pool,
            None,
            Some(authority),
        )
        .expect("TUI runtime options");
        options.workdir = temp.path().to_path_buf();
        options.disable_harness = true;
        options.discover_environment_providers = false;
        drop(store);
        drop(pool);
        let runtime = Box::pin(RuntimeFactory::build(options))
            .await
            .expect("build runtime");
        let interaction = runtime
            .interactions()
            .new_interaction(CreateInteractionRequest {
                title: Some("approval refresh".to_owned()),
                config: InteractionConfig::new(InteractionTarget::Agent(
                    agent.id.parse::<AgentId>().expect("agent UUID"),
                )),
                client_context: ClientContext::new(temp.path().to_path_buf())
                    .expect("client context"),
            })
            .await
            .expect("create interaction");
        let conversation_id = interaction.conversation_id;
        let approval_id = ApprovalId::new();
        let request_id = uuid::Uuid::now_v7().to_string();
        let mut app = App::new(Theme::dark(), runtime, Tab::Console);
        app.tui_state.interaction.select_agent(agent.id, agent.name);
        app.tui_state.interaction.conversation_id = Some(conversation_id.to_string());
        app.tui_state.interaction.command_result =
            Some(crate::tui::interaction::ConsoleCommandResult {
                request_id: request_id.clone(),
                line: format!("/approve {approval_id}"),
                status: crate::tui::interaction::ConsoleCommandStatus::Running,
                title: "Executing shared command".to_owned(),
                lines: Vec::new(),
                approval_resolution: None,
            });

        app.apply_controller_event(ControllerUpdate::Control(
            ControllerEvent::CommandCompleted {
                agent_id: app
                    .tui_state
                    .interaction
                    .agent_id
                    .clone()
                    .expect("selected agent"),
                conversation_id: Some(conversation_id.to_string()),
                request_id: request_id.clone(),
                result: crate::tui::interaction::ConsoleCommandResult {
                    request_id,
                    line: format!("/approve {approval_id}"),
                    status: crate::tui::interaction::ConsoleCommandStatus::Completed,
                    title: "Durable approval resolved".to_owned(),
                    lines: vec![format!("approval: {approval_id}")],
                    approval_resolution: Some(crate::tui::interaction::ConsoleApprovalResolution {
                        conversation_id,
                        approval_id,
                        decision: crate::tui::interaction::ConsoleApprovalDecision::Approve,
                    }),
                },
                selection: None,
                model_update: crate::tui::interaction::ConsoleModelUpdate::Unchanged,
                agent_update: None,
            },
        ));

        assert_eq!(
            app.tui_state.approval_queue.status,
            ApprovalQueueStatus::Loading
        );
        assert!(app.run_controller.is_control_active());
        let update = app
            .run_controller
            .recv_update()
            .await
            .expect("approval refresh completion");
        app.apply_controller_event(update);
        assert_eq!(
            app.tui_state.approval_queue.status,
            ApprovalQueueStatus::Ready
        );
        assert!(app.tui_state.pending_approvals.is_empty());
        assert_eq!(
            app.tui_state
                .interaction
                .command_result
                .as_ref()
                .and_then(|result| result.approval_resolution.as_ref())
                .map(|resolution| resolution.approval_id),
            Some(approval_id)
        );
        app.shutdown_workers().await.expect("shutdown app workers");
    }

    struct RecordingRestoreActions {
        calls: Arc<Mutex<Vec<RestoreStep>>>,
        failing_step: Option<RestoreStep>,
    }

    impl RestoreActions for RecordingRestoreActions {
        fn restore(&mut self, step: RestoreStep) -> std::io::Result<()> {
            self.calls.lock().unwrap().push(step);
            if self.failing_step == Some(step) {
                Err(std::io::Error::other("injected restore failure"))
            } else {
                Ok(())
            }
        }
    }

    fn recording_actions(
        failing_step: Option<RestoreStep>,
    ) -> (RecordingRestoreActions, Arc<Mutex<Vec<RestoreStep>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (
            RecordingRestoreActions {
                calls: Arc::clone(&calls),
                failing_step,
            },
            calls,
        )
    }

    fn fail_while_guarded(actions: RecordingRestoreActions) -> Result<()> {
        let _restoration = RestorationGuard::new(actions);
        Err(anyhow!("injected event-loop failure"))
    }

    #[test]
    fn restoration_guard_restores_on_normal_error() {
        let (actions, calls) = recording_actions(None);

        let result = fail_while_guarded(actions);

        assert!(result.is_err());
        assert_eq!(*calls.lock().unwrap(), RestoreStep::ALL);
    }

    #[test]
    fn restoration_guard_restores_while_unwinding_a_panic() {
        let (actions, calls) = recording_actions(None);

        let result = std::panic::catch_unwind(|| {
            let _restoration = RestorationGuard::new(actions);
            panic!("injected event-loop panic");
        });

        assert!(result.is_err());
        assert_eq!(*calls.lock().unwrap(), RestoreStep::ALL);
    }

    #[test]
    fn restoration_attempts_every_action_after_one_fails() {
        let (actions, calls) = recording_actions(Some(RestoreStep::RawMode));
        let mut restoration = RestorationGuard::new(actions);

        let error = restoration
            .restore()
            .expect_err("raw-mode failure should surface");

        assert!(error.to_string().contains("disable raw mode"));
        assert_eq!(*calls.lock().unwrap(), RestoreStep::ALL);
    }

    #[tokio::test]
    async fn headless_event_pump_bounds_input_without_dropping_actions() {
        let source = stream::iter([
            Ok(Event::Key(KeyEvent::new(
                KeyCode::Char('i'),
                KeyModifiers::NONE,
            ))),
            Ok(Event::Key(KeyEvent::new(
                KeyCode::Char('x'),
                KeyModifiers::NONE,
            ))),
        ])
        .chain(stream::pending());
        let mut pump = EventPump::start_with_input(source, Duration::from_secs(60), 1);

        tokio::task::yield_now().await;
        assert_eq!(pump.signal_rx.len(), 1, "input queue must stay bounded");
        let first = pump
            .next_event(std::future::pending())
            .await
            .expect("first input event");
        let second = pump
            .next_event(std::future::pending())
            .await
            .expect("second input event");

        assert!(matches!(
            first,
            EventLoopEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::Char('i'),
                ..
            }))
        ));
        assert!(matches!(
            second,
            EventLoopEvent::Terminal(Event::Key(KeyEvent {
                code: KeyCode::Char('x'),
                ..
            }))
        ));
        pump.shutdown().await.expect("stop bounded input pump");
        assert!(pump.tasks.is_empty());
    }

    #[tokio::test]
    async fn headless_event_pump_coalesces_resize_and_tick_bursts() {
        let source = stream::iter([
            Ok(Event::Resize(80, 24)),
            Ok(Event::Resize(100, 30)),
            Ok(Event::Resize(140, 50)),
        ])
        .chain(stream::pending());
        let mut pump = EventPump::start_with_input(source, Duration::from_millis(5), 1);

        tokio::task::yield_now().await;
        let resized = pump
            .next_event(std::future::pending())
            .await
            .expect("latest resize event");
        assert!(matches!(
            resized,
            EventLoopEvent::Terminal(Event::Resize(140, 50))
        ));

        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(pump.signal_rx.len(), 1, "ticks must coalesce at capacity");
        assert!(matches!(
            pump.next_event(std::future::pending())
                .await
                .expect("coalesced tick"),
            EventLoopEvent::Tick
        ));
        pump.shutdown().await.expect("stop coalescing pump");
    }

    #[tokio::test]
    async fn headless_event_pump_wakes_for_background_completion_without_polling() {
        let mut pump = EventPump::start_with_input(
            stream::pending(),
            Duration::from_secs(60),
            EVENT_CHANNEL_CAPACITY,
        );

        let event = pump
            .next_event(async {
                Some(BackgroundEvent::Controller(Box::new(
                    ControllerUpdate::Control(ControllerEvent::Progress("completed".to_owned())),
                )))
            })
            .await
            .expect("background completion event");
        assert!(matches!(
            event,
            EventLoopEvent::Background(event)
                if matches!(
                    *event,
                    BackgroundEvent::Controller(ref update)
                        if matches!(update.as_ref(), ControllerUpdate::Control(
                            ControllerEvent::Progress(detail)
                        ) if detail == "completed")
                )
        ));
        pump.shutdown().await.expect("stop background pump");
    }

    #[tokio::test]
    async fn headless_event_pump_has_no_busy_tick_or_orphan_producers() {
        let mut pump = EventPump::start_with_input(
            stream::pending(),
            Duration::from_secs(60),
            EVENT_CHANNEL_CAPACITY,
        );

        let pending = tokio::time::timeout(
            Duration::from_millis(20),
            pump.next_event(std::future::pending()),
        )
        .await;
        assert!(pending.is_err(), "event pump woke without an event");
        assert!(pump.tasks.iter().all(|task| !task.is_finished()));

        pump.shutdown().await.expect("stop idle event pump");
        assert!(pump.tasks.is_empty());
    }

    #[tokio::test]
    async fn headless_event_pump_surfaces_input_failure_and_stops_cleanly() {
        let source = stream::iter([Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "synthetic input failure",
        ))]);
        let mut pump = EventPump::start_with_input(source, Duration::from_secs(60), 1);

        let error = pump
            .next_event(std::future::pending())
            .await
            .expect_err("input failure must stop the loop");
        assert!(error.to_string().contains("synthetic input failure"));
        pump.shutdown().await.expect("stop failed input pump");
    }

    #[tokio::test]
    async fn blocked_projection_refresh_keeps_input_resize_and_controller_events_responsive() {
        let pool = SqlitePool::open_in_memory().expect("in-memory pool");
        let started = Arc::new(tokio::sync::Notify::new());
        let started_by_worker = Arc::clone(&started);
        let refresh_worker: RefreshWorker = Arc::new(move |_request| {
            let started = Arc::clone(&started_by_worker);
            Box::pin(async move {
                started.notify_one();
                std::future::pending().await
            })
        });
        let mut jobs = BackgroundJobs::with_workers(
            pool,
            crate::tui::db::ChainPoller::with_url(None),
            refresh_worker,
            immediate_chain_worker(),
        );
        jobs.request_refresh(None);
        started.notified().await;

        let source = stream::iter([
            Ok(Event::Key(KeyEvent::new(
                KeyCode::Char('x'),
                KeyModifiers::NONE,
            ))),
            Ok(Event::Resize(123, 45)),
            Ok(Event::Key(KeyEvent::new(
                KeyCode::Char('i'),
                KeyModifiers::NONE,
            ))),
        ])
        .chain(stream::pending());
        let mut pump = EventPump::start_with_input(source, Duration::from_secs(60), 1);
        let (saw_cancel, saw_resize, saw_prompt) =
            tokio::time::timeout(Duration::from_millis(100), async {
                let mut saw_cancel = false;
                let mut saw_resize = false;
                let mut saw_prompt = false;
                while !(saw_cancel && saw_resize && saw_prompt) {
                    let event = pump
                        .next_event(async { jobs.recv().await.map(BackgroundEvent::Projection) })
                        .await
                        .expect("terminal event");
                    match event {
                        EventLoopEvent::Terminal(Event::Resize(123, 45)) => saw_resize = true,
                        EventLoopEvent::Terminal(Event::Key(key))
                            if key.code == KeyCode::Char('x') =>
                        {
                            saw_cancel = matches!(
                                terminal_event_to_action(Event::Key(key), InputMode::Normal),
                                Some(TuiAction::CancelActiveRun)
                            );
                        }
                        EventLoopEvent::Terminal(Event::Key(key))
                            if key.code == KeyCode::Char('i') =>
                        {
                            saw_prompt = true;
                        }
                        _ => {}
                    }
                }
                (saw_cancel, saw_resize, saw_prompt)
            })
            .await
            .expect("terminal input stalled behind blocked refresh");
        assert!(saw_cancel && saw_resize && saw_prompt);

        let controller = tokio::time::timeout(
            Duration::from_millis(100),
            pump.next_event(async {
                Some(BackgroundEvent::Controller(Box::new(
                    ControllerUpdate::Control(ControllerEvent::Progress(
                        "still responsive".to_owned(),
                    )),
                )))
            }),
        )
        .await
        .expect("controller event stalled behind blocked refresh")
        .expect("controller event");
        assert!(matches!(
            controller,
            EventLoopEvent::Background(event)
                if matches!(
                    *event,
                    BackgroundEvent::Controller(ref update)
                        if matches!(update.as_ref(), ControllerUpdate::Control(
                            ControllerEvent::Progress(detail)
                        ) if detail == "still responsive")
                )
        ));

        let detached = tokio::time::timeout(Duration::from_millis(100), jobs.shutdown())
            .await
            .expect("blocked refresh worker was not reaped");
        assert_eq!(detached, 0, "async refresh task should abort cleanly");
        pump.shutdown().await.expect("stop responsive input pump");
        assert!(jobs.tasks.is_empty());
    }

    #[tokio::test]
    async fn refresh_jobs_coalesce_and_discard_stale_generation_results() {
        let pool = SqlitePool::open_in_memory().expect("in-memory pool");
        let first_started = Arc::new(tokio::sync::Notify::new());
        let first_started_by_worker = Arc::clone(&first_started);
        let first_release = Arc::new(tokio::sync::Notify::new());
        let first_release_by_worker = Arc::clone(&first_release);
        let active = Arc::new(AtomicUsize::new(0));
        let active_by_worker = Arc::clone(&active);
        let max_active = Arc::new(AtomicUsize::new(0));
        let max_active_by_worker = Arc::clone(&max_active);
        let refresh_worker: RefreshWorker = Arc::new(move |request| {
            let first_started = Arc::clone(&first_started_by_worker);
            let first_release = Arc::clone(&first_release_by_worker);
            let active = Arc::clone(&active_by_worker);
            let max_active = Arc::clone(&max_active_by_worker);
            Box::pin(async move {
                let now_active = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(now_active, Ordering::SeqCst);
                if request.generation == 1 {
                    first_started.notify_one();
                    first_release.notified().await;
                }
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(empty_projection(request.generation))
            })
        });
        let mut jobs = BackgroundJobs::with_workers(
            pool,
            crate::tui::db::ChainPoller::with_url(None),
            refresh_worker,
            immediate_chain_worker(),
        );

        jobs.request_refresh(Some("run-1".to_owned()));
        first_started.notified().await;
        for generation in 2..=10 {
            jobs.request_refresh(Some(format!("run-{generation}")));
        }
        assert_eq!(
            jobs.pending_refresh.as_ref().map(|job| job.generation),
            Some(10)
        );
        first_release.notify_one();

        let stale = jobs.recv().await.expect("stale refresh completion");
        assert!(
            jobs.complete(stale).is_none(),
            "stale result must not apply"
        );
        let latest = jobs.recv().await.expect("latest refresh completion");
        let Some(ProjectionUpdate::Refresh(snapshot)) = jobs.complete(latest) else {
            panic!("latest refresh snapshot was not applied");
        };
        assert_eq!(snapshot.selected_run.as_deref(), Some("run-10"));
        assert_eq!(snapshot.error.as_deref(), Some("generation-10"));
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
        assert_eq!(jobs.shutdown().await, 0);
    }

    #[tokio::test]
    async fn blocked_chain_poll_coalesces_requests_and_returns_visible_failure() {
        let pool = SqlitePool::open_in_memory().expect("in-memory pool");
        let started = Arc::new(tokio::sync::Notify::new());
        let started_by_worker = Arc::clone(&started);
        let release = Arc::new(tokio::sync::Notify::new());
        let release_by_worker = Arc::clone(&release);
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_by_worker = Arc::clone(&calls);
        let chain_worker: ChainWorker = Arc::new(move |request| {
            let started = Arc::clone(&started_by_worker);
            let release = Arc::clone(&release_by_worker);
            let calls = Arc::clone(&calls_by_worker);
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                started.notify_one();
                release.notified().await;
                Ok(ChainJobOutput {
                    poller: request.poller,
                    result: Some(Err("synthetic chain timeout".to_owned())),
                })
            })
        });
        let refresh_worker: RefreshWorker =
            Arc::new(|request| Box::pin(async move { Ok(empty_projection(request.generation)) }));
        let mut jobs = BackgroundJobs::with_workers(
            pool,
            crate::tui::db::ChainPoller::with_url(Some("http://test.invalid".to_owned())),
            refresh_worker,
            chain_worker,
        );

        jobs.request_chain_poll();
        started.notified().await;
        for _ in 0..20 {
            jobs.request_chain_poll();
        }
        release.notify_one();
        let result = jobs.recv().await.expect("chain completion");
        let Some(ProjectionUpdate::Chain(Err(error))) = jobs.complete(result) else {
            panic!("chain failure was not projected");
        };
        assert_eq!(error, "synthetic chain timeout");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(jobs.shutdown().await, 0);
    }

    #[test]
    fn terminal_events_translate_without_another_read() {
        assert!(matches!(
            terminal_event_to_action(Event::Resize(120, 42), InputMode::Normal),
            Some(TuiAction::Resize(120, 42))
        ));

        assert!(matches!(
            terminal_event_to_action(
                Event::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char('q'),
                    crossterm::event::KeyModifiers::NONE,
                )),
                InputMode::Normal
            ),
            Some(TuiAction::Quit)
        ));

        assert!(terminal_event_to_action(Event::FocusGained, InputMode::Normal).is_none());
    }

    #[test]
    fn bracketed_paste_is_one_prompt_action_and_ignored_outside_the_composer() {
        let payload = "/status\r\nexplain this".to_owned();
        assert!(matches!(
            terminal_event_to_action(Event::Paste(payload.clone()), InputMode::Prompt),
            Some(TuiAction::PromptPaste(pasted)) if pasted == payload
        ));
        assert!(terminal_event_to_action(Event::Paste(payload), InputMode::Normal).is_none());
    }
}
