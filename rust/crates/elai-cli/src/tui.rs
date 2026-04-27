//! Full-screen TUI for the ELAI CLI.
//!
//! Architecture:
//! - Main thread: ratatui event loop + rendering (owns the terminal).
//! - Background thread: `runtime.run_turn()` execution.
//! - `TuiMsg` channel (runtime → TUI): text chunks, tool calls, usage, done/error.
//! - `PermRequest` channel (runtime → TUI): permission prompts.
//! - `PermDecision` channel (TUI → runtime): approval/denial response.
//!
//! Layout (two columns, style Claude Code):
//!
//! ```text
//! ┌── Header (ELAI card) ───────────────┬── Side panel ──────┐
//! │  ASCII art + Welcome + dir/session  │  Tips / Recent     │
//! ├─────────────────────────────────────┴────────────────────┤
//! │  Chat panel (scrollable)                                  │
//! ├───────────────────────────────────────────────────────────┤
//! │  Status footer: model · perms · tokens · cost · session   │
//! ├───────────────────────────────────────────────────────────┤
//! │  Input box + hint line                                    │
//! └───────────────────────────────────────────────────────────┘
//! ```

use std::env;
use std::io;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEventKind,
};
use pulldown_cmark::{CodeBlockKind, Event as MdEvent, Options, Parser, Tag, TagEnd};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Terminal;

// ─── Inter-thread message types ──────────────────────────────────────────────

/// Events sent from the background runtime thread to the TUI main thread.
#[derive(Debug)]
pub enum TuiMsg {
    TextChunk(String),
    ToolCall { name: String, input: String },
    ToolResult { ok: bool, summary: String },
    Usage { input_tokens: u32, output_tokens: u32 },
    Done,
    Error(String),
    SwdResult(crate::swd::SwdTransaction),
    SwdBatchResult(Vec<crate::swd::SwdTransaction>),
    #[allow(dead_code)]
    BudgetWarning { pct: f32, dimension: String },
    #[allow(dead_code)]
    BudgetExhausted { reason: String },
    #[allow(dead_code)]
    BudgetUpdate { pct: f32, cost_usd: f64 },
    CorrectionRetry { attempt: u8, max_attempts: u8 },
    SwdDiffPreview {
        actions: Vec<(String, Vec<crate::diff::DiffHunk>)>,
        reply_tx: std::sync::mpsc::SyncSender<bool>,
    },
    #[allow(dead_code)]
    SystemNote(String),
}

/// A permission request sent from the runtime thread; the reply channel is
/// embedded so the runtime can block until the user responds.
pub struct PermRequest {
    pub tool_name: String,
    pub input: String,
    pub required_mode: String,
    pub reply_tx: mpsc::SyncSender<PermDecision>,
}

#[derive(Debug, Clone, Copy)]
pub enum PermDecision {
    Allow,
    AllowAlways,
    Deny,
}

// ─── Chat model ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum ChatEntry {
    UserMessage(String),
    AssistantText(String),
    ToolCallEntry { name: String, input: String },
    ToolResultEntry { ok: bool, summary: String },
    SystemNote(String),
    SwdLogEntry {
        transactions: Vec<crate::swd::SwdTransaction>,
        mode: crate::swd::SwdLevel,
    },
    CorrectionRetryEntry { attempt: u8, max_attempts: u8 },
    SwdDiffEntry {
        path: String,
        hunks: Vec<crate::diff::DiffHunk>,
    },
}

// ─── Overlay states ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum OverlayKind {
    ModelPicker {
        items: Vec<String>,
        filter: String,
        selected: usize,
    },
    PermissionPicker {
        items: Vec<String>,
        selected: usize,
    },
    SlashPalette {
        items: Vec<(String, String)>,
        filter: String,
        selected: usize,
    },
    SessionPicker {
        items: Vec<(String, usize)>,
        selected: usize,
    },
    ToolApproval {
        tool_name: String,
        input_preview: String,
        required_mode: String,
        reply_tx: mpsc::SyncSender<PermDecision>,
    },
    SwdConfirmApply {
        action_count: usize,
        reply_tx: std::sync::mpsc::SyncSender<bool>,
    },
    UninstallConfirm,
    SetupWizard {
        step: u8,
        provider_sel: usize,
        key1: String,
        key2: String,
        input: String,
        cursor: usize,
    },
}

// ─── Application state ────────────────────────────────────────────────────────

pub struct UiApp {
    pub model: String,
    pub permission_mode: String,
    pub session_id: String,
    pub input: String,
    pub cursor_col: usize,
    pub chat: Vec<ChatEntry>,
    pub chat_scroll: usize,
    pub thinking: bool,
    pub overlay: Option<OverlayKind>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[allow(dead_code)]
    pub cost_usd: f64,
    pub recent_sessions: Vec<(String, usize)>,
    pub should_quit: bool,
    pub history: Vec<String>,
    pub history_index: Option<usize>,
    pub history_backup: String,
    pub spinner_frame: usize,
    pub read_mode: bool,
    pub swd_level: Arc<AtomicU8>,
    pub budget_pct: f32,
    pub budget_cost_usd: f64,
    pub budget_enabled: bool,
}

impl UiApp {
    pub fn new(
        model: String,
        permission_mode: String,
        session_id: String,
        recent_sessions: Vec<(String, usize)>,
        swd_level: Arc<AtomicU8>,
    ) -> Self {
        Self {
            model,
            permission_mode,
            session_id,
            input: String::new(),
            cursor_col: 0,
            chat: Vec::new(),
            chat_scroll: 0,
            thinking: false,
            overlay: None,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            recent_sessions,
            should_quit: false,
            history: Vec::new(),
            history_index: None,
            history_backup: String::new(),
            spinner_frame: 0,
            read_mode: false,
            swd_level,
            budget_pct: 0.0,
            budget_cost_usd: 0.0,
            budget_enabled: false,
        }
    }

    pub fn tick(&mut self) {
        if self.thinking {
            self.spinner_frame = self.spinner_frame.wrapping_add(1);
        }
    }

    pub fn push_chat(&mut self, entry: ChatEntry) {
        self.chat.push(entry);
        self.scroll_to_bottom();
    }

    fn scroll_to_bottom(&mut self) {
        // `chat_scroll` is a line offset (not message index). We defer the exact
        // bottom offset calculation to `draw_chat`, where we know `max_scroll`
        // = total_rendered_lines - visible_lines.
        self.chat_scroll = usize::MAX;
    }

    pub fn apply_tui_msg(&mut self, msg: TuiMsg) {
        match msg {
            TuiMsg::TextChunk(text) => {
                if let Some(ChatEntry::AssistantText(ref mut buf)) = self.chat.last_mut() {
                    buf.push_str(&text);
                } else {
                    self.chat.push(ChatEntry::AssistantText(text));
                }
                self.scroll_to_bottom();
            }
            TuiMsg::ToolCall { name, input } => {
                self.push_chat(ChatEntry::ToolCallEntry { name, input });
            }
            TuiMsg::ToolResult { ok, summary } => {
                self.push_chat(ChatEntry::ToolResultEntry { ok, summary });
            }
            TuiMsg::Usage {
                input_tokens,
                output_tokens,
            } => {
                self.input_tokens += input_tokens;
                self.output_tokens += output_tokens;
            }
            TuiMsg::Done => {
                self.thinking = false;
            }
            TuiMsg::Error(msg) => {
                self.thinking = false;
                self.push_chat(ChatEntry::SystemNote(format!("❌ Error: {msg}")));
            }
            TuiMsg::SwdResult(tx) => {
                let appended = matches!(
                    self.chat.last_mut(),
                    Some(ChatEntry::SwdLogEntry { .. })
                );
                if appended {
                    if let Some(ChatEntry::SwdLogEntry { transactions, .. }) = self.chat.last_mut() {
                        transactions.push(tx);
                    }
                    self.scroll_to_bottom();
                } else {
                    self.push_chat(ChatEntry::SwdLogEntry {
                        transactions: vec![tx],
                        mode: crate::swd::SwdLevel::Partial,
                    });
                }
            }
            TuiMsg::SwdBatchResult(txs) => {
                self.push_chat(ChatEntry::SwdLogEntry {
                    transactions: txs,
                    mode: crate::swd::SwdLevel::Full,
                });
            }
            TuiMsg::BudgetWarning { pct, dimension } => {
                self.push_chat(ChatEntry::SystemNote(format!(
                    "⚠️  Budget {pct:.0}% consumed ({dimension})"
                )));
            }
            TuiMsg::BudgetExhausted { reason } => {
                self.thinking = false;
                self.push_chat(ChatEntry::SystemNote(format!(
                    "🛑 Budget exhausted: {reason}"
                )));
            }
            TuiMsg::BudgetUpdate { pct, cost_usd } => {
                self.budget_pct = pct;
                self.budget_cost_usd = cost_usd;
            }
            TuiMsg::CorrectionRetry { attempt, max_attempts } => {
                self.push_chat(ChatEntry::CorrectionRetryEntry { attempt, max_attempts });
            }
            TuiMsg::SwdDiffPreview { actions, reply_tx } => {
                let action_count = actions.len();
                for (path, hunks) in actions {
                    self.push_chat(ChatEntry::SwdDiffEntry { path, hunks });
                }
                self.overlay = Some(OverlayKind::SwdConfirmApply { action_count, reply_tx });
            }
            TuiMsg::SystemNote(note) => {
                self.push_chat(ChatEntry::SystemNote(note));
            }
        }
    }

    fn scroll_chat_up(&mut self, delta: usize) {
        self.chat_scroll = self.chat_scroll.saturating_sub(delta);
    }

    fn scroll_chat_down(&mut self, delta: usize) {
        self.chat_scroll = self.chat_scroll.saturating_add(delta);
    }

    fn push_history(&mut self, line: String) {
        if !line.is_empty() && self.history.last().map(|s| s.as_str()) != Some(&line) {
            self.history.push(line);
        }
        self.history_index = None;
        self.history_backup.clear();
    }

    fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                self.history_backup = self.input.clone();
                self.history_index = Some(self.history.len() - 1);
            }
            Some(0) => return,
            Some(n) => {
                self.history_index = Some(n - 1);
            }
        }
        if let Some(idx) = self.history_index {
            self.input = self.history[idx].clone();
            self.cursor_col = self.input.len();
        }
    }

    fn history_down(&mut self) {
        match self.history_index {
            None => {}
            Some(n) if n + 1 >= self.history.len() => {
                self.history_index = None;
                self.input = self.history_backup.clone();
                self.cursor_col = self.input.len();
            }
            Some(n) => {
                self.history_index = Some(n + 1);
                self.input = self.history[n + 1].clone();
                self.cursor_col = self.input.len();
            }
        }
    }

    fn input_char(&mut self, c: char) {
        let idx = self
            .input
            .char_indices()
            .nth(self.cursor_col)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len());
        self.input.insert(idx, c);
        self.cursor_col += 1;
        self.history_index = None;
    }

    fn input_backspace(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            let idx = self
                .input
                .char_indices()
                .nth(self.cursor_col)
                .map(|(i, _)| i)
                .unwrap_or(self.input.len());
            self.input.remove(idx);
        }
    }

    fn move_cursor_left(&mut self) {
        self.cursor_col = self.cursor_col.saturating_sub(1);
    }

    fn move_cursor_right(&mut self) {
        if self.cursor_col < self.input.chars().count() {
            self.cursor_col += 1;
        }
    }

    fn move_cursor_home(&mut self) {
        self.cursor_col = 0;
    }

    fn move_cursor_end(&mut self) {
        self.cursor_col = self.input.chars().count();
    }

    fn clear_input(&mut self) {
        self.input.clear();
        self.cursor_col = 0;
        self.history_index = None;
    }

    #[allow(dead_code)]
    fn take_input(&mut self) -> String {
        let text = self.input.clone();
        self.clear_input();
        text
    }

    fn filtered_model_list(filter: &str) -> Vec<String> {
        let all = [
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001",
            "claude-opus-4-7-thinking",
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4.5",
            "o1",
            "o3",
            "o4-mini",
            "grok-3",
            "grok-3-mini",
        ];
        let f = filter.to_lowercase();
        all.iter()
            .filter(|m| f.is_empty() || m.to_lowercase().contains(&f))
            .map(|s| s.to_string())
            .collect()
    }

    pub fn open_model_picker(&mut self) {
        let items = Self::filtered_model_list("");
        self.overlay = Some(OverlayKind::ModelPicker {
            items,
            filter: String::new(),
            selected: 0,
        });
    }

    pub fn open_permission_picker(&mut self) {
        let items = vec![
            "read-only".to_string(),
            "workspace-write".to_string(),
            "danger-full-access".to_string(),
        ];
        let selected = items
            .iter()
            .position(|s| s == &self.permission_mode)
            .unwrap_or(0);
        self.overlay = Some(OverlayKind::PermissionPicker { items, selected });
    }

    pub fn open_slash_palette(&mut self) {
        let items = slash_palette_items();
        self.overlay = Some(OverlayKind::SlashPalette {
            filter: String::new(),
            items,
            selected: 0,
        });
    }

    pub fn open_session_picker(&mut self) {
        self.overlay = Some(OverlayKind::SessionPicker {
            items: self.recent_sessions.clone(),
            selected: 0,
        });
    }

    pub fn open_setup_wizard(&mut self) {
        self.overlay = Some(OverlayKind::SetupWizard {
            step: 0,
            provider_sel: 0,
            key1: String::new(),
            key2: String::new(),
            input: String::new(),
            cursor: 0,
        });
    }

    pub fn open_uninstall_confirm(&mut self) {
        self.overlay = Some(OverlayKind::UninstallConfirm);
    }
}

fn slash_palette_items() -> Vec<(String, String)> {
    vec![
        ("help".into(), "Mostrar ajuda".into()),
        ("status".into(), "Status da sessão".into()),
        ("model".into(), "Mostrar/trocar modelo".into()),
        ("permissions".into(), "Mostrar/trocar permissões".into()),
        ("session".into(), "Retomar sessão".into()),
        ("clear".into(), "Limpar histórico".into()),
        ("compact".into(), "Compactar histórico".into()),
        ("cost".into(), "Mostrar custo".into()),
        ("diff".into(), "Mostrar git diff".into()),
        ("export".into(), "Exportar conversa".into()),
        ("init".into(), "Inicializar projeto".into()),
        ("memory".into(), "Mostrar ELAI.md".into()),
        ("dream".into(), "Comprimir entradas antigas da memória (AI)".into()),
        ("verify".into(), "Verificar codebase vs memória (ELAI.md)".into()),
        ("version".into(), "Mostrar versão".into()),
        ("swd".into(), "Strict Write Discipline (off/partial/full)".into()),
        ("budget".into(), "Budget limiter (tokens/custo)".into()),
        ("keys".into(), "Configurar/trocar API keys".into()),
        ("uninstall".into(), "Desinstalar Elai Code".into()),
        ("exit".into(), "Sair".into()),
    ]
}

// ─── Terminal lifecycle helpers ───────────────────────────────────────────────

/// Enter alternate screen + raw mode + mouse capture.
pub fn enter_tui(stdout: &mut impl io::Write) -> io::Result<()> {
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(())
}

/// Restore terminal on exit (always call even on error).
pub fn leave_tui(stdout: &mut impl io::Write) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(stdout, LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}

// ─── Main TUI loop ────────────────────────────────────────────────────────────

/// Result returned when the user submits input or picks an action inside the TUI.
pub enum TuiAction {
    SendMessage(String),
    SetModel(String),
    SetPermissions(String),
    ResumeSession(String),
    SlashCommand(String),
    EnterReadMode,
    ExitReadMode,
    SetupComplete,
    Uninstall,
    Quit,
    None,
}

/// Drive a single frame-cycle: poll events, update state, return an action.
pub fn poll_and_handle(
    app: &mut UiApp,
    msg_rx: &mpsc::Receiver<TuiMsg>,
    perm_rx: &mpsc::Receiver<PermRequest>,
) -> TuiAction {
    // Drain runtime messages first (non-blocking).
    loop {
        match msg_rx.try_recv() {
            Ok(msg) => app.apply_tui_msg(msg),
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                if app.thinking {
                    app.thinking = false;
                }
                break;
            }
        }
    }

    // Check for permission requests (non-blocking).
    if app.overlay.is_none() {
        if let Ok(req) = perm_rx.try_recv() {
            let input_preview = req.input.chars().take(80).collect::<String>();
            app.overlay = Some(OverlayKind::ToolApproval {
                tool_name: req.tool_name,
                input_preview,
                required_mode: req.required_mode,
                reply_tx: req.reply_tx,
            });
        }
    }

    // Poll terminal events with short timeout so the loop stays responsive.
    if !event::poll(Duration::from_millis(50)).unwrap_or(false) {
        return TuiAction::None;
    }

    let ev = match event::read() {
        Ok(ev) => ev,
        Err(_) => return TuiAction::None,
    };

    match ev {
        Event::Resize(_, _) => TuiAction::None,

        Event::Mouse(mouse) => {
            if !app.read_mode {
                match mouse.kind {
                    MouseEventKind::ScrollUp => app.scroll_chat_up(2),
                    MouseEventKind::ScrollDown => app.scroll_chat_down(2),
                    _ => {}
                }
            }
            TuiAction::None
        }

        Event::Key(key) if key.kind == KeyEventKind::Press => {
            if app.read_mode {
                return TuiAction::ExitReadMode;
            }
            handle_key(app, key)
        }

        _ => TuiAction::None,
    }
}

fn handle_key(app: &mut UiApp, key: KeyEvent) -> TuiAction {
    // ── Active overlay ────────────────────────────────────────────────────────
    if app.overlay.is_some() {
        return handle_overlay_key(app, key);
    }

    // ── Global shortcuts (no overlay) ─────────────────────────────────────────
    match (key.modifiers, key.code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            if app.thinking {
                app.thinking = false;
                app.push_chat(ChatEntry::SystemNote("⚠ Geração cancelada.".into()));
                return TuiAction::None;
            }
            return TuiAction::Quit;
        }
        (KeyModifiers::CONTROL, KeyCode::Char('l')) => {
            app.chat.clear();
            app.chat_scroll = 0;
            return TuiAction::None;
        }
        (KeyModifiers::CONTROL, KeyCode::Char('k')) => {
            app.open_slash_palette();
            return TuiAction::None;
        }
        (KeyModifiers::CONTROL, KeyCode::Char('r')) => {
            return TuiAction::EnterReadMode;
        }
        (KeyModifiers::NONE, KeyCode::F(2)) => {
            app.open_model_picker();
            return TuiAction::None;
        }
        (KeyModifiers::NONE, KeyCode::F(3)) => {
            app.open_permission_picker();
            return TuiAction::None;
        }
        (KeyModifiers::NONE, KeyCode::F(4)) => {
            app.open_session_picker();
            return TuiAction::None;
        }
        // Scroll chat
        (KeyModifiers::NONE, KeyCode::PageUp) => {
            app.scroll_chat_up(10);
            return TuiAction::None;
        }
        (KeyModifiers::NONE, KeyCode::PageDown) => {
            app.scroll_chat_down(10);
            return TuiAction::None;
        }
        (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
            app.scroll_chat_up(5);
            return TuiAction::None;
        }
        (KeyModifiers::CONTROL, KeyCode::Char('d')) => {
            app.scroll_chat_down(5);
            return TuiAction::None;
        }
        (KeyModifiers::NONE, KeyCode::Home) if app.input.is_empty() => {
            app.chat_scroll = 0;
            return TuiAction::None;
        }
        (KeyModifiers::NONE, KeyCode::End) if app.input.is_empty() => {
            app.scroll_to_bottom();
            return TuiAction::None;
        }
        _ => {}
    }

    // ── Input-level shortcuts ─────────────────────────────────────────────────

    // Don't accept input while the runtime is thinking.
    if app.thinking {
        return TuiAction::None;
    }

    match (key.modifiers, key.code) {
        // Submit
        (KeyModifiers::NONE, KeyCode::Enter) => {
            let text = app.input.trim().to_string();
            if text.is_empty() {
                return TuiAction::None;
            }
            app.push_history(text.clone());

            if matches!(text.as_str(), "/exit" | "/quit") {
                return TuiAction::Quit;
            }

            // Slash commands that open overlays.
            if text == "/model" {
                app.clear_input();
                app.open_model_picker();
                return TuiAction::None;
            }
            if text == "/permissions" {
                app.clear_input();
                app.open_permission_picker();
                return TuiAction::None;
            }
            if text == "/session" {
                app.clear_input();
                app.open_session_picker();
                return TuiAction::None;
            }

            app.clear_input();
            app.push_chat(ChatEntry::UserMessage(text.clone()));
            app.scroll_to_bottom();

            // Detect other slash commands.
            if text.starts_with('/') {
                return TuiAction::SlashCommand(text);
            }
            TuiAction::SendMessage(text)
        }

        // Newline (Shift+Enter)
        (KeyModifiers::SHIFT, KeyCode::Enter) => {
            app.input_char('\n');
            TuiAction::None
        }

        // '/' on empty input: auto-open slash palette
        (KeyModifiers::NONE, KeyCode::Char('/')) if app.input.is_empty() => {
            app.open_slash_palette();
            TuiAction::None
        }

        // Tab: open slash palette (also works mid-word for / commands)
        (KeyModifiers::NONE, KeyCode::Tab) => {
            if app.input.is_empty() || app.input.starts_with('/') {
                app.open_slash_palette();
            }
            TuiAction::None
        }

        // History
        (KeyModifiers::NONE, KeyCode::Up) => {
            app.history_up();
            TuiAction::None
        }
        (KeyModifiers::NONE, KeyCode::Down) => {
            app.history_down();
            TuiAction::None
        }

        // Cursor movement
        (KeyModifiers::NONE, KeyCode::Left) => {
            app.move_cursor_left();
            TuiAction::None
        }
        (KeyModifiers::NONE, KeyCode::Right) => {
            app.move_cursor_right();
            TuiAction::None
        }
        (KeyModifiers::NONE, KeyCode::Home) => {
            app.move_cursor_home();
            TuiAction::None
        }
        (KeyModifiers::NONE, KeyCode::End) => {
            app.move_cursor_end();
            TuiAction::None
        }

        // Delete
        (KeyModifiers::NONE, KeyCode::Backspace)
        | (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
            app.input_backspace();
            TuiAction::None
        }

        // Regular character
        (_, KeyCode::Char(c)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input_char(c);
            TuiAction::None
        }

        _ => TuiAction::None,
    }
}

fn handle_overlay_key(app: &mut UiApp, key: KeyEvent) -> TuiAction {
    // Helper closures for navigation.
    let overlay = app.overlay.take();
    match overlay {
        Some(OverlayKind::ToolApproval {
            tool_name,
            input_preview: _,
            required_mode: _,
            reply_tx,
        }) => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Char('y'))
                | (KeyModifiers::NONE, KeyCode::Enter) => {
                    let _ = reply_tx.send(PermDecision::Allow);
                }
                (KeyModifiers::NONE, KeyCode::Char('a')) => {
                    let _ = reply_tx.send(PermDecision::AllowAlways);
                    app.push_chat(ChatEntry::SystemNote(format!(
                        "✅ Tool '{tool_name}' adicionada à whitelist permanente."
                    )));
                }
                _ => {
                    let _ = reply_tx.send(PermDecision::Deny);
                    app.push_chat(ChatEntry::SystemNote(format!(
                        "⛔ Tool '{tool_name}' negado pelo usuário."
                    )));
                }
            }
            app.overlay = None;
            TuiAction::None
        }

        Some(OverlayKind::ModelPicker {
            items,
            mut filter,
            mut selected,
        }) => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Esc) => {
                    app.overlay = None;
                }
                (KeyModifiers::NONE, KeyCode::Up) => {
                    selected = selected.saturating_sub(1);
                    app.overlay = Some(OverlayKind::ModelPicker { items, filter, selected });
                }
                (KeyModifiers::NONE, KeyCode::Down) => {
                    let filtered = UiApp::filtered_model_list(&filter);
                    selected = (selected + 1).min(filtered.len().saturating_sub(1));
                    app.overlay = Some(OverlayKind::ModelPicker { items, filter, selected });
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    let filtered = UiApp::filtered_model_list(&filter);
                    if let Some(model) = filtered.get(selected) {
                        let m = model.clone();
                        app.overlay = None;
                        return TuiAction::SetModel(m);
                    }
                    app.overlay = None;
                }
                (KeyModifiers::NONE, KeyCode::Backspace) => {
                    filter.pop();
                    selected = 0;
                    app.overlay = Some(OverlayKind::ModelPicker { items, filter, selected });
                }
                (_, KeyCode::Char(c)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    filter.push(c);
                    selected = 0;
                    app.overlay = Some(OverlayKind::ModelPicker { items, filter, selected });
                }
                _ => {
                    app.overlay = Some(OverlayKind::ModelPicker { items, filter, selected });
                }
            }
            TuiAction::None
        }

        Some(OverlayKind::PermissionPicker {
            items,
            mut selected,
        }) => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Esc) => {
                    app.overlay = None;
                }
                (KeyModifiers::NONE, KeyCode::Up) => {
                    selected = selected.saturating_sub(1);
                    app.overlay = Some(OverlayKind::PermissionPicker { items, selected });
                }
                (KeyModifiers::NONE, KeyCode::Down) => {
                    selected = (selected + 1).min(items.len().saturating_sub(1));
                    app.overlay = Some(OverlayKind::PermissionPicker { items, selected });
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    if let Some(perm) = items.get(selected) {
                        let p = perm.clone();
                        app.overlay = None;
                        return TuiAction::SetPermissions(p);
                    }
                    app.overlay = None;
                }
                _ => {
                    app.overlay = Some(OverlayKind::PermissionPicker { items, selected });
                }
            }
            TuiAction::None
        }

        Some(OverlayKind::SlashPalette {
            items,
            mut filter,
            mut selected,
        }) => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Up) => {
                    selected = selected.saturating_sub(1);
                    app.overlay = Some(OverlayKind::SlashPalette { items, filter, selected });
                }
                (KeyModifiers::NONE, KeyCode::Down) => {
                    let filtered = filter_slash_items(&items, &filter);
                    selected = (selected + 1).min(filtered.len().saturating_sub(1));
                    app.overlay = Some(OverlayKind::SlashPalette { items, filter, selected });
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    let filtered = filter_slash_items(&items, &filter);
                    if let Some((cmd, _)) = filtered.get(selected) {
                        let cmd = cmd.clone();
                        app.overlay = None;
                        app.clear_input();
                        return TuiAction::SlashCommand(format!("/{cmd}"));
                    }
                    app.overlay = None;
                    app.clear_input();
                }
                (KeyModifiers::NONE, KeyCode::Esc) => {
                    app.overlay = None;
                    app.clear_input();
                }
                (KeyModifiers::NONE, KeyCode::Backspace) => {
                    filter.pop();
                    selected = 0;
                    app.overlay = Some(OverlayKind::SlashPalette { items, filter, selected });
                }
                (_, KeyCode::Char(c)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let c = if c == '/' && filter.is_empty() { c } else { c };
                    filter.push(c);
                    selected = 0;
                    app.overlay = Some(OverlayKind::SlashPalette { items, filter, selected });
                }
                _ => {
                    app.overlay = Some(OverlayKind::SlashPalette { items, filter, selected });
                }
            }
            TuiAction::None
        }

        Some(OverlayKind::SessionPicker {
            items,
            mut selected,
        }) => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Esc) => {
                    app.overlay = None;
                }
                (KeyModifiers::NONE, KeyCode::Up) => {
                    selected = selected.saturating_sub(1);
                    app.overlay = Some(OverlayKind::SessionPicker { items, selected });
                }
                (KeyModifiers::NONE, KeyCode::Down) => {
                    selected = (selected + 1).min(items.len().saturating_sub(1));
                    app.overlay = Some(OverlayKind::SessionPicker { items, selected });
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    if let Some((session_id, _)) = items.get(selected) {
                        let s = session_id.clone();
                        app.overlay = None;
                        return TuiAction::ResumeSession(s);
                    }
                    app.overlay = None;
                }
                _ => {
                    app.overlay = Some(OverlayKind::SessionPicker { items, selected });
                }
            }
            TuiAction::None
        }

        Some(OverlayKind::UninstallConfirm) => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    app.overlay = None;
                    return TuiAction::Uninstall;
                }
                _ => {
                    app.overlay = None;
                    app.push_chat(ChatEntry::SystemNote("Desinstalação cancelada.".into()));
                }
            }
            TuiAction::None
        }

        Some(OverlayKind::SwdConfirmApply {
            action_count: _,
            reply_tx,
        }) => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Char('a'))
                | (KeyModifiers::NONE, KeyCode::Enter) => {
                    let _ = reply_tx.send(true);
                    app.push_chat(ChatEntry::SystemNote(
                        "✅ SWD: batch aceito — aplicando...".into(),
                    ));
                }
                _ => {
                    let _ = reply_tx.send(false);
                    app.push_chat(ChatEntry::SystemNote(
                        "⛔ SWD: batch rejeitado pelo usuário.".into(),
                    ));
                }
            }
            app.overlay = None;
            TuiAction::None
        }

        Some(OverlayKind::SetupWizard {
            step,
            provider_sel,
            key1,
            key2,
            input,
            cursor,
        }) => {
            match step {
                0 => {
                    // Provider selection step
                    match (key.modifiers, key.code) {
                        (KeyModifiers::NONE, KeyCode::Esc) => {
                            app.overlay = None;
                        }
                        (KeyModifiers::NONE, KeyCode::Up) => {
                            let sel = provider_sel.saturating_sub(1);
                            app.overlay = Some(OverlayKind::SetupWizard {
                                step,
                                provider_sel: sel,
                                key1,
                                key2,
                                input,
                                cursor,
                            });
                        }
                        (KeyModifiers::NONE, KeyCode::Down) => {
                            let sel = (provider_sel + 1).min(2);
                            app.overlay = Some(OverlayKind::SetupWizard {
                                step,
                                provider_sel: sel,
                                key1,
                                key2,
                                input,
                                cursor,
                            });
                        }
                        (KeyModifiers::NONE, KeyCode::Enter) => {
                            // Advance to key input step
                            app.overlay = Some(OverlayKind::SetupWizard {
                                step: 1,
                                provider_sel,
                                key1,
                                key2,
                                input: String::new(),
                                cursor: 0,
                            });
                        }
                        _ => {
                            app.overlay = Some(OverlayKind::SetupWizard {
                                step,
                                provider_sel,
                                key1,
                                key2,
                                input,
                                cursor,
                            });
                        }
                    }
                    TuiAction::None
                }
                1 | 2 => {
                    // Key input step
                    match (key.modifiers, key.code) {
                        (KeyModifiers::NONE, KeyCode::Esc) => {
                            app.overlay = None;
                            TuiAction::None
                        }
                        (KeyModifiers::NONE, KeyCode::Backspace)
                        | (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
                            let mut new_input = input.clone();
                            let mut new_cursor = cursor;
                            if new_cursor > 0 {
                                new_cursor -= 1;
                                let byte_idx = new_input
                                    .char_indices()
                                    .nth(new_cursor)
                                    .map(|(i, _)| i)
                                    .unwrap_or(new_input.len());
                                new_input.remove(byte_idx);
                            }
                            app.overlay = Some(OverlayKind::SetupWizard {
                                step,
                                provider_sel,
                                key1,
                                key2,
                                input: new_input,
                                cursor: new_cursor,
                            });
                            TuiAction::None
                        }
                        (KeyModifiers::NONE, KeyCode::Enter) => {
                            if step == 1 {
                                // Finished typing key1
                                let new_key1 = input.clone();
                                if provider_sel == 2 {
                                    // "Both" — advance to step 2 for key2
                                    app.overlay = Some(OverlayKind::SetupWizard {
                                        step: 2,
                                        provider_sel,
                                        key1: new_key1,
                                        key2: String::new(),
                                        input: String::new(),
                                        cursor: 0,
                                    });
                                    TuiAction::None
                                } else {
                                    // Single provider — save and close
                                    let _ = save_setup_keys(provider_sel, &new_key1, "");
                                    app.overlay = None;
                                    TuiAction::SetupComplete
                                }
                            } else {
                                // step == 2: finished typing key2
                                let new_key2 = input.clone();
                                let _ = save_setup_keys(provider_sel, &key1, &new_key2);
                                app.overlay = None;
                                TuiAction::SetupComplete
                            }
                        }
                        (_, KeyCode::Char(c))
                            if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            let mut new_input = input.clone();
                            let mut new_cursor = cursor;
                            let byte_idx = new_input
                                .char_indices()
                                .nth(new_cursor)
                                .map(|(i, _)| i)
                                .unwrap_or(new_input.len());
                            new_input.insert(byte_idx, c);
                            new_cursor += 1;
                            app.overlay = Some(OverlayKind::SetupWizard {
                                step,
                                provider_sel,
                                key1,
                                key2,
                                input: new_input,
                                cursor: new_cursor,
                            });
                            TuiAction::None
                        }
                        _ => {
                            app.overlay = Some(OverlayKind::SetupWizard {
                                step,
                                provider_sel,
                                key1,
                                key2,
                                input,
                                cursor,
                            });
                            TuiAction::None
                        }
                    }
                }
                _ => {
                    app.overlay = None;
                    TuiAction::None
                }
            }
        }

        None => TuiAction::None,
    }
}

fn filter_slash_items<'a>(
    items: &'a [(String, String)],
    filter: &str,
) -> Vec<&'a (String, String)> {
    let f = filter.trim_start_matches('/').to_lowercase();
    items
        .iter()
        .filter(|(cmd, desc)| {
            f.is_empty()
                || cmd.to_lowercase().contains(&f)
                || desc.to_lowercase().contains(&f)
        })
        .collect()
}

// ─── Rendering ────────────────────────────────────────────────────────────────

pub fn render(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut UiApp,
) -> io::Result<()> {
    terminal.draw(|frame| {
        let size = frame.area();

        // Outer vertical split: header, body, status, input.
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(10), // header
                Constraint::Min(3),     // chat body
                Constraint::Length(1),  // status footer
                Constraint::Length(3),  // input + hint
            ])
            .split(size);

        draw_header(frame, outer[0], app);
        draw_chat(frame, outer[1], app);
        draw_status(frame, outer[2], app);
        draw_input(frame, outer[3], app);

        // Draw overlays on top.
        if let Some(ref overlay) = app.overlay {
            draw_overlay(frame, size, overlay, app);
        }
    })?;
    Ok(())
}

// ── Header ───────────────────────────────────────────────────────────────────

fn draw_header(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &UiApp,
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);

    draw_elai_card(frame, cols[0], app);
    draw_side_panel(frame, cols[1], app);
}

const ELAI_ASCII: &str = "\
  ██████████████████   ███████╗██╗      █████╗ ██╗\n\
  ████████▓▓▄▄▓▓▄▄▓▓   ██╔════╝██║     ██╔══██╗██║\n\
  ████████▓▓██▓▓██▓▓   █████╗  ██║     ███████║██║\n\
  ████████▓▓▀▀▓▓▀▀▓▓   ██╔══╝  ██║     ██╔══██║██║\n\
    ████████████████   ███████╗███████╗██║  ██║██║\n\
          ████  ████   ╚══════╝╚══════╝╚═╝  ╚═╝╚═╝";

fn draw_elai_card(frame: &mut ratatui::Frame, area: Rect, _app: &UiApp) {
    // corpo do mascote e texto ELAI: laranja claro
    let body_style = Style::default().fg(Color::Rgb(242, 222, 206));
    // olhos (▄ ▀ e █ depois de ▓): laranja saturado
    let eye_style = Style::default().fg(Color::Rgb(201, 123, 74));
    // ▓ células: cavidade dos olhos — marrom escuro visível
    let dot_style = Style::default().fg(Color::Rgb(110, 65, 28));
    let dim = Style::default().fg(Color::DarkGray);

    let username = whoami_user();
    let cwd = env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "~".to_string());

    let mut lines: Vec<Line> = ELAI_ASCII
        .lines()
        .map(|l| {
            #[derive(Clone, Copy, PartialEq)]
            enum Seg { Body, Dot, Eye }

            let mut spans: Vec<Span> = Vec::new();
            let mut current = String::new();
            let mut seg = Seg::Body;

            for ch in l.chars() {
                let next = match ch {
                    '▓' => Seg::Dot,
                    '▄' | '▀' => Seg::Eye,
                    '█' if matches!(seg, Seg::Dot | Seg::Eye) => Seg::Eye,
                    _ => Seg::Body,
                };
                if next != seg && !current.is_empty() {
                    spans.push(Span::styled(current.clone(), match seg {
                        Seg::Body => body_style,
                        Seg::Dot  => dot_style,
                        Seg::Eye  => eye_style,
                    }));
                    current.clear();
                }
                seg = next;
                current.push(ch);
            }
            if !current.is_empty() {
                spans.push(Span::styled(current, match seg {
                    Seg::Body => body_style,
                    Seg::Dot  => dot_style,
                    Seg::Eye  => eye_style,
                }));
            }
            Line::from(spans)
        })
        .collect();

    lines.push(Line::from(vec![
        Span::styled(format!("  Welcome back, {username}!"), dim),
        Span::raw("  "),
        Span::styled(
            format!("v{}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(Color::Rgb(201, 123, 74)),
        ),
    ]));
    lines.push(Line::from(Span::styled(format!("  {cwd}"), dim)));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(201, 123, 74)))
        .padding(Padding::new(2, 0, 0, 0));

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

fn draw_side_panel(frame: &mut ratatui::Frame, area: Rect, app: &UiApp) {
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "Tips for getting started",
            Style::default()
                .fg(Color::Indexed(215))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "  Run /init to create a ELAI.md",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "  F2 trocar modelo · F3 permissões",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "  Ctrl+K slash palette",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            "Recent activity",
            Style::default()
                .fg(Color::Indexed(215))
                .add_modifier(Modifier::BOLD),
        )),
    ];

    if app.recent_sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No recent activity",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for (session_id, msg_count) in app.recent_sessions.iter().take(3) {
            let short_id = session_id
                .strip_prefix("session-")
                .unwrap_or(session_id)
                .chars()
                .take(12)
                .collect::<String>();
            lines.push(Line::from(Span::styled(
                format!("  • {short_id} ({msg_count} msgs)"),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Indexed(215)));

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

// ── Chat panel ────────────────────────────────────────────────────────────────

fn draw_chat(frame: &mut ratatui::Frame, area: Rect, app: &mut UiApp) {
    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::TOP)
        .border_style(Style::default().fg(Color::Indexed(239)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = chat_to_lines(app, inner.width as usize);
    let total = lines.len();
    let visible = inner.height as usize;
    let max_scroll = total.saturating_sub(visible);
    let scroll = app.chat_scroll.min(max_scroll);
    // Keep the state normalized to the current viewport/content dimensions.
    app.chat_scroll = scroll;

    let display: Vec<Line> = lines.into_iter().skip(scroll).take(visible).collect();

    let paragraph = Paragraph::new(display).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);

    // Scrollbar.
    if total > visible {
        let mut scroll_state = ScrollbarState::new(max_scroll).position(scroll);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));
        frame.render_stateful_widget(scrollbar, area, &mut scroll_state);
    }
}

fn markdown_to_tui_lines(text: &str, wrap_width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut bold = false;
    let mut italic = false;
    let mut heading: Option<u8> = None;
    let mut in_code = false;
    let mut list_depth: usize = 0;

    let flush = |lines: &mut Vec<Line<'static>>, spans: &mut Vec<Span<'static>>| {
        if !spans.is_empty() {
            lines.push(Line::from(std::mem::take(spans)));
        }
    };

    let text_style = |bold: bool, italic: bool, heading: Option<u8>| -> Style {
        let mut s = Style::default();
        if bold || heading.is_some() {
            s = s.add_modifier(Modifier::BOLD);
        }
        if italic {
            s = s.add_modifier(Modifier::ITALIC);
        }
        s.fg(match heading {
            Some(1) => Color::Cyan,
            Some(2) => Color::White,
            Some(3) => Color::Blue,
            Some(_) => Color::Gray,
            None if bold => Color::Yellow,
            _ => Color::Indexed(252),
        })
    };

    for event in Parser::new_ext(text, Options::all()) {
        match event {
            MdEvent::Start(Tag::Heading { level, .. }) => {
                flush(&mut lines, &mut spans);
                heading = Some(level as u8);
            }
            MdEvent::End(TagEnd::Heading(..)) => {
                flush(&mut lines, &mut spans);
                lines.push(Line::from(""));
                heading = None;
            }
            MdEvent::Start(Tag::Strong) => bold = true,
            MdEvent::End(TagEnd::Strong) => bold = false,
            MdEvent::Start(Tag::Emphasis) => italic = true,
            MdEvent::End(TagEnd::Emphasis) => italic = false,
            MdEvent::Start(Tag::List(_)) => list_depth += 1,
            MdEvent::End(TagEnd::List(..)) => {
                list_depth = list_depth.saturating_sub(1);
                if list_depth == 0 {
                    lines.push(Line::from(""));
                }
            }
            MdEvent::Start(Tag::Item) => {
                flush(&mut lines, &mut spans);
                let indent = "  ".repeat(list_depth.saturating_sub(1));
                spans.push(Span::styled(
                    format!("{indent}• "),
                    Style::default().fg(Color::Indexed(215)),
                ));
            }
            MdEvent::End(TagEnd::Item) => flush(&mut lines, &mut spans),
            MdEvent::Start(Tag::CodeBlock(kind)) => {
                in_code = true;
                flush(&mut lines, &mut spans);
                let lang = match kind {
                    CodeBlockKind::Fenced(l) if !l.is_empty() => format!(" {l} "),
                    _ => String::new(),
                };
                lines.push(Line::from(Span::styled(
                    format!("  ╭─{lang}─"),
                    Style::default().fg(Color::Indexed(239)),
                )));
            }
            MdEvent::End(TagEnd::CodeBlock) => {
                in_code = false;
                flush(&mut lines, &mut spans);
                lines.push(Line::from(Span::styled(
                    "  ╰──────",
                    Style::default().fg(Color::Indexed(239)),
                )));
                lines.push(Line::from(""));
            }
            MdEvent::Text(t) => {
                let t = t.into_string();
                if in_code {
                    for l in t.lines() {
                        flush(&mut lines, &mut spans);
                        lines.push(Line::from(Span::styled(
                            format!("  │ {l}"),
                            Style::default().fg(Color::Indexed(156)),
                        )));
                    }
                } else {
                    let style = text_style(bold, italic, heading);
                    let mut first = true;
                    for raw_line in t.lines() {
                        if !first {
                            flush(&mut lines, &mut spans);
                        }
                        first = false;
                        for chunk in wrap_text(raw_line, wrap_width) {
                            spans.push(Span::styled(chunk, style));
                        }
                    }
                }
            }
            MdEvent::Code(t) => {
                spans.push(Span::styled(
                    t.into_string(),
                    Style::default().fg(Color::Green),
                ));
            }
            MdEvent::SoftBreak => spans.push(Span::raw(" ")),
            MdEvent::HardBreak => flush(&mut lines, &mut spans),
            MdEvent::End(TagEnd::Paragraph) => {
                flush(&mut lines, &mut spans);
                lines.push(Line::from(""));
            }
            MdEvent::Rule => {
                flush(&mut lines, &mut spans);
                lines.push(Line::from(Span::styled(
                    "─".repeat(wrap_width.min(60)),
                    Style::default().fg(Color::DarkGray),
                )));
                lines.push(Line::from(""));
            }
            _ => {}
        }
    }
    flush(&mut lines, &mut spans);

    // Remove trailing blank lines
    while lines
        .last()
        .map(|l: &Line| l.spans.iter().all(|s| s.content.trim().is_empty()))
        .unwrap_or(false)
    {
        lines.pop();
    }
    lines
}

fn chat_to_lines(app: &UiApp, width: usize) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    let wrap_width = width.saturating_sub(4).max(20);

    for entry in &app.chat {
        match entry {
            ChatEntry::UserMessage(msg) => {
                result.push(Line::from(Span::styled(
                    "> ".to_string(),
                    Style::default()
                        .fg(Color::Indexed(215))
                        .add_modifier(Modifier::BOLD),
                )));
                for line in wrap_text(msg, wrap_width) {
                    result.push(Line::from(Span::styled(
                        format!("  {line}"),
                        Style::default().fg(Color::White),
                    )));
                }
                result.push(Line::from(""));
            }
            ChatEntry::AssistantText(text) => {
                let md_lines = markdown_to_tui_lines(text, wrap_width.saturating_sub(2));
                for line in md_lines {
                    let mut indented_spans = vec![Span::raw("  ")];
                    indented_spans.extend(line.spans);
                    result.push(Line::from(indented_spans));
                }
                result.push(Line::from(""));
            }
            ChatEntry::ToolCallEntry { name, input } => {
                let summary = input.chars().take(60).collect::<String>();
                result.push(Line::from(vec![
                    Span::styled("  ⚙ tool: ", Style::default().fg(Color::Cyan)),
                    Span::styled(
                        name.clone(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("({summary}…)"), Style::default().fg(Color::DarkGray)),
                ]));
            }
            ChatEntry::ToolResultEntry { ok, summary } => {
                let (icon, style) = if *ok {
                    ("↳ ok", Style::default().fg(Color::Green))
                } else {
                    ("↳ err", Style::default().fg(Color::Red))
                };
                let short = summary.chars().take(70).collect::<String>();
                result.push(Line::from(Span::styled(
                    format!("    {icon}: {short}"),
                    style,
                )));
            }
            ChatEntry::SystemNote(note) => {
                for line in note.lines() {
                    result.push(Line::from(Span::styled(
                        format!("  {line}"),
                        Style::default().fg(Color::Yellow),
                    )));
                }
                result.push(Line::from(""));
            }
            ChatEntry::SwdLogEntry { transactions, mode } => {
                use crate::swd::SwdOutcome;
                result.push(Line::from(Span::styled(
                    format!(
                        "  ⛨ SWD {} — {} operação(ões)",
                        mode.as_str(),
                        transactions.len()
                    ),
                    Style::default()
                        .fg(Color::Indexed(215))
                        .add_modifier(Modifier::BOLD),
                )));
                for tx in transactions {
                    let (icon, color) = match &tx.outcome {
                        SwdOutcome::Verified => ("✓", Color::Green),
                        SwdOutcome::Noop => ("·", Color::Yellow),
                        SwdOutcome::Drift { .. } => ("~", Color::Yellow),
                        SwdOutcome::Failed { .. } => ("✗", Color::Red),
                        SwdOutcome::RolledBack => ("↩", Color::Red),
                    };
                    let short_path: String = if tx.path.len() > 45 {
                        format!("…{}", &tx.path[tx.path.len() - 44..])
                    } else {
                        tx.path.clone()
                    };
                    result.push(Line::from(vec![
                        Span::styled(format!("    {icon} "), Style::default().fg(color)),
                        Span::styled(short_path, Style::default().fg(Color::White)),
                        Span::styled(
                            format!("  [{}]", tx.tool_name),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
                result.push(Line::from(""));
            }
            ChatEntry::CorrectionRetryEntry { attempt, max_attempts } => {
                result.push(Line::from(Span::styled(
                    format!("  \u{21a9} SWD retry {attempt}/{max_attempts}"),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )));
                result.push(Line::from(""));
            }
            ChatEntry::SwdDiffEntry { path, hunks } => {
                use crate::diff::DiffTag;
                result.push(Line::from(Span::styled(
                    format!("  --- {path}"),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                )));
                if hunks.is_empty() {
                    result.push(Line::from(Span::styled(
                        "  (Nenhuma alteração detectada)",
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    for hunk in hunks {
                        let old_count = hunk.lines.iter()
                            .filter(|l| matches!(l.tag, DiffTag::Keep | DiffTag::Remove))
                            .count();
                        let new_count = hunk.lines.iter()
                            .filter(|l| matches!(l.tag, DiffTag::Keep | DiffTag::Add))
                            .count();
                        result.push(Line::from(Span::styled(
                            format!(
                                "  @@ -{},{} +{},{} @@",
                                hunk.old_start, old_count, hunk.new_start, new_count
                            ),
                            Style::default().fg(Color::Magenta),
                        )));
                        for line in &hunk.lines {
                            let (marker, style) = match line.tag {
                                DiffTag::Keep => (
                                    " ",
                                    Style::default().fg(Color::DarkGray),
                                ),
                                DiffTag::Remove => (
                                    "-",
                                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                                ),
                                DiffTag::Add => (
                                    "+",
                                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                                ),
                            };
                            let lineno = match line.tag {
                                DiffTag::Add => "     ".to_string(),
                                _ => line.old_lineno
                                    .map(|n| format!("{n:>4} "))
                                    .unwrap_or_else(|| "     ".to_string()),
                            };
                            result.push(Line::from(vec![
                                Span::styled(format!("  {lineno}| {marker} "), style),
                                Span::styled(line.value.clone(), style),
                            ]));
                        }
                    }
                }
                result.push(Line::from(""));
            }
        }
    }

    if app.thinking {
        let frame = SPINNER[app.spinner_frame % SPINNER.len()];
        result.push(Line::from(Span::styled(
            format!("  {frame} Thinking…"),
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )));
    }

    result
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut result = Vec::new();
    if width == 0 {
        return result;
    }
    for raw_line in text.lines() {
        if raw_line.is_empty() {
            result.push(String::new());
            continue;
        }
        let mut current_width = 0;
        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            let wlen = word.len();
            // Break very long unspaced tokens (paths, URLs, JSON blobs) so they
            // do not overflow/cut in the terminal viewport.
            if wlen > width {
                if !current.is_empty() {
                    result.push(current.clone());
                    current.clear();
                    current_width = 0;
                }
                let chars = word.chars().collect::<Vec<_>>();
                for chunk in chars.chunks(width) {
                    result.push(chunk.iter().collect());
                }
                continue;
            }
            if current_width > 0 && current_width + 1 + wlen > width {
                result.push(current.clone());
                current.clear();
                current_width = 0;
            }
            if current_width > 0 {
                current.push(' ');
                current_width += 1;
            }
            current.push_str(word);
            current_width += wlen;
        }
        if !current.is_empty() {
            result.push(current);
        }
    }
    result
}

// ── Status footer ─────────────────────────────────────────────────────────────

fn budget_bar(pct: f32) -> (String, ratatui::style::Color) {
    let filled = ((pct / 100.0) * 8.0).round() as usize;
    let filled = filled.min(8);
    let empty = 8usize.saturating_sub(filled);
    let color = if pct >= 90.0 {
        ratatui::style::Color::Red
    } else if pct >= 80.0 {
        ratatui::style::Color::Yellow
    } else {
        ratatui::style::Color::Green
    };
    (format!("[{}{}]", "|".repeat(filled), " ".repeat(empty)), color)
}

fn draw_status(frame: &mut ratatui::Frame, area: Rect, app: &UiApp) {
    let spinner = if app.thinking {
        SPINNER[app.spinner_frame % SPINNER.len()]
    } else {
        "·"
    };
    let cost = estimate_cost(&app.model, app.input_tokens, app.output_tokens);
    let swd_str = crate::swd::SwdLevel::from_u8(app.swd_level.load(Ordering::Relaxed)).as_str();
    let budget_segment = if app.budget_enabled {
        let (bar, _) = budget_bar(app.budget_pct);
        format!(
            " · Budget {} {:.0}% · ${:.2}",
            bar, app.budget_pct, app.budget_cost_usd
        )
    } else {
        String::new()
    };
    let text = format!(
        " {spinner} Model {} · Perm {} · Tokens {}in / {}out · ${:.4} · SWD:{}{} · {}",
        app.model,
        app.permission_mode,
        app.input_tokens,
        app.output_tokens,
        cost,
        swd_str,
        budget_segment,
        short_session_id(&app.session_id),
    );
    let style = if app.read_mode {
        Style::default().fg(Color::Yellow)
    } else if app.thinking {
        Style::default().fg(Color::Blue)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let paragraph = Paragraph::new(text).style(style);
    frame.render_widget(paragraph, area);
}

fn short_session_id(id: &str) -> &str {
    id.strip_prefix("session-").unwrap_or(id)
}

fn estimate_cost(model: &str, input: u32, output: u32) -> f64 {
    let (in_rate, out_rate) = if model.contains("gpt-4") {
        (0.000_005, 0.000_015)
    } else if model.contains("sonnet") {
        (0.000_003, 0.000_015)
    } else if model.contains("haiku") {
        (0.000_000_8, 0.000_004)
    } else {
        // opus default
        (0.000_015, 0.000_075)
    };
    f64::from(input) * in_rate + f64::from(output) * out_rate
}

// ── Input box ─────────────────────────────────────────────────────────────────

fn draw_input(frame: &mut ratatui::Frame, area: Rect, app: &UiApp) {
    // In read mode show a distinct banner instead of the normal input box.
    if app.read_mode {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);
        frame.render_widget(
            Paragraph::new(Span::styled(
                "  MODO LEITURA — selecione e copie o texto livremente",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )),
            layout[0],
        );
        frame.render_widget(
            Paragraph::new("  Pressione qualquer tecla para retomar o modo TUI")
                .style(Style::default().fg(Color::DarkGray)),
            layout[1],
        );
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Indexed(215)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let hint = " / comandos · ↑/↓ histórico · F2 modelo · F3 perm · F4 sessão · Ctrl+R leitura · Ctrl+C sair";

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    // Input line with cursor.
    let before_cursor: String = app.input.chars().take(app.cursor_col).collect();
    let cursor_char: String = app
        .input
        .chars()
        .nth(app.cursor_col)
        .map(|c| c.to_string())
        .unwrap_or_else(|| " ".to_string());
    let after_cursor: String = app.input.chars().skip(app.cursor_col + 1).collect();

    let input_spans = vec![
        Span::styled("> ", Style::default().fg(Color::Indexed(215))),
        Span::styled(before_cursor, Style::default().fg(Color::White)),
        Span::styled(
            cursor_char,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Indexed(215))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(after_cursor, Style::default().fg(Color::White)),
    ];
    frame.render_widget(Paragraph::new(Line::from(input_spans)), layout[0]);

    // Hint line.
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
        layout[1],
    );
}

// ── Overlays ──────────────────────────────────────────────────────────────────

fn draw_overlay(
    frame: &mut ratatui::Frame,
    area: Rect,
    overlay: &OverlayKind,
    app: &UiApp,
) {
    match overlay {
        OverlayKind::ToolApproval {
            tool_name,
            input_preview,
            required_mode,
            ..
        } => {
            draw_tool_approval(frame, area, tool_name, input_preview, required_mode);
        }
        OverlayKind::ModelPicker {
            filter, selected, ..
        } => {
            let items = UiApp::filtered_model_list(filter);
            draw_picker(
                frame,
                area,
                "Selecione o modelo",
                &items.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                *selected,
                Some(filter),
                &format!("atual: {}", app.model),
            );
        }
        OverlayKind::PermissionPicker { items, selected } => {
            draw_picker(
                frame,
                area,
                "Modo de permissão",
                &items.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                *selected,
                None,
                &format!("atual: {}", app.permission_mode),
            );
        }
        OverlayKind::SlashPalette {
            items,
            filter,
            selected,
        } => {
            let filtered = filter_slash_items(items, filter);
            let labels: Vec<String> = filtered
                .iter()
                .map(|(cmd, desc)| format!("/{cmd:<12} {desc}"))
                .collect();
            draw_picker(
                frame,
                area,
                "Slash Commands (Ctrl+K)",
                &labels.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                *selected,
                Some(filter),
                "",
            );
        }
        OverlayKind::SessionPicker { items, selected } => {
            let labels: Vec<String> = items
                .iter()
                .map(|(id, count)| {
                    let short = id.strip_prefix("session-").unwrap_or(id);
                    format!("{short:<20} ({count} msgs)")
                })
                .collect();
            draw_picker(
                frame,
                area,
                "Sessões recentes",
                &labels.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                *selected,
                None,
                "",
            );
        }
        OverlayKind::SwdConfirmApply { action_count, .. } => {
            draw_swd_confirm(frame, area, *action_count);
        }
        OverlayKind::UninstallConfirm => {
            draw_uninstall_confirm(frame, area);
        }
        OverlayKind::SetupWizard {
            step,
            provider_sel,
            input,
            ..
        } => {
            draw_setup_wizard(frame, area, *step, *provider_sel, input);
        }
    }
}

fn draw_picker(
    frame: &mut ratatui::Frame,
    area: Rect,
    title: &str,
    items: &[&str],
    selected: usize,
    filter: Option<&str>,
    note: &str,
) {
    let width = (area.width / 2).max(50).min(area.width - 4);
    let height = (items.len() as u16 + 6).min(area.height - 4);
    let x = (area.width.saturating_sub(width)) / 2 + area.x;
    let y = (area.height.saturating_sub(height)) / 2 + area.y;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Indexed(215)));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if filter.is_some() {
            vec![
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ]
        } else {
            vec![Constraint::Min(1), Constraint::Length(1)]
        })
        .split(inner);

    let list_area = layout[0];
    let hint_area = if filter.is_some() { layout[2] } else { layout[1] };

    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            if i == selected {
                ListItem::new(format!("▶ {item}"))
                    .style(Style::default().fg(Color::Black).bg(Color::Indexed(215)))
            } else {
                ListItem::new(format!("  {item}"))
                    .style(Style::default().fg(Color::White))
            }
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(selected));
    frame.render_stateful_widget(List::new(list_items), list_area, &mut list_state);

    // Filter line.
    if let Some(f) = filter {
        let filter_area = layout[1];
        frame.render_widget(
            Paragraph::new(format!("  filtro: {f}_"))
                .style(Style::default().fg(Color::DarkGray)),
            filter_area,
        );
    }

    // Hint line.
    let hint = if note.is_empty() {
        "  ↑/↓ navegar · Enter aplicar · Esc cancelar".to_string()
    } else {
        format!("  ↑/↓ navegar · Enter aplicar · Esc cancelar  ({note})")
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
        hint_area,
    );
}

fn draw_tool_approval(
    frame: &mut ratatui::Frame,
    area: Rect,
    tool_name: &str,
    input_preview: &str,
    required_mode: &str,
) {
    let width = (area.width * 2 / 3).max(60).min(area.width - 4);
    let height = 10u16.min(area.height - 4);
    let x = (area.width.saturating_sub(width)) / 2 + area.x;
    let y = (area.height.saturating_sub(height)) / 2 + area.y;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" ⚠  Aprovar tool? ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let lines = vec![
        Line::from(vec![
            Span::styled("  Tool        ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                tool_name.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Required    ", Style::default().fg(Color::DarkGray)),
            Span::styled(required_mode.to_string(), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("  Input       ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                input_preview.chars().take(60).collect::<String>(),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  [ Y ] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled("Sim, uma vez   ", Style::default().fg(Color::Green)),
            Span::styled("[ A ] ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled("Sempre   ", Style::default().fg(Color::Cyan)),
            Span::styled("[ N ] ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::styled("Não", Style::default().fg(Color::Red)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Enter=Sim · A=Sempre · N/Esc=Não",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_swd_confirm(frame: &mut ratatui::Frame, area: Rect, action_count: usize) {
    let width = 54u16.min(area.width.saturating_sub(4));
    let height = 7u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2 + area.x;
    let y = (area.height.saturating_sub(height)) / 2 + area.y;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(format!(" \u{26a8} SWD: Aplicar {action_count} arquivo(s)? "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Indexed(215)));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  [A] ",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Aceitar    ", Style::default().fg(Color::White)),
            Span::styled(
                "[R] ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Rejeitar", Style::default().fg(Color::White)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  A/Enter = Aceitar  ·  R/Esc = Rejeitar",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_uninstall_confirm(frame: &mut ratatui::Frame, area: Rect) {
    let width = 56u16.min(area.width.saturating_sub(4));
    let height = 13u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2 + area.x;
    let y = (area.height.saturating_sub(height)) / 2 + area.y;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" ⚠  Desinstalar Elai Code ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let install_dir = std::env::var("ELAI_INSTALL_DIR").unwrap_or_else(|_| "/usr/local/bin".into());
    let home = std::env::var("HOME").unwrap_or_else(|_| "~".into());

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Serão removidos:",
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  • {install_dir}/elai"),
            Style::default().fg(Color::Red),
        )),
        Line::from(Span::styled(
            format!("  • {home}/.elai/"),
            Style::default().fg(Color::Red),
        )),
        Line::from(Span::styled(
            "  • Linhas elai-code no arquivo shell RC",
            Style::default().fg(Color::Red),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Esta ação é irreversível.",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Enter confirmar  ·  Esc cancelar",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_setup_wizard(
    frame: &mut ratatui::Frame,
    area: Rect,
    step: u8,
    provider_sel: usize,
    input: &str,
) {
    let width = 52u16.min(area.width.saturating_sub(4));
    let height = 12u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2 + area.x;
    let y = (area.height.saturating_sub(height)) / 2 + area.y;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Configurar API Key ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Indexed(215)));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let lines: Vec<Line> = if step == 0 {
        let providers = [
            ("Anthropic", "(Claude opus / sonnet / haiku)"),
            ("OpenAI", "(gpt-4o, gpt-4o-mini, o3...)"),
            ("Ambos", ""),
        ];
        let mut v: Vec<Line> = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Escolha seu provedor de IA:",
                Style::default().fg(Color::White),
            )),
            Line::from(""),
        ];
        for (i, (name, note)) in providers.iter().enumerate() {
            let selected = i == provider_sel;
            let prefix = if selected { "  \u{25b6} " } else { "    " };
            let label = format!("[{}] {:<10} {}", i + 1, name, note);
            v.push(Line::from(Span::styled(
                format!("{prefix}{label}"),
                if selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Indexed(215))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                },
            )));
        }
        v.push(Line::from(""));
        v.push(Line::from(Span::styled(
            "  \u{2191}/\u{2193} navegar \u{00b7} Enter confirmar",
            Style::default().fg(Color::DarkGray),
        )));
        v
    } else {
        let provider_name = match provider_sel {
            0 => "Anthropic",
            1 => "OpenAI",
            _ => if step == 1 { "Anthropic" } else { "OpenAI" },
        };
        let field_label = format!("  {} API key:", provider_name);
        let masked: String = "\u{2022}".repeat(input.chars().count());
        let display = format!("  > {masked}");
        vec![
            Line::from(""),
            Line::from(Span::styled(field_label, Style::default().fg(Color::White))),
            Line::from(""),
            Line::from(Span::styled(
                display,
                Style::default().fg(Color::Indexed(215)),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Enter confirmar \u{00b7} Esc cancelar",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    };

    frame.render_widget(Paragraph::new(lines), inner);
}

pub fn save_setup_keys(provider_sel: usize, key1: &str, key2: &str) -> std::io::Result<()> {
    use std::io::Write;
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME not set"))?;
    let dir = home.join(".elai");
    std::fs::create_dir_all(&dir)?;
    let env_path = dir.join(".env");

    // Read existing content to preserve other keys.
    let existing = std::fs::read_to_string(&env_path).unwrap_or_default();
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|l| {
            let k = l.trim_start_matches("export ").split('=').next().unwrap_or("");
            k != "ANTHROPIC_API_KEY" && k != "OPENAI_API_KEY"
        })
        .map(String::from)
        .collect();

    match provider_sel {
        0 => {
            lines.push(format!("ANTHROPIC_API_KEY={key1}"));
            std::env::set_var("ANTHROPIC_API_KEY", key1);
        }
        1 => {
            lines.push(format!("OPENAI_API_KEY={key1}"));
            std::env::set_var("OPENAI_API_KEY", key1);
        }
        _ => {
            lines.push(format!("ANTHROPIC_API_KEY={key1}"));
            lines.push(format!("OPENAI_API_KEY={key2}"));
            std::env::set_var("ANTHROPIC_API_KEY", key1);
            std::env::set_var("OPENAI_API_KEY", key2);
        }
    }

    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&env_path)?;
    writeln!(f, "# Elai Code \u{2014} API keys")?;
    for line in &lines {
        if !line.starts_with('#') {
            writeln!(f, "{line}")?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

// ─── Small helpers ────────────────────────────────────────────────────────────

fn whoami_user() -> String {
    env::var("USER")
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_else(|_| "User".to_string())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn make_app() -> UiApp {
        UiApp::new(
            "gpt-4o".to_string(),
            "danger-full-access".to_string(),
            "session-test".to_string(),
            vec![],
            Arc::new(AtomicU8::new(crate::swd::SwdLevel::default() as u8)),
        )
    }

    #[test]
    fn apply_text_chunk_appends_to_existing_assistant_entry() {
        let mut app = make_app();
        app.apply_tui_msg(TuiMsg::TextChunk("Hello".to_string()));
        app.apply_tui_msg(TuiMsg::TextChunk(", World".to_string()));
        assert_eq!(app.chat.len(), 1);
        if let ChatEntry::AssistantText(text) = &app.chat[0] {
            assert_eq!(text, "Hello, World");
        } else {
            panic!("expected AssistantText");
        }
    }

    #[test]
    fn apply_done_clears_thinking_flag() {
        let mut app = make_app();
        app.thinking = true;
        app.apply_tui_msg(TuiMsg::Done);
        assert!(!app.thinking);
    }

    #[test]
    fn apply_error_clears_thinking_flag_and_adds_note() {
        let mut app = make_app();
        app.thinking = true;
        app.apply_tui_msg(TuiMsg::Error("boom".to_string()));
        assert!(!app.thinking);
        assert!(!app.chat.is_empty());
        if let ChatEntry::SystemNote(note) = &app.chat[0] {
            assert!(note.contains("boom"));
        } else {
            panic!("expected SystemNote");
        }
    }

    #[test]
    fn apply_usage_accumulates_tokens() {
        let mut app = make_app();
        app.apply_tui_msg(TuiMsg::Usage { input_tokens: 100, output_tokens: 50 });
        app.apply_tui_msg(TuiMsg::Usage { input_tokens: 200, output_tokens: 75 });
        assert_eq!(app.input_tokens, 300);
        assert_eq!(app.output_tokens, 125);
    }

    #[test]
    fn tool_call_and_result_add_separate_chat_entries() {
        let mut app = make_app();
        app.apply_tui_msg(TuiMsg::ToolCall {
            name: "bash".to_string(),
            input: r#"{"command":"ls"}"#.to_string(),
        });
        app.apply_tui_msg(TuiMsg::ToolResult {
            ok: true,
            summary: "file1 file2".to_string(),
        });
        assert_eq!(app.chat.len(), 2);
        assert!(matches!(app.chat[0], ChatEntry::ToolCallEntry { .. }));
        assert!(matches!(app.chat[1], ChatEntry::ToolResultEntry { ok: true, .. }));
    }

    #[test]
    fn input_char_and_backspace_work_correctly() {
        let mut app = make_app();
        app.input_char('h');
        app.input_char('i');
        assert_eq!(app.input, "hi");
        assert_eq!(app.cursor_col, 2);
        app.input_backspace();
        assert_eq!(app.input, "h");
        assert_eq!(app.cursor_col, 1);
    }

    #[test]
    fn history_navigation_works() {
        let mut app = make_app();
        app.push_history("first".to_string());
        app.push_history("second".to_string());
        app.history_up();
        assert_eq!(app.input, "second");
        app.history_up();
        assert_eq!(app.input, "first");
        app.history_down();
        assert_eq!(app.input, "second");
        app.history_down();
        assert_eq!(app.input, "");
    }

    #[test]
    fn cursor_movement_stays_in_bounds() {
        let mut app = make_app();
        app.input = "hello".to_string();
        app.cursor_col = 5;
        app.move_cursor_right(); // should not go past 5
        assert_eq!(app.cursor_col, 5);
        app.move_cursor_home();
        assert_eq!(app.cursor_col, 0);
        app.move_cursor_left(); // should not go below 0
        assert_eq!(app.cursor_col, 0);
        app.move_cursor_end();
        assert_eq!(app.cursor_col, 5);
    }

    #[test]
    fn filtered_model_list_returns_filtered_results() {
        let all = UiApp::filtered_model_list("");
        assert!(!all.is_empty());
        let gpt = UiApp::filtered_model_list("gpt");
        assert!(gpt.iter().all(|m| m.contains("gpt")));
    }

    #[test]
    fn scroll_chat_stays_in_bounds() {
        let mut app = make_app();
        app.chat = vec![
            ChatEntry::UserMessage("a".to_string()),
            ChatEntry::UserMessage("b".to_string()),
            ChatEntry::UserMessage("c".to_string()),
        ];
        app.chat_scroll = 0;
        app.scroll_chat_up(10); // should clamp at 0
        assert_eq!(app.chat_scroll, 0);
        app.scroll_chat_down(100); // line-offset mode: clamp happens in draw_chat via max_scroll
        assert_eq!(app.chat_scroll, 100);
    }

    #[test]
    fn perm_decision_allow_sends_allow_over_channel() {
        let (tx, rx) = mpsc::sync_channel(1);
        let req = PermRequest {
            tool_name: "bash".to_string(),
            input: "ls".to_string(),
            required_mode: "workspace-write".to_string(),
            reply_tx: tx,
        };
        let mut app = make_app();
        app.overlay = Some(OverlayKind::ToolApproval {
            tool_name: req.tool_name,
            input_preview: req.input,
            required_mode: req.required_mode,
            reply_tx: req.reply_tx,
        });
        // Simulate pressing 'y'.
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('y'),
            crossterm::event::KeyModifiers::NONE,
        );
        handle_overlay_key(&mut app, key);
        let decision = rx.try_recv().expect("should have received decision");
        assert!(matches!(decision, PermDecision::Allow));
        assert!(app.overlay.is_none());
    }

    #[test]
    fn perm_decision_deny_on_escape() {
        let (tx, rx) = mpsc::sync_channel(1);
        let mut app = make_app();
        app.overlay = Some(OverlayKind::ToolApproval {
            tool_name: "bash".to_string(),
            input_preview: "rm -rf".to_string(),
            required_mode: "danger-full-access".to_string(),
            reply_tx: tx,
        });
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        );
        handle_overlay_key(&mut app, key);
        let decision = rx.try_recv().expect("should have received decision");
        assert!(matches!(decision, PermDecision::Deny));
    }
}
