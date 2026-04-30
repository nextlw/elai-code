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
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEventKind,
};
use pulldown_cmark::{CodeBlockKind, Event as MdEvent, Options, Parser, Tag, TagEnd};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme as SyntectTheme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Terminal;

use commands::SlashCategory;

use crate::render::{ColorTheme, RatatuiTheme};

/// Acesso ao tema único da TUI.
///
/// **Fonte de verdade**: [`crate::render::ColorTheme`] (em `crossterm::Color`).
/// Esta função apenas projeta o tema para `ratatui::style::Color` via
/// [`ColorTheme::for_tui`]. **Não use literais `Color::Xxx` em `tui.rs`** —
/// se faltar token, adicione-o em `ColorTheme` antes de consumir aqui.
///
/// Cacheado em `OnceLock<Mutex<_>>` para evitar ler config/env em todo render.
/// Use [`refresh_theme_cache`] quando um comando de runtime alterar o tema.
fn theme_cache() -> &'static Mutex<RatatuiTheme> {
    static THEME_CACHE: OnceLock<Mutex<RatatuiTheme>> = OnceLock::new();
    THEME_CACHE.get_or_init(|| Mutex::new(ColorTheme::resolved().for_tui()))
}

#[inline]
fn theme() -> RatatuiTheme {
    *theme_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Atualiza o cache de tema com as cores resolvidas do ambiente.
///
/// Chamado pelo runtime quando um comando altera o tema (ex: `/theme gray 240`).
/// Re-resolve `ColorTheme` e projeta para `RatatuiTheme` via cache singleton.
pub fn refresh_theme_cache() {
    let mut guard = theme_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = ColorTheme::resolved().for_tui();
}

// ─── Inter-thread message types ──────────────────────────────────────────────

/// Events sent from the background runtime thread to the TUI main thread.
#[derive(Debug)]
pub enum TuiMsg {
    TextChunk(String),
    ThinkingChunk(String),
    ToolCall { name: String, input: String },
    ToolResult { ok: bool },
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
    /// Update da "linha viva" de uma task. O TUI substitui in-place a entry
    /// existente com mesmo `task_id` (scan reverso ≤ 8 entries) ou faz push.
    TaskProgress {
        task_id: String,
        label: String,
        msg: String,
    },
    /// Sinaliza fim da task. Marca a entry como `finished` e armazena o status.
    TaskProgressEnd {
        task_id: String,
        label: String,
        status: runtime::TaskStatus,
        summary: Option<String>,
    },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolItemStatus {
    Running,
    Ok,
    Err,
}

#[derive(Debug, Clone)]
pub struct ToolBatchItem {
    /// Nome da ferramenta (ex: `bash`, `read_file`).
    pub name: String,
    /// Resumo legível do input — extraído via `tool_input_one_line` (mostra
    /// `cd /foo` em vez do JSON literal `{"command":"cd /foo"}`).
    pub input_summary: String,
    pub status: ToolItemStatus,
}

#[derive(Debug, Clone)]
pub enum ChatEntry {
    UserMessage(String),
    AssistantText(String),
    /// Bloco agrupado de tool calls executadas em sequência. Itens recém-chegados
    /// são `Running` (spinner) e migram para `Ok`/`Err` quando o resultado
    /// correspondente chega. Qualquer entry de outro tipo "interrompe" o
    /// agrupamento (`closed = true`); o próximo `TuiMsg::ToolCall` abre um
    /// bloco novo.
    ToolBatchEntry {
        items: Vec<ToolBatchItem>,
        closed: bool,
    },
    /// Bloco de raciocínio interno (extended thinking). Acumula enquanto
    /// `finished = false`; congela quando texto ou Done/Error chega.
    ThinkingBlock {
        text: String,
        finished: bool,
    },
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
    /// Linha viva de uma task. Mutável até `finished = true`; depois congela.
    /// Para tasks "multi-line" (ex.: DeepResearch), `events` acumula o histórico
    /// e é renderizado como bloco; tasks single-line usam apenas `msg`.
    TaskProgress {
        task_id: String,
        label: String,
        msg: String,
        events: Vec<String>,
        finished: bool,
        status: Option<runtime::TaskStatus>,
    },
}

// ─── Overlay states ───────────────────────────────────────────────────────────

#[derive(Debug)]
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
    LocalePicker {
        items: Vec<String>,
        selected: usize,
    },
    SlashPalette {
        items: Vec<(SlashCategory, String, String)>,
        filter: String,
        selected: usize,
    },
    FileMentionPicker {
        items: Vec<String>,    // paths relativos do projeto (cache)
        filter: String,        // texto após o `@` (live)
        selected: usize,       // índice na lista filtrada
        anchor_pos: usize,     // posição do `@` no input (em chars, não bytes)
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
    /// Legacy OpenAI key setup wizard (kept for compatibility, accessible via `/keys` if needed).
    #[allow(dead_code)]
    SetupWizard {
        step: u8,
        provider_sel: usize,
        key1: String,
        key2: String,
        input: String,
        cursor: usize,
    },
    AuthPicker {
        step: AuthStep,
    },
    /// Multi-step first-run setup wizard (model + permissions + defaults).
    FirstRunWizard {
        step: WizardStep,
        state: WizardState,
    },
    /// Modal para colar a API key do DeepResearch (input mascarado).
    DeepResearchKeyInput {
        input: String,
        cursor: usize,
    },
}

/// Which authentication method the user selected in the AuthPicker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethodChoice {
    ClaudeAiOAuth,
    ConsoleOAuth,
    SsoOAuth,
    PasteApiKey,
    PasteAuthToken,
    /// API key da OpenAI (sk-... ou sk-proj-...). Salva como
    /// `AuthMethod::OpenAiApiKey` no credentials store.
    PasteOpenAiKey,
    UseBedrock,
    UseVertex,
    UseFoundry,
    ImportClaudeCode,
    LegacyElai,
}

/// Step state machine for the AuthPicker overlay.
#[derive(Debug)]
pub enum AuthStep {
    /// List of methods; `selected` is index in the visible (filtered) list.
    MethodList {
        selected: usize,
        claude_code_detected: bool,
    },
    /// Collect e-mail (SSO only).
    EmailInput {
        method: AuthMethodChoice,
        input: String,
        cursor: usize,
    },
    /// Paste an API key or auth token.
    PasteSecret {
        method: AuthMethodChoice,
        input: String,
        cursor: usize,
        masked: bool,
    },
    /// OAuth browser flow in progress.
    BrowserFlow {
        method: AuthMethodChoice,
        url: String,
        port: u16,
        started_at: std::time::Instant,
        rx: std::sync::mpsc::Receiver<AuthEvent>,
        cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    },
    /// Confirmation for 3P toggle.
    Confirm3p {
        method: AuthMethodChoice,
        env_var: &'static str,
    },
    /// Success: show summary.
    Done {
        label: String,
    },
    /// Error: show message and ask Esc/Enter.
    Failed {
        error: String,
    },
}

/// Events sent from the OAuth background thread to the TUI.
#[derive(Debug)]
pub enum AuthEvent {
    #[allow(dead_code)]
    Progress(String),
    Success(String),
    Error(String),
}

// ─── First-run setup wizard types ────────────────────────────────────────────

/// State collected while the setup wizard is running.
#[derive(Debug, Clone)]
pub struct WizardState {
    pub model: String,
    pub permission_mode: String,
    pub features: runtime::FeatureFlags,
}

impl Default for WizardState {
    fn default() -> Self {
        Self {
            model: "claude-opus-4-6".into(),
            permission_mode: "workspace-write".into(),
            features: runtime::FeatureFlags::default(),
        }
    }
}

/// Step state machine for the first-run setup wizard.
#[derive(Debug, Clone)]
pub enum WizardStep {
    /// Welcome screen — press Enter to continue.
    Welcome,
    /// Model selection (4 choices).
    Model { selected: usize },
    /// Permission mode selection (3 choices).
    Permissions { selected: usize },
    /// Optional defaults toggle (auto-update / telemetry / indexing).
    /// `focused` is 0..2 indicating which toggle the cursor is on.
    Defaults { focused: usize },
    /// Summary + persist — press Enter to close.
    Done,
}

// ─── Helpers para in-place update de progresso de tasks ─────────────────────

/// Casa apenas com `ChatEntry::TaskProgress` que ainda não foi finalizada e
/// pertence à task com `task_id` informado. Usado pelo scan reverso do
/// `apply_tui_msg` ao receber `TuiMsg::TaskProgress{,End}`.
fn matches_task_progress(entry: &ChatEntry, task_id: &str) -> bool {
    matches!(
        entry,
        ChatEntry::TaskProgress {
            task_id: tid,
            finished: false,
            ..
        } if tid == task_id
    )
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
    pub indexed_paths: Vec<String>, // cache lazy de `.elai/index/metadata.json` ou re-walk
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
    /// Onboarding: dicas exibidas quando o chat ainda está vazio.
    pub tips: Vec<crate::tips::Tip>,
    pub tips_order: Vec<usize>,
    pub tips_cursor: usize,
    /// `false` após o usuário enviar a primeira mensagem; `true` novamente após `/clear`.
    pub show_tips: bool,
    /// Mensagens digitadas enquanto `thinking = true`, aguardando envio.
    pub message_queue: std::collections::VecDeque<String>,
    /// Próxima mensagem a ser despachada logo que `thinking` voltar a `false`.
    pub pending_outgoing: Option<String>,
    /// `true` se a mensagem atual contém a keyword `ultrathink`.
    pub ultrathink_active: bool,
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
            indexed_paths: Vec::new(),
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
            tips: {
                let loaded = crate::tips::load_tips();
                loaded
            },
            tips_order: Vec::new(),
            tips_cursor: 0,
            show_tips: true,
            message_queue: std::collections::VecDeque::new(),
            pending_outgoing: None,
            ultrathink_active: false,
        }
        .with_shuffled_tips()
    }

    fn with_shuffled_tips(mut self) -> Self {
        self.tips_order = crate::tips::shuffle_indices(self.tips.len());
        self.tips_cursor = 0;
        self
    }

    /// Re-embaralha as dicas e reativa o overlay (chamado no `/clear` e Ctrl+L).
    pub fn reset_tips(&mut self) {
        self.tips_order = crate::tips::shuffle_indices(self.tips.len());
        self.tips_cursor = 0;
        self.show_tips = true;
    }

    /// Avança para a próxima dica (wrap-around).
    pub fn next_tip(&mut self) {
        if self.tips_order.is_empty() {
            return;
        }
        self.tips_cursor = (self.tips_cursor + 1) % self.tips_order.len();
    }

    /// Volta para a dica anterior (wrap-around).
    pub fn prev_tip(&mut self) {
        if self.tips_order.is_empty() {
            return;
        }
        self.tips_cursor = if self.tips_cursor == 0 {
            self.tips_order.len() - 1
        } else {
            self.tips_cursor - 1
        };
    }

    /// Dica atual + posição (`current_index_1based`, `total`). Retorna `None`
    /// se não houver dicas carregadas.
    pub fn current_tip(&self) -> Option<(&crate::tips::Tip, usize, usize)> {
        let idx = *self.tips_order.get(self.tips_cursor)?;
        let tip = self.tips.get(idx)?;
        Some((tip, self.tips_cursor + 1, self.tips_order.len()))
    }

    pub fn tick(&mut self) {
        if self.thinking {
            self.spinner_frame = self.spinner_frame.wrapping_add(1);
        }
    }

    pub fn push_chat(&mut self, entry: ChatEntry) {
        if matches!(entry, ChatEntry::UserMessage(_)) {
            self.show_tips = false;
        }
        // Qualquer entry que NÃO seja um item de tool batch fecha o batch
        // aberto na cauda do chat — o próximo tool call iniciará um bloco novo.
        if !matches!(entry, ChatEntry::ToolBatchEntry { .. }) {
            self.close_open_tool_batch();
        }
        // Fecha ThinkingBlock aberto quando outra entry que não seja ThinkingBlock chega.
        if !matches!(entry, ChatEntry::ThinkingBlock { .. }) {
            if let Some(ChatEntry::ThinkingBlock { finished, .. }) = self.chat.last_mut() {
                *finished = true;
            }
        }
        self.chat.push(entry);
        self.scroll_to_bottom();
    }

    /// Marca o último `ToolBatchEntry` aberto como `closed = true`. Idempotente.
    fn close_open_tool_batch(&mut self) {
        if let Some(ChatEntry::ToolBatchEntry { closed, .. }) = self.chat.last_mut() {
            *closed = true;
        }
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
                // Fecha bloco de thinking quando texto narrativo começa.
                if let Some(ChatEntry::ThinkingBlock { finished, .. }) = self.chat.last_mut() {
                    *finished = true;
                }
                // Texto do agente entra direto na entry `AssistantText` aberta
                // (caso comum: chunks fragmentados do mesmo parágrafo) ou cria
                // uma nova entry. Texto entre tools NÃO é tratado como
                // "pensamento" — é resposta normal multi-linha do agente.
                if let Some(ChatEntry::AssistantText(ref mut buf)) = self.chat.last_mut() {
                    buf.push_str(&text);
                } else {
                    // Nova entry de texto → fecha o tool batch aberto, se houver.
                    self.close_open_tool_batch();
                    self.chat.push(ChatEntry::AssistantText(text));
                }
                self.scroll_to_bottom();
            }
            TuiMsg::ThinkingChunk(text) => {
                if let Some(ChatEntry::ThinkingBlock { text: buf, finished: false }) =
                    self.chat.last_mut()
                {
                    buf.push_str(&text);
                } else {
                    self.chat.push(ChatEntry::ThinkingBlock { text, finished: false });
                    self.scroll_to_bottom();
                }
            }
            TuiMsg::ToolCall { name, input } => {
                // Resumo legível: usa `tool_input_one_line` para extrair o
                // command/path/query relevante (em vez do JSON literal cru).
                let input_summary = crate::tool_input_one_line(&name, &input);
                let item = ToolBatchItem {
                    name,
                    input_summary,
                    status: ToolItemStatus::Running,
                };
                if let Some(ChatEntry::ToolBatchEntry { items, closed: false }) =
                    self.chat.last_mut()
                {
                    items.push(item);
                } else {
                    self.chat.push(ChatEntry::ToolBatchEntry {
                        items: vec![item],
                        closed: false,
                    });
                }
                self.scroll_to_bottom();
            }
            TuiMsg::ToolResult { ok } => {
                // Tool result chega imediatamente após o tool call no mesmo
                // turn — o último entry deve ser o ToolBatchEntry onde o call
                // foi inserido. Marca o último item ainda Running.
                if let Some(ChatEntry::ToolBatchEntry { items, .. }) = self.chat.last_mut() {
                    if let Some(item) = items
                        .iter_mut()
                        .rev()
                        .find(|it| it.status == ToolItemStatus::Running)
                    {
                        item.status = if ok { ToolItemStatus::Ok } else { ToolItemStatus::Err };
                    }
                }
                self.scroll_to_bottom();
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
                self.ultrathink_active = false;
                if let Some(ChatEntry::ThinkingBlock { finished, .. }) = self.chat.last_mut() {
                    *finished = true;
                }
                if let Some(next) = self.message_queue.pop_front() {
                    self.pending_outgoing = Some(next);
                }
            }
            TuiMsg::Error(msg) => {
                self.thinking = false;
                self.ultrathink_active = false;
                if let Some(ChatEntry::ThinkingBlock { finished, .. }) = self.chat.last_mut() {
                    *finished = true;
                }
                self.push_chat(ChatEntry::SystemNote(format!("❌ Error: {msg}")));
                if let Some(next) = self.message_queue.pop_front() {
                    self.pending_outgoing = Some(next);
                }
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
                if let Some(next) = self.message_queue.pop_front() {
                    self.pending_outgoing = Some(next);
                }
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
            TuiMsg::TaskProgress { task_id, label, msg } => {
                let multiline = is_multiline_task(&label);
                // Scan reverso curto (≤ 8 entries) — cobre o caso comum onde
                // tool calls / assistant text se intercalam com updates.
                let found = self
                    .chat
                    .iter_mut()
                    .rev()
                    .take(8)
                    .find(|e| matches_task_progress(e, &task_id));
                match found {
                    Some(ChatEntry::TaskProgress {
                        msg: m,
                        label: l,
                        events,
                        ..
                    }) => {
                        if multiline {
                            // Acumula no histórico, dedup do último.
                            if events.last().map(String::as_str) != Some(msg.as_str()) {
                                events.push(msg.clone());
                            }
                        }
                        *m = msg;
                        *l = label;
                    }
                    _ => {
                        let events = if multiline { vec![msg.clone()] } else { Vec::new() };
                        self.push_chat(ChatEntry::TaskProgress {
                            task_id,
                            label,
                            msg,
                            events,
                            finished: false,
                            status: None,
                        });
                    }
                }
                self.scroll_to_bottom();
            }
            TuiMsg::TaskProgressEnd {
                task_id,
                label,
                status,
                summary,
            } => {
                if let Some(ChatEntry::TaskProgress {
                    msg,
                    label: l,
                    finished,
                    status: s,
                    ..
                }) = self
                    .chat
                    .iter_mut()
                    .rev()
                    .take(8)
                    .find(|e| matches_task_progress(e, &task_id))
                {
                    if let Some(sum) = summary {
                        *msg = sum;
                    }
                    *l = label;
                    *finished = true;
                    *s = Some(status);
                    self.scroll_to_bottom();
                }
                // Se nada casou, a task encerrou sem ter emitido — ignora.
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
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001",
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

    pub fn open_locale_picker(&mut self, locales: Vec<String>, current: &str) {
        let selected = locales.iter().position(|s| s == current).unwrap_or(0);
        self.overlay = Some(OverlayKind::LocalePicker { items: locales, selected });
    }

    pub fn open_slash_palette(&mut self) {
        let items = slash_palette_items();
        let initial_rows = build_palette_rows(&items, "");
        let selected = first_selectable_row(&initial_rows);
        self.overlay = Some(OverlayKind::SlashPalette {
            filter: String::new(),
            items,
            selected,
        });
    }

    pub fn open_session_picker(&mut self) {
        self.overlay = Some(OverlayKind::SessionPicker {
            items: self.recent_sessions.clone(),
            selected: 0,
        });
    }

    pub fn open_file_mention_picker(&mut self, cwd: &std::path::Path, anchor_pos: usize) {
        if self.indexed_paths.is_empty() {
            self.indexed_paths = load_indexed_paths(cwd);
        }
        self.overlay = Some(OverlayKind::FileMentionPicker {
            items: self.indexed_paths.clone(),
            filter: String::new(),
            selected: 0,
            anchor_pos,
        });
    }

    #[allow(dead_code)]
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

    pub fn open_auth_picker(&mut self) {
        let detected = runtime::detect_claude_code_credentials().is_some();
        self.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::MethodList {
                selected: 0,
                claude_code_detected: detected,
            },
        });
    }

    pub fn open_first_run_wizard(&mut self) {
        self.overlay = Some(OverlayKind::FirstRunWizard {
            step: WizardStep::Welcome,
            state: WizardState::default(),
        });
    }

    pub fn open_uninstall_confirm(&mut self) {
        self.overlay = Some(OverlayKind::UninstallConfirm);
    }

    pub fn open_deepresearch_key_input(&mut self) {
        self.overlay = Some(OverlayKind::DeepResearchKeyInput {
            input: String::new(),
            cursor: 0,
        });
    }
}

/// Tradução PT-BR para cada slash command conhecido. Comandos sem entrada
/// caem no `spec.summary` (inglês) — garante visibilidade mesmo de novos
/// `SLASH_COMMAND_SPECS` que ainda não foram traduzidos.
fn slash_command_pt_description(name: &str) -> Option<&'static str> {
    Some(match name {
        "help" => "Mostrar ajuda",
        "status" => "Status da sessão",
        "compact" => "Compactar histórico",
        "model" => "Mostrar/trocar modelo",
        "permissions" => "Mostrar/trocar permissões",
        "clear" => "Limpar histórico",
        "cost" => "Mostrar custo",
        "resume" => "Carregar sessão salva",
        "config" => "Inspecionar configuração Elai",
        "memory" => "Mostrar ELAI.md",
        "init" => "Inicializar projeto",
        "diff" => "Mostrar git diff",
        "version" => "Mostrar versão",
        "update" => "Atualizar Elai Code",
        "bughunter" => "Caçar bugs no codebase",
        "branch" => "Listar/criar/trocar branches",
        "worktree" => "Gerenciar worktrees git",
        "commit" => "Gerar mensagem e commitar",
        "commit-push-pr" => "Commit, push e abrir PR",
        "pr" => "Rascunhar/abrir pull request",
        "issue" => "Rascunhar/abrir GitHub issue",
        "ultraplan" => "Plano profundo (multi-step)",
        "teleport" => "Saltar para arquivo/símbolo",
        "debug-tool-call" => "Replay do último tool call",
        "export" => "Exportar conversa",
        "session" => "Listar/trocar sessões",
        "plugin" => "Gerenciar plugins",
        "agents" => "Listar agents configurados",
        "skills" => "Listar skills disponíveis",
        "budget" => "Budget limiter (tokens/custo)",
        "tools" => "Inspecionar tools da sessão",
        "cache" => "Gerenciar cache de resposta",
        "dream" => "Comprimir memória antiga (AI)",
        "stats" => "Estatísticas de tokens/custo",
        "providers" => "Painel de uso por provider",
        "verify" => "Verificar codebase vs memória",
        "theme" => "Ajustar tema (cinza secundário)",
        _ => return None,
    })
}

/// Constrói a lista da paleta a partir de `slash_command_specs()` (respeitando
/// `hidden` e `is_enabled`) e acrescenta os 4 comandos sintetizados pelo REPL
/// que não vivem como `SlashCommandSpec` (`swd`, `keys`, `uninstall`, `exit`).
fn slash_palette_items() -> Vec<(SlashCategory, String, String)> {
    let mut items: Vec<(SlashCategory, String, String)> = commands::slash_command_specs()
        .iter()
        .filter(|spec| !spec.hidden && (spec.is_enabled)())
        .map(|spec| {
            let display = spec.user_facing_name.unwrap_or(spec.name);
            let desc = slash_command_pt_description(spec.name)
                .map_or_else(|| spec.summary(), str::to_string);
            (spec.category, display.to_string(), desc)
        })
        .collect();

    // Comandos REPL-local (não vivem em SLASH_COMMAND_SPECS).
    items.extend([
        (
            SlashCategory::Behavior,
            "swd".into(),
            "Strict Write Discipline (off/partial/full)".into(),
        ),
        (
            SlashCategory::Behavior,
            "keys".into(),
            "Configurar/trocar API keys".into(),
        ),
        (
            SlashCategory::Behavior,
            "theme".into(),
            "Ajustar tema: /theme gray <232-255>".into(),
        ),
        (
            SlashCategory::System,
            "uninstall".into(),
            "Desinstalar Elai Code".into(),
        ),
        (SlashCategory::Session, "exit".into(), "Sair".into()),
    ]);

    items
}

/// Comandos cuja implementação no modo TUI ainda é placeholder ("em breve").
/// A paleta os exibe em cor mais apagada e com sufixo "(em breve)" para
/// sinalizar visualmente que dispará-los só emite uma mensagem informativa.
fn is_command_coming_soon(name: &str) -> bool {
    matches!(
        name,
        "bughunter"
            | "ultraplan"
            | "teleport"
            | "commit"
            | "commit-push-pr"
            | "pr"
            | "issue"
    )
}

/// Rótulo PT-BR + emoji para o cabeçalho da seção na paleta Ctrl+K.
fn category_label_pt(cat: SlashCategory) -> &'static str {
    match cat {
        SlashCategory::Session => "🗨  Sessão",
        SlashCategory::Behavior => "⚙  Comportamento",
        SlashCategory::Project => "📁 Projeto",
        SlashCategory::Git => "🌿 Git",
        SlashCategory::Analysis => "🔍 Análise",
        SlashCategory::System => "🛠  Sistema",
        SlashCategory::Plugins => "🧩 Plugins",
        SlashCategory::Custom => "✨ Custom",
    }
}

/// Linha renderizada na paleta — separa cabeçalho de comando.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PaletteRow {
    Header(String),
    Command { cmd: String, desc: String },
}

/// Constrói o vetor de linhas da paleta agrupando por categoria, respeitando o filtro.
fn build_palette_rows(
    items: &[(SlashCategory, String, String)],
    filter: &str,
) -> Vec<PaletteRow> {
    type CatBucket<'a> = (SlashCategory, Vec<(&'a String, &'a String)>);
    let filtered = filter_slash_items(items, filter);
    let mut by_cat: std::collections::BTreeMap<u8, CatBucket> =
        std::collections::BTreeMap::new();
    for (cat, cmd, desc) in &filtered {
        by_cat
            .entry(cat.order())
            .or_insert_with(|| (*cat, Vec::new()))
            .1
            .push((cmd, desc));
    }
    let mut rows = Vec::new();
    for (_, (cat, cmds)) in by_cat {
        if cmds.is_empty() {
            continue;
        }
        rows.push(PaletteRow::Header(category_label_pt(cat).into()));
        for (cmd, desc) in cmds {
            rows.push(PaletteRow::Command {
                cmd: cmd.clone(),
                desc: desc.clone(),
            });
        }
    }
    rows
}

/// Próximo índice selecionável (pulando `Header`). Mantém posição se já está no fim.
fn next_selectable_row(rows: &[PaletteRow], from: usize) -> usize {
    let mut i = from.saturating_add(1);
    while i < rows.len() {
        if matches!(rows[i], PaletteRow::Command { .. }) {
            return i;
        }
        i += 1;
    }
    from
}

/// Anterior selecionável (pulando `Header`). Mantém posição se já está no topo.
fn prev_selectable_row(rows: &[PaletteRow], from: usize) -> usize {
    if from == 0 {
        return 0;
    }
    let mut i = from - 1;
    loop {
        if matches!(rows[i], PaletteRow::Command { .. }) {
            return i;
        }
        if i == 0 {
            return from;
        }
        i -= 1;
    }
}

/// Primeiro `Command` (pulando `Header` líder). Retorna 0 se não houver comandos.
fn first_selectable_row(rows: &[PaletteRow]) -> usize {
    rows.iter()
        .position(|r| matches!(r, PaletteRow::Command { .. }))
        .unwrap_or(0)
}

// ─── Terminal lifecycle helpers ───────────────────────────────────────────────

/// Enter alternate screen + raw mode with mouse capture for scroll events.
/// Text selection still works via Shift+drag in most terminals.
pub fn enter_tui(stdout: &mut impl io::Write) -> io::Result<()> {
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(())
}

/// Restore terminal on exit (always call even on error).
pub fn leave_tui(stdout: &mut impl io::Write) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(stdout, DisableMouseCapture, LeaveAlternateScreen)?;
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
    CopyToClipboard(String),
    EnterReadMode,
    ExitReadMode,
    SetupComplete,
    AuthComplete { label: String },
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

    // Despacha mensagem enfileirada assim que thinking voltou a false.
    if let Some(msg) = app.pending_outgoing.take() {
        app.ultrathink_active = msg.to_lowercase().contains("ultrathink");
        return TuiAction::SendMessage(msg);
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
            app.reset_tips();
            return TuiAction::None;
        }
        // Navegação entre dicas: setas simples quando o overlay de tips está
        // visível e o input está vazio (cursor não tem onde ir nesse estado,
        // então não conflita com o handler genérico de cursor).
        // Evitamos `Ctrl+arrow` porque o macOS intercepta para troca de Spaces.
        (KeyModifiers::NONE, KeyCode::Right)
            if app.show_tips && app.chat.is_empty() && app.input.is_empty() =>
        {
            app.next_tip();
            return TuiAction::None;
        }
        (KeyModifiers::NONE, KeyCode::Left)
            if app.show_tips && app.chat.is_empty() && app.input.is_empty() =>
        {
            app.prev_tip();
            return TuiAction::None;
        }
        (KeyModifiers::CONTROL, KeyCode::Char('k')) => {
            app.open_slash_palette();
            return TuiAction::None;
        }
        (KeyModifiers::CONTROL, KeyCode::Char('r')) => {
            return TuiAction::EnterReadMode;
        }
        // Ctrl+Y: copia a última mensagem do assistente para a área de transferência.
        (KeyModifiers::CONTROL, KeyCode::Char('y')) => {
            let text = app
                .chat
                .iter()
                .rev()
                .find_map(|e| {
                    if let ChatEntry::AssistantText(t) = e {
                        Some(t.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            if !text.is_empty() {
                return TuiAction::CopyToClipboard(text);
            }
            return TuiAction::None;
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

    match (key.modifiers, key.code) {
        // Submit
        (KeyModifiers::NONE, KeyCode::Enter) => {
            let text = app.input.trim().to_string();
            if text.is_empty() {
                return TuiAction::None;
            }
            app.push_history(text.clone());

            // Se o runtime está processando, enfileira a mensagem para envio posterior.
            if app.thinking {
                app.clear_input();
                let pos = app.message_queue.len() + 1;
                app.message_queue.push_back(text);
                app.push_chat(ChatEntry::SystemNote(format!(
                    "📥 Mensagem adicionada à fila (posição {pos})"
                )));
                return TuiAction::None;
            }

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

            app.ultrathink_active = text.to_lowercase().contains("ultrathink");

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
        (KeyModifiers::SHIFT, KeyCode::Up) => {
            app.history_up();
            TuiAction::None
        }
        (KeyModifiers::SHIFT, KeyCode::Down) => {
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
            if c == '@' {
                // Insert '@' then open file mention picker (works at any cursor position)
                let anchor_pos = app.cursor_col;
                app.input_char('@');
                app.open_file_mention_picker(
                    &std::env::current_dir().unwrap_or_default(),
                    anchor_pos,
                );
                return TuiAction::None;
            }
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

        Some(OverlayKind::LocalePicker {
            items,
            mut selected,
        }) => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Esc) => {
                    app.overlay = None;
                }
                (KeyModifiers::NONE, KeyCode::Up) => {
                    selected = selected.saturating_sub(1);
                    app.overlay = Some(OverlayKind::LocalePicker { items, selected });
                }
                (KeyModifiers::NONE, KeyCode::Down) => {
                    selected = (selected + 1).min(items.len().saturating_sub(1));
                    app.overlay = Some(OverlayKind::LocalePicker { items, selected });
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    if let Some(lang) = items.get(selected) {
                        let cmd = format!("/locale {lang}");
                        app.overlay = None;
                        return TuiAction::SlashCommand(cmd);
                    }
                    app.overlay = None;
                }
                _ => {
                    app.overlay = Some(OverlayKind::LocalePicker { items, selected });
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
                    let rows = build_palette_rows(&items, &filter);
                    selected = prev_selectable_row(&rows, selected);
                    app.overlay = Some(OverlayKind::SlashPalette { items, filter, selected });
                }
                (KeyModifiers::NONE, KeyCode::Down) => {
                    let rows = build_palette_rows(&items, &filter);
                    selected = next_selectable_row(&rows, selected);
                    app.overlay = Some(OverlayKind::SlashPalette { items, filter, selected });
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    let rows = build_palette_rows(&items, &filter);
                    if let Some(PaletteRow::Command { cmd, .. }) = rows.get(selected) {
                        let cmd = cmd.clone();
                        app.overlay = None;
                        app.clear_input();
                        return TuiAction::SlashCommand(format!("/{cmd}"));
                    }
                    // Se o filtro contém espaço, tenta "/<cmd> <arg>" — suporte a argumentos inline.
                    let stripped = filter.trim_start_matches('/');
                    if let Some((cmd_name, arg)) = stripped.split_once(' ') {
                        let arg = arg.trim();
                        if !arg.is_empty() {
                            let cmd_rows = build_palette_rows(&items, cmd_name);
                            let exact = cmd_rows.iter().find_map(|r| {
                                if let PaletteRow::Command { cmd, .. } = r {
                                    if cmd.as_str() == cmd_name { Some(cmd.clone()) } else { None }
                                } else {
                                    None
                                }
                            });
                            if let Some(cmd) = exact {
                                app.overlay = None;
                                app.clear_input();
                                return TuiAction::SlashCommand(format!("/{cmd} {arg}"));
                            }
                        }
                    }
                    // Header selecionado ou lista vazia → no-op (não fecha overlay).
                    app.overlay = Some(OverlayKind::SlashPalette { items, filter, selected });
                }
                (KeyModifiers::NONE, KeyCode::Esc) => {
                    app.overlay = None;
                    app.clear_input();
                }
                (KeyModifiers::NONE, KeyCode::Backspace) => {
                    filter.pop();
                    let rows = build_palette_rows(&items, &filter);
                    selected = first_selectable_row(&rows);
                    app.overlay = Some(OverlayKind::SlashPalette { items, filter, selected });
                }
                (_, KeyCode::Char(c)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let c = if c == '/' && filter.is_empty() { c } else { c };
                    filter.push(c);
                    let rows = build_palette_rows(&items, &filter);
                    selected = first_selectable_row(&rows);
                    app.overlay = Some(OverlayKind::SlashPalette { items, filter, selected });
                }
                _ => {
                    app.overlay = Some(OverlayKind::SlashPalette { items, filter, selected });
                }
            }
            TuiAction::None
        }

        Some(OverlayKind::FileMentionPicker {
            items,
            filter,
            selected,
            anchor_pos,
        }) => {
            let filtered = filter_mention_items(&items, &filter);
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Esc) => {
                    app.overlay = None;
                }
                (KeyModifiers::NONE, KeyCode::Up) => {
                    let new_sel = if selected == 0 {
                        filtered.len().saturating_sub(1)
                    } else {
                        selected - 1
                    };
                    app.overlay = Some(OverlayKind::FileMentionPicker {
                        items,
                        filter,
                        selected: new_sel,
                        anchor_pos,
                    });
                }
                (KeyModifiers::NONE, KeyCode::Down) => {
                    let new_sel = if filtered.is_empty() {
                        0
                    } else {
                        (selected + 1) % filtered.len()
                    };
                    app.overlay = Some(OverlayKind::FileMentionPicker {
                        items,
                        filter,
                        selected: new_sel,
                        anchor_pos,
                    });
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    if let Some(path) = filtered.get(selected).copied() {
                        let path_s = path.to_string();
                        let insert_pos = anchor_pos + 1; // após o `@`
                        let byte_idx = app
                            .input
                            .char_indices()
                            .nth(insert_pos)
                            .map(|(i, _)| i)
                            .unwrap_or(app.input.len());
                        let chars_inserted = path_s.chars().count() + 1; // +1 do espaço
                        app.input.insert_str(byte_idx, &format!("{path_s} "));
                        app.cursor_col = insert_pos + chars_inserted;
                    }
                    app.overlay = None;
                }
                (KeyModifiers::NONE, KeyCode::Backspace) => {
                    if filter.is_empty() {
                        // Remove o @ e fecha overlay
                        let byte_idx = app
                            .input
                            .char_indices()
                            .nth(anchor_pos)
                            .map(|(i, _)| i);
                        if let Some(idx) = byte_idx {
                            if app.input[idx..].starts_with('@') {
                                app.input.remove(idx);
                                if app.cursor_col > anchor_pos {
                                    app.cursor_col -= 1;
                                }
                            }
                        }
                        app.overlay = None;
                    } else {
                        let mut new_filter = filter.clone();
                        new_filter.pop();
                        app.overlay = Some(OverlayKind::FileMentionPicker {
                            items,
                            filter: new_filter,
                            selected: 0,
                            anchor_pos,
                        });
                    }
                }
                (_, KeyCode::Char(c)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let mut new_filter = filter.clone();
                    new_filter.push(c);
                    app.overlay = Some(OverlayKind::FileMentionPicker {
                        items,
                        filter: new_filter,
                        selected: 0,
                        anchor_pos,
                    });
                }
                _ => {
                    app.overlay = Some(OverlayKind::FileMentionPicker {
                        items,
                        filter,
                        selected,
                        anchor_pos,
                    });
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

        Some(OverlayKind::AuthPicker { step }) => {
            handle_auth_picker_key(app, key, step)
        }

        Some(OverlayKind::FirstRunWizard { step, state }) => {
            handle_first_run_wizard_key(app, key, step, state)
        }

        Some(OverlayKind::DeepResearchKeyInput { mut input, mut cursor }) => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Esc) => {
                    app.overlay = None;
                    TuiAction::None
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    let trimmed = input.trim().to_string();
                    if trimmed.is_empty() {
                        app.overlay =
                            Some(OverlayKind::DeepResearchKeyInput { input, cursor });
                        TuiAction::None
                    } else {
                        app.overlay = None;
                        TuiAction::SlashCommand(format!("/deepresearch {}", trimmed))
                    }
                }
                (KeyModifiers::NONE, KeyCode::Backspace)
                | (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
                    if cursor > 0 {
                        cursor -= 1;
                        let idx = input
                            .char_indices()
                            .nth(cursor)
                            .map(|(i, _)| i)
                            .unwrap_or(input.len());
                        let next = input
                            .char_indices()
                            .nth(cursor + 1)
                            .map(|(i, _)| i)
                            .unwrap_or(input.len());
                        input.replace_range(idx..next, "");
                    }
                    app.overlay = Some(OverlayKind::DeepResearchKeyInput { input, cursor });
                    TuiAction::None
                }
                (KeyModifiers::NONE, KeyCode::Left) => {
                    cursor = cursor.saturating_sub(1);
                    app.overlay = Some(OverlayKind::DeepResearchKeyInput { input, cursor });
                    TuiAction::None
                }
                (KeyModifiers::NONE, KeyCode::Right) => {
                    if cursor < input.chars().count() {
                        cursor += 1;
                    }
                    app.overlay = Some(OverlayKind::DeepResearchKeyInput { input, cursor });
                    TuiAction::None
                }
                (mods, KeyCode::Char(c))
                    if !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT) =>
                {
                    let idx = input
                        .char_indices()
                        .nth(cursor)
                        .map(|(i, _)| i)
                        .unwrap_or(input.len());
                    input.insert(idx, c);
                    cursor += 1;
                    app.overlay = Some(OverlayKind::DeepResearchKeyInput { input, cursor });
                    TuiAction::None
                }
                _ => {
                    app.overlay = Some(OverlayKind::DeepResearchKeyInput { input, cursor });
                    TuiAction::None
                }
            }
        }

        None => TuiAction::None,
    }
}

fn auth_methods_visible(claude_code_detected: bool) -> Vec<(AuthMethodChoice, &'static str)> {
    let mut methods: Vec<(AuthMethodChoice, &'static str)> = vec![
        (AuthMethodChoice::ClaudeAiOAuth, "Claude.ai OAuth  (Pro/Max)"),
        (AuthMethodChoice::ConsoleOAuth,  "Console OAuth    (cria API key)"),
        (AuthMethodChoice::SsoOAuth,      "SSO OAuth        (claude.ai + SSO)"),
        (AuthMethodChoice::PasteApiKey,   "Colar API key    (sk-ant-...)"),
        (AuthMethodChoice::PasteAuthToken,"Colar Auth Token (Bearer)"),
        (AuthMethodChoice::PasteOpenAiKey,"Colar OpenAI key (sk-...)"),
        (AuthMethodChoice::UseBedrock,    "AWS Bedrock"),
        (AuthMethodChoice::UseVertex,     "Google Vertex AI"),
        (AuthMethodChoice::UseFoundry,    "Azure Foundry"),
        (AuthMethodChoice::LegacyElai,    "Elai OAuth legacy (elai.dev)"),
    ];
    if claude_code_detected {
        methods.insert(0, (AuthMethodChoice::ImportClaudeCode, "Importar Claude Code credentials  [detectado]"));
    }
    methods
}

fn handle_auth_picker_key(app: &mut UiApp, key: KeyEvent, step: AuthStep) -> TuiAction {
    match step {
        AuthStep::MethodList { selected, claude_code_detected } => {
            let methods = auth_methods_visible(claude_code_detected);
            let count = methods.len();
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Esc) => {
                    app.overlay = None;
                    TuiAction::None
                }
                (KeyModifiers::NONE, KeyCode::Up) => {
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::MethodList {
                            selected: selected.saturating_sub(1),
                            claude_code_detected,
                        },
                    });
                    TuiAction::None
                }
                (KeyModifiers::NONE, KeyCode::Down) => {
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::MethodList {
                            selected: (selected + 1).min(count.saturating_sub(1)),
                            claude_code_detected,
                        },
                    });
                    TuiAction::None
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    let Some((method, _)) = methods.get(selected).cloned() else {
                        app.overlay = None;
                        return TuiAction::None;
                    };
                    match method {
                        AuthMethodChoice::ClaudeAiOAuth
                        | AuthMethodChoice::ConsoleOAuth
                        | AuthMethodChoice::SsoOAuth => {
                            if method == AuthMethodChoice::SsoOAuth {
                                app.overlay = Some(OverlayKind::AuthPicker {
                                    step: AuthStep::EmailInput {
                                        method,
                                        input: String::new(),
                                        cursor: 0,
                                    },
                                });
                            } else {
                                let (url, port, rx, cancel_flag) = start_oauth_flow(method, None);
                                app.overlay = Some(OverlayKind::AuthPicker {
                                    step: AuthStep::BrowserFlow {
                                        method,
                                        url,
                                        port,
                                        started_at: Instant::now(),
                                        rx,
                                        cancel_flag,
                                    },
                                });
                            }
                        }
                        AuthMethodChoice::PasteApiKey
                        | AuthMethodChoice::PasteAuthToken
                        | AuthMethodChoice::PasteOpenAiKey => {
                            app.overlay = Some(OverlayKind::AuthPicker {
                                step: AuthStep::PasteSecret {
                                    method,
                                    input: String::new(),
                                    cursor: 0,
                                    masked: true,
                                },
                            });
                        }
                        AuthMethodChoice::UseBedrock => {
                            app.overlay = Some(OverlayKind::AuthPicker {
                                step: AuthStep::Confirm3p {
                                    method,
                                    env_var: "CLAUDE_CODE_USE_BEDROCK",
                                },
                            });
                        }
                        AuthMethodChoice::UseVertex => {
                            app.overlay = Some(OverlayKind::AuthPicker {
                                step: AuthStep::Confirm3p {
                                    method,
                                    env_var: "CLAUDE_CODE_USE_VERTEX",
                                },
                            });
                        }
                        AuthMethodChoice::UseFoundry => {
                            app.overlay = Some(OverlayKind::AuthPicker {
                                step: AuthStep::Confirm3p {
                                    method,
                                    env_var: "CLAUDE_CODE_USE_FOUNDRY",
                                },
                            });
                        }
                        AuthMethodChoice::ImportClaudeCode => {
                            match runtime::import_claude_code_credentials() {
                                Ok(Some(_)) => {
                                    app.overlay = Some(OverlayKind::AuthPicker {
                                        step: AuthStep::Done {
                                            label: "Imported Claude Code credentials".to_string(),
                                        },
                                    });
                                }
                                Ok(None) => {
                                    app.overlay = Some(OverlayKind::AuthPicker {
                                        step: AuthStep::Failed {
                                            error: "No Claude Code credentials found to import".to_string(),
                                        },
                                    });
                                }
                                Err(e) => {
                                    app.overlay = Some(OverlayKind::AuthPicker {
                                        step: AuthStep::Failed {
                                            error: format!("import error: {e}"),
                                        },
                                    });
                                }
                            }
                        }
                        AuthMethodChoice::LegacyElai => {
                            app.overlay = Some(OverlayKind::AuthPicker {
                                step: AuthStep::Done {
                                    label: "Use `elai login --legacy-elai` no terminal".to_string(),
                                },
                            });
                        }
                    }
                    TuiAction::None
                }
                _ => {
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::MethodList { selected, claude_code_detected },
                    });
                    TuiAction::None
                }
            }
        }

        AuthStep::EmailInput { method, mut input, mut cursor } => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Esc) => {
                    let detected = runtime::detect_claude_code_credentials().is_some();
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::MethodList { selected: 0, claude_code_detected: detected },
                    });
                }
                (KeyModifiers::NONE, KeyCode::Backspace)
                | (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
                    if cursor > 0 {
                        cursor -= 1;
                        let idx = input.char_indices().nth(cursor).map(|(i, _)| i).unwrap_or(input.len());
                        input.remove(idx);
                    }
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::EmailInput { method, input, cursor },
                    });
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    let email = if input.trim().is_empty() { None } else { Some(input.clone()) };
                    let (url, port, rx, cancel_flag) = start_oauth_flow(method, email);
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::BrowserFlow {
                            method,
                            url,
                            port,
                            started_at: Instant::now(),
                            rx,
                            cancel_flag,
                        },
                    });
                }
                (_, KeyCode::Char(c)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let idx = input.char_indices().nth(cursor).map(|(i, _)| i).unwrap_or(input.len());
                    input.insert(idx, c);
                    cursor += 1;
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::EmailInput { method, input, cursor },
                    });
                }
                _ => {
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::EmailInput { method, input, cursor },
                    });
                }
            }
            TuiAction::None
        }

        AuthStep::PasteSecret { method, mut input, mut cursor, masked } => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Esc) => {
                    let detected = runtime::detect_claude_code_credentials().is_some();
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::MethodList { selected: 0, claude_code_detected: detected },
                    });
                }
                (KeyModifiers::NONE, KeyCode::Backspace)
                | (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
                    if cursor > 0 {
                        cursor -= 1;
                        let idx = input.char_indices().nth(cursor).map(|(i, _)| i).unwrap_or(input.len());
                        input.remove(idx);
                    }
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::PasteSecret { method, input, cursor, masked },
                    });
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    let result = match method {
                        AuthMethodChoice::PasteApiKey => crate::auth::save_pasted_api_key(&input),
                        AuthMethodChoice::PasteAuthToken => crate::auth::save_pasted_auth_token(&input),
                        AuthMethodChoice::PasteOpenAiKey => crate::auth::save_pasted_openai_key(&input),
                        _ => Err(crate::auth::AuthError::InvalidInput("unexpected method".into())),
                    };
                    match result {
                        Ok(()) => {
                            let label = match method {
                                AuthMethodChoice::PasteApiKey => "API key salva".to_string(),
                                AuthMethodChoice::PasteOpenAiKey => "OpenAI key salva".to_string(),
                                _ => "Auth token salvo".to_string(),
                            };
                            app.overlay = Some(OverlayKind::AuthPicker {
                                step: AuthStep::Done { label },
                            });
                        }
                        Err(e) => {
                            app.overlay = Some(OverlayKind::AuthPicker {
                                step: AuthStep::Failed { error: e.to_string() },
                            });
                        }
                    }
                }
                (_, KeyCode::Char(c)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let idx = input.char_indices().nth(cursor).map(|(i, _)| i).unwrap_or(input.len());
                    input.insert(idx, c);
                    cursor += 1;
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::PasteSecret { method, input, cursor, masked },
                    });
                }
                _ => {
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::PasteSecret { method, input, cursor, masked },
                    });
                }
            }
            TuiAction::None
        }

        AuthStep::BrowserFlow { method, url, port, started_at, rx, cancel_flag } => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Esc) => {
                    cancel_flag.store(true, Ordering::Relaxed);
                    let detected = runtime::detect_claude_code_credentials().is_some();
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::MethodList { selected: 0, claude_code_detected: detected },
                    });
                }
                _ => {
                    // Drain events from channel while keeping step alive.
                    let mut next_step = AuthStep::BrowserFlow { method, url, port, started_at, rx, cancel_flag };
                    if let AuthStep::BrowserFlow { ref rx, .. } = next_step {
                        if let Ok(event) = rx.try_recv() {
                            next_step = match event {
                                AuthEvent::Success(label) => AuthStep::Done { label },
                                AuthEvent::Error(msg) => AuthStep::Failed { error: msg },
                                AuthEvent::Progress(_) => next_step,
                            };
                        }
                    }
                    // Reconstruct if still BrowserFlow (workaround for partial move).
                    app.overlay = Some(OverlayKind::AuthPicker { step: next_step });
                }
            }
            TuiAction::None
        }

        AuthStep::Confirm3p { method, env_var } => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Char('y'))
                | (KeyModifiers::NONE, KeyCode::Char('Y'))
                | (KeyModifiers::NONE, KeyCode::Enter) => {
                    match crate::auth::save_3p_named(env_var) {
                        Ok(()) => {
                            app.overlay = Some(OverlayKind::AuthPicker {
                                step: AuthStep::Done {
                                    label: format!("{method:?} salvo. Adicione `export {env_var}=1` ao seu shell RC."),
                                },
                            });
                        }
                        Err(e) => {
                            app.overlay = Some(OverlayKind::AuthPicker {
                                step: AuthStep::Failed { error: e.to_string() },
                            });
                        }
                    }
                }
                _ => {
                    let detected = runtime::detect_claude_code_credentials().is_some();
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::MethodList { selected: 0, claude_code_detected: detected },
                    });
                }
            }
            TuiAction::None
        }

        AuthStep::Done { label } => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Esc) | (KeyModifiers::NONE, KeyCode::Enter) => {
                    app.overlay = None;
                    return TuiAction::AuthComplete { label };
                }
                _ => {
                    app.overlay = Some(OverlayKind::AuthPicker { step: AuthStep::Done { label } });
                }
            }
            TuiAction::None
        }

        AuthStep::Failed { error } => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Esc) | (KeyModifiers::NONE, KeyCode::Enter) => {
                    let detected = runtime::detect_claude_code_credentials().is_some();
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::MethodList { selected: 0, claude_code_detected: detected },
                    });
                }
                _ => {
                    app.overlay = Some(OverlayKind::AuthPicker { step: AuthStep::Failed { error } });
                }
            }
            TuiAction::None
        }
    }
}

/// Drain AuthEvents from a BrowserFlow channel and advance the overlay step if needed.
/// Call this from the main tick loop so the UI updates without requiring a keypress.
pub fn drain_auth_events(app: &mut UiApp) {
    // We need to take the overlay, drain, and put it back to avoid borrow conflicts.
    let overlay = app.overlay.take();
    if let Some(OverlayKind::AuthPicker { step }) = overlay {
        let next_step = match step {
            AuthStep::BrowserFlow { method, url, port, started_at, rx, cancel_flag } => {
                match rx.try_recv() {
                    Ok(AuthEvent::Success(label)) => AuthStep::Done { label },
                    Ok(AuthEvent::Error(msg)) => AuthStep::Failed { error: msg },
                    Ok(AuthEvent::Progress(_)) | Err(_) => {
                        AuthStep::BrowserFlow { method, url, port, started_at, rx, cancel_flag }
                    }
                }
            }
            other => other,
        };
        app.overlay = Some(OverlayKind::AuthPicker { step: next_step });
    } else {
        app.overlay = overlay;
    }
}

const WIZARD_MODELS: &[&str] = &[
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-sonnet-4-6",
    "claude-haiku-4-5-20251001",
    "gpt-4o-mini",
];

const WIZARD_PERMS: &[&str] = &[
    "read-only",
    "workspace-write",
    "danger-full-access",
];

fn handle_first_run_wizard_key(
    app: &mut UiApp,
    key: KeyEvent,
    step: WizardStep,
    state: WizardState,
) -> TuiAction {
    match step {
        WizardStep::Welcome => match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Esc) => {
                app.overlay = None;
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Model { selected: 0 },
                    state,
                });
                TuiAction::None
            }
            _ => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Welcome,
                    state,
                });
                TuiAction::None
            }
        },

        WizardStep::Model { selected } => match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Esc) => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Welcome,
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Up) => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Model {
                        selected: selected.saturating_sub(1),
                    },
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                let next = (selected + 1).min(WIZARD_MODELS.len().saturating_sub(1));
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Model { selected: next },
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let model = WIZARD_MODELS
                    .get(selected)
                    .copied()
                    .unwrap_or("claude-opus-4-6")
                    .to_string();
                let new_state = WizardState { model, ..state };
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Permissions { selected: 0 },
                    state: new_state,
                });
                TuiAction::None
            }
            _ => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Model { selected },
                    state,
                });
                TuiAction::None
            }
        },

        WizardStep::Permissions { selected } => match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Esc) => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Model { selected: 0 },
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Up) => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Permissions {
                        selected: selected.saturating_sub(1),
                    },
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                let next = (selected + 1).min(WIZARD_PERMS.len().saturating_sub(1));
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Permissions { selected: next },
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let perm = WIZARD_PERMS
                    .get(selected)
                    .copied()
                    .unwrap_or("workspace-write")
                    .to_string();
                let new_state = WizardState {
                    permission_mode: perm,
                    ..state
                };
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Defaults { focused: 0 },
                    state: new_state,
                });
                TuiAction::None
            }
            _ => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Permissions { selected },
                    state,
                });
                TuiAction::None
            }
        },

        WizardStep::Defaults { focused } => match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Esc) => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Permissions { selected: 0 },
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Tab)
            | (KeyModifiers::NONE, KeyCode::Down) => {
                let next = (focused + 1) % 3;
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Defaults { focused: next },
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::SHIFT, KeyCode::BackTab)
            | (KeyModifiers::NONE, KeyCode::Up) => {
                let prev = if focused == 0 { 2 } else { focused - 1 };
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Defaults { focused: prev },
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Char(' ')) => {
                let new_state = match focused {
                    0 => WizardState {
                        features: runtime::FeatureFlags {
                            auto_update: !state.features.auto_update,
                            ..state.features
                        },
                        ..state
                    },
                    1 => WizardState {
                        features: runtime::FeatureFlags {
                            telemetry: !state.features.telemetry,
                            ..state.features
                        },
                        ..state
                    },
                    _ => WizardState {
                        features: runtime::FeatureFlags {
                            indexing: !state.features.indexing,
                            ..state.features
                        },
                        ..state
                    },
                };
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Defaults { focused },
                    state: new_state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Done,
                    state,
                });
                TuiAction::None
            }
            _ => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Defaults { focused },
                    state,
                });
                TuiAction::None
            }
        },

        WizardStep::Done => match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter) | (KeyModifiers::NONE, KeyCode::Esc) => {
                // Persist global config.
                let cfg = runtime::GlobalConfig {
                    setup_complete: true,
                    default_model: state.model.clone(),
                    default_permission_mode: state.permission_mode.clone(),
                    features: state.features.clone(),
                    ..runtime::GlobalConfig::default()
                };
                let _ = runtime::save_global_config(&cfg);
                // Apply to live app state.
                app.model = state.model.clone();
                app.permission_mode = state.permission_mode.clone();
                app.overlay = None;
                TuiAction::SetupComplete
            }
            _ => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Done,
                    state,
                });
                TuiAction::None
            }
        },
    }
}

fn start_oauth_flow(
    method: AuthMethodChoice,
    email: Option<String>,
) -> (
    String,
    u16,
    std::sync::mpsc::Receiver<AuthEvent>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let (tx, rx) = std::sync::mpsc::channel::<AuthEvent>();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = cancel.clone();
    let port = crate::auth::DEFAULT_OAUTH_CALLBACK_PORT;

    let endpoints = runtime::AnthropicOAuthEndpoints::production();
    let mode = match method {
        AuthMethodChoice::ConsoleOAuth => runtime::OAuthMode::Console,
        _ => runtime::OAuthMode::ClaudeAi,
    };
    let cfg = endpoints.to_oauth_config(mode);
    let pkce = match runtime::generate_pkce_pair() {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(AuthEvent::Error(format!("pkce: {e}")));
            return ("".to_string(), port, rx, cancel);
        }
    };
    let state = match runtime::generate_state() {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(AuthEvent::Error(format!("state: {e}")));
            return ("".to_string(), port, rx, cancel);
        }
    };
    let redirect = runtime::loopback_redirect_uri(port);
    let mut req = runtime::OAuthAuthorizationRequest::from_config(
        &cfg,
        redirect.clone(),
        state.clone(),
        &pkce,
    );
    if let Some(ref em) = email {
        req = req.with_extra_param("login_hint", em.as_str());
    }
    if matches!(method, AuthMethodChoice::SsoOAuth) {
        req = req.with_extra_param("login_method", "sso");
    }
    let url = req.build_url();
    let url_for_thread = url.clone();

    // JoinHandle descartado: thread daemon de abertura do browser para OAuth; encerra quando o canal fecha (TUI encerrando).
    let _oauth_browser_handle = std::thread::spawn(move || {
        let _ = tx.send(AuthEvent::Progress("Opening browser...".into()));
        let _ = crate::auth::open_browser(&url_for_thread);
        let _ = tx.send(AuthEvent::Progress(format!("Waiting for callback on port {port}...")));

        let listener = match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => l,
            Err(e) => {
                let _ = tx.send(AuthEvent::Error(format!("bind: {e}")));
                return;
            }
        };
        listener.set_nonblocking(true).ok();

        let stream = loop {
            if cancel_clone.load(Ordering::Relaxed) {
                let _ = tx.send(AuthEvent::Error("cancelled".into()));
                return;
            }
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => {
                    let _ = tx.send(AuthEvent::Error(format!("accept: {e}")));
                    return;
                }
            }
        };

        use std::io::{Read, Write};
        let mut s = stream;
        let mut buf = [0u8; 4096];
        let n = match s.read(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                let _ = tx.send(AuthEvent::Error(format!("read: {e}")));
                return;
            }
        };
        let req_text = String::from_utf8_lossy(&buf[..n]).to_string();
        let target = req_text
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("/");
        let cb = match runtime::parse_oauth_callback_request_target(target) {
            Ok(cb) => cb,
            Err(e) => {
                let _ = tx.send(AuthEvent::Error(format!("callback parse: {e}")));
                return;
            }
        };
        let body = if cb.error.is_some() {
            "Anthropic OAuth login failed. You can close this window."
        } else {
            "Anthropic OAuth login succeeded. You can close this window."
        };
        let _ = s.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .as_bytes(),
        );

        if let Some(err) = cb.error.as_ref() {
            let _ = tx.send(AuthEvent::Error(format!("OAuth error: {err}")));
            return;
        }
        let code = match cb.code {
            Some(c) => c,
            None => {
                let _ = tx.send(AuthEvent::Error("no auth code in callback".into()));
                return;
            }
        };
        let returned_state = cb.state.unwrap_or_default();
        if returned_state != state {
            let _ = tx.send(AuthEvent::Error("state mismatch (possible CSRF)".into()));
            return;
        }

        let rt = match tokio::runtime::Runtime::new() {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(AuthEvent::Error(format!("tokio runtime: {e}")));
                return;
            }
        };
        let client = api::ElaiApiClient::from_auth(api::AuthSource::None)
            .with_base_url(api::read_base_url());
        let exchange = runtime::OAuthTokenExchangeRequest::from_config(
            &cfg,
            code,
            state,
            pkce.verifier,
            redirect,
        );
        let beta = endpoints.beta_header.clone();
        let tokens = match rt.block_on(async {
            client
                .exchange_oauth_code_with_headers(&cfg, &exchange, &[("anthropic-beta", &beta)])
                .await
        }) {
            Ok(t) => t,
            Err(e) => {
                let _ = tx.send(AuthEvent::Error(format!("token exchange: {e}")));
                return;
            }
        };

        match method {
            AuthMethodChoice::ConsoleOAuth => {
                let api_key = match rt.block_on(async {
                    client
                        .create_console_api_key(&endpoints, &tokens.access_token)
                        .await
                }) {
                    Ok(k) => k,
                    Err(e) => {
                        let _ = tx.send(AuthEvent::Error(format!("create_api_key: {e}")));
                        return;
                    }
                };
                let auth_method = runtime::AuthMethod::ConsoleApiKey {
                    api_key,
                    origin: runtime::ApiKeyOrigin::ConsoleOAuth,
                };
                if let Err(e) = runtime::save_auth_method(&auth_method) {
                    let _ = tx.send(AuthEvent::Error(format!("save: {e}")));
                    return;
                }
                let _ = tx.send(AuthEvent::Success("Console OAuth concluido — API key salva".to_string()));
            }
            _ => {
                let auth_method = runtime::AuthMethod::ClaudeAiOAuth {
                    token_set: runtime::OAuthTokenSet {
                        access_token: tokens.access_token,
                        refresh_token: tokens.refresh_token,
                        expires_at: tokens.expires_at,
                        scopes: tokens.scopes,
                    },
                    subscription: None,
                };
                if let Err(e) = runtime::save_auth_method(&auth_method) {
                    let _ = tx.send(AuthEvent::Error(format!("save: {e}")));
                    return;
                }
                let label = if matches!(method, AuthMethodChoice::SsoOAuth) {
                    "SSO OAuth concluido".to_string()
                } else {
                    "Claude.ai OAuth concluido".to_string()
                };
                let _ = tx.send(AuthEvent::Success(label));
            }
        }
    });

    (url, port, rx, cancel)
}

/// Carrega lista de paths para o picker. Tenta:
/// 1. `.elai/index/metadata.json` se existe (rápido).
/// 2. Fallback: re-walk do projeto via `crate::verify::walk_project`.
/// Limita a 5000 paths.
fn load_indexed_paths(cwd: &std::path::Path) -> Vec<String> {
    const MAX_PATHS: usize = 5000;
    let metadata_path = cwd.join(".elai").join("index").join("metadata.json");
    if metadata_path.is_file() {
        if let Ok(s) = std::fs::read_to_string(&metadata_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                if let Some(arr) = v.get("indexed_paths").and_then(|x| x.as_array()) {
                    let paths: Vec<String> = arr
                        .iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .take(MAX_PATHS)
                        .collect();
                    if !paths.is_empty() {
                        return paths;
                    }
                }
            }
        }
    }
    // Fallback: walk via verify::walk_project (returns relative paths)
    crate::verify::walk_project(cwd)
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .take(MAX_PATHS)
        .collect()
}

/// Ranking: basename match > path match. Case-insensitive substring.
fn filter_mention_items<'a>(items: &'a [String], filter: &str) -> Vec<&'a String> {
    if filter.is_empty() {
        return items.iter().take(50).collect();
    }
    let needle = filter.to_lowercase();
    let mut basename_hits: Vec<&'a String> = Vec::new();
    let mut path_hits: Vec<&'a String> = Vec::new();
    for item in items {
        let lower = item.to_lowercase();
        let basename = std::path::Path::new(item)
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        if basename.contains(&needle) {
            basename_hits.push(item);
        } else if lower.contains(&needle) {
            path_hits.push(item);
        }
    }
    basename_hits.extend(path_hits);
    basename_hits.into_iter().take(50).collect()
}

fn filter_slash_items<'a>(
    items: &'a [(SlashCategory, String, String)],
    filter: &str,
) -> Vec<&'a (SlashCategory, String, String)> {
    let f = filter.trim_start_matches('/').to_lowercase();
    items
        .iter()
        .filter(|(_, cmd, desc)| {
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

        // Compute how many rows the input text needs so the box grows with content.
        // avail_w = terminal_width - 2 (block borders) - 2 ("> " prompt prefix), min 1
        let avail_w = (size.width.saturating_sub(4) as usize).max(1);
        let text_rows = count_input_rows(&app.input, avail_w);
        let visible_input_rows = text_rows.min(6_usize); // grow up to 6 rows, then scroll
        // area height = top_border(1) + input_rows + hint(1) + bottom_border(1) = rows + 3
        let input_area_h = (visible_input_rows + 3) as u16;

        // Outer vertical split: header, body, margin, status, input.
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(12),         // header
                Constraint::Min(3),             // chat body
                Constraint::Length(2),          // margin between chat and status (≈24px)
                Constraint::Length(1),          // status footer
                Constraint::Length(input_area_h), // input (grows with content)
            ])
            .split(size);

        draw_header(frame, outer[0], app);
        draw_chat(frame, outer[1], app);
        // outer[2] is the visual margin — nothing rendered
        draw_status(frame, outer[3], app);
        draw_input(frame, outer[4], app);

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
    // Quadro único arredondado com título compacto " Elai Code v0.7.1 "
    let title_style = Style::default()
        .fg(theme().easter_egg.warm)
        .add_modifier(Modifier::BOLD);
    let title = Span::styled(
        format!(" Elai Code v{} ", env!("CARGO_PKG_VERSION")),
        title_style,
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme().easter_egg.warm))
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split horizontal: mascote+ELAI | divisor | tips/recent.
    // `Min(52)` garante o ASCII (≈50 cols) na esquerda e cresce em telas largas;
    // o divisor acompanha a fronteira proporcional entre os dois `Min`.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(52),
            Constraint::Length(1),
            Constraint::Min(20),
        ])
        .split(inner);

    draw_elai_card(frame, cols[0], app);
    draw_header_divider(frame, cols[1]);
    draw_side_panel(frame, cols[2], app);
}

fn draw_header_divider(frame: &mut ratatui::Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme().easter_egg.warm));
    frame.render_widget(block, area);
}

// Mascote + "ELAI" (sem o ".CODE" gigante). Sem indent fixo: centralizado via Alignment::Center.
const ELAI_ASCII: &str = "\
██████████████████   ███████╗██╗      █████╗ ██╗\n\
████████▓▓▄▄▓▓▄▄▓▓   ██╔════╝██║     ██╔══██╗██║\n\
████████▓▓██▓▓██▓▓   █████╗  ██║     ███████║██║\n\
████████▓▓▀▀▓▓▀▀▓▓   ██╔══╝  ██║     ██╔══██║██║\n\
██████████████████   ███████╗███████╗██║  ██║██║\n\
";

// Largura do bloco mascote+ELAI (cada linha do ELAI_ASCII).
const ELAI_BLOCK_WIDTH: usize = 48;

/// Encurta o caminho atual:
/// 1. Substitui `$HOME` por `~`.
/// 2. Se ainda exceder `max_width`, elide segmentos do meio com `…`,
///    preservando a raiz e o(s) último(s) segmento(s) do caminho.
fn shorten_cwd(max_width: usize) -> String {
    let raw = env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "~".to_string());

    let home = env::var("HOME").unwrap_or_default();
    let path = if !home.is_empty() && raw.starts_with(&home) {
        format!("~{}", &raw[home.len()..])
    } else {
        raw
    };

    if path.chars().count() <= max_width || max_width == 0 {
        return path;
    }

    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return path;
    }

    let head = if path.starts_with('~') { "~" } else { "" };
    let mut tail_count = parts.len().min(2);

    while tail_count > 0 {
        let tail = parts[parts.len() - tail_count..].join("/");
        let candidate = if head.is_empty() {
            format!("/…/{tail}")
        } else {
            format!("{head}/…/{tail}")
        };
        if candidate.chars().count() <= max_width {
            return candidate;
        }
        tail_count -= 1;
    }

    // Último recurso: só o último segmento, possivelmente truncado.
    let last = parts.last().copied().unwrap_or("");
    let prefix = if head.is_empty() { "…/" } else { "~/…/" };
    let mut s = format!("{prefix}{last}");
    if s.chars().count() > max_width && max_width > 1 {
        let take = max_width.saturating_sub(1);
        s = format!("{}…", s.chars().take(take).collect::<String>());
    }
    s
}

fn draw_elai_card(frame: &mut ratatui::Frame, area: Rect, _app: &UiApp) {
    // corpo do mascote e texto ELAI.CODE: laranja claro
    let body_style = Style::default().fg(theme().easter_egg.body);
    // olhos (▄ ▀ e █ depois de ▓): laranja saturado
    let eye_style = Style::default().fg(theme().easter_egg.warm);
    // ▓ células: cavidade dos olhos — marrom escuro visível
    let dot_style = Style::default().fg(theme().easter_egg.dark);
    let dim = Style::default().fg(theme().text_secondary);

    let username = whoami_user();
    // Texto vai centralizado na coluna; reserva apenas 1 col de respiro de cada lado.
    let cwd_budget = (area.width as usize).saturating_sub(2);
    let cwd = shorten_cwd(cwd_budget);

    // Margem mínima de 1 linha acima do mascote.
    let mut lines: Vec<Line> = vec![Line::from(Span::raw(""))];
    lines.extend(ELAI_ASCII
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
        }));

    // Braços do mascote + linha de fechamento do "ELAI" (╚══════╝...).
    // Largura total = 7 + 3 + 2 + 3 + 4 + 27 = 46 chars; padding para 48 mantém
    // o alinhamento vertical com as linhas do mascote/ELAI sob `Alignment::Center`.
    lines.push(Line::from(vec![
        Span::raw("         "),
        Span::styled("███", body_style),
        Span::raw("   "),
        Span::styled("███", body_style),
        Span::raw("    "),
        Span::styled(
            "╚══════╝╚══════╝╚═╝  ╚═╝╚═╝",
            body_style,
        ),
        Span::raw(" ".repeat(ELAI_BLOCK_WIDTH.saturating_sub(46))),
    ]));

    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(vec![
        Span::styled(
            rust_i18n::t!("tui.header.welcome", username = username).to_string(),
            Style::default().fg(theme().easter_egg.warm).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),

    ]));
    lines.push(Line::from(Span::styled(cwd, dim)));

    let paragraph = Paragraph::new(lines)
        .block(Block::default())
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}

fn draw_side_panel(frame: &mut ratatui::Frame, area: Rect, app: &UiApp) {
    let muted = Style::default().fg(theme().text_secondary);
    let mut lines: Vec<Line> = vec![
        Line::from(Span::raw("")),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                rust_i18n::t!("tui.side_panel.tips_header").to_string(),
                Style::default()
                    .fg(theme().primary_accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            format!("  {}", rust_i18n::t!("tui.side_panel.run_init")),
            muted,
        )),
        Line::from(Span::styled(
            format!("  {}", rust_i18n::t!("tui.side_panel.shortcuts")),
            muted,
        )),
        Line::from(Span::styled(
            format!("  {}", rust_i18n::t!("tui.side_panel.slash_palette")),
            muted,
        )),
        Line::from(Span::raw("")),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                rust_i18n::t!("tui.side_panel.recent_activity_header").to_string(),
                Style::default()
                    .fg(theme().primary_accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    if app.recent_sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {}", rust_i18n::t!("tui.side_panel.no_recent")),
            muted,
        )));
    } else {
        for (session_id, msg_count) in app.recent_sessions.iter().take(3) {
            let short_id = session_id
                .strip_prefix("session-")
                .unwrap_or(session_id)
                .chars()
                .take(12)
                .collect::<String>();
            let msgs_label =
                rust_i18n::t!("tui.side_panel.session_msgs", count = msg_count.to_string());
            lines.push(Line::from(Span::styled(
                format!("  • {short_id} ({msgs_label})"),
                muted,
            )));
        }
    }

    let paragraph = Paragraph::new(lines).block(Block::default());
    frame.render_widget(paragraph, area);
}

// ── Chat panel ────────────────────────────────────────────────────────────────

fn draw_chat(frame: &mut ratatui::Frame, area: Rect, app: &mut UiApp) {
    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::TOP)
        .border_style(Style::default().fg(theme().border_inactive));

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

// ── Syntax highlighting ───────────────────────────────────────────────────────

static SYNTAX_RESOURCES: OnceLock<(SyntaxSet, SyntectTheme)> = OnceLock::new();

fn syntax_resources() -> &'static (SyntaxSet, SyntectTheme) {
    SYNTAX_RESOURCES.get_or_init(|| {
        let ss = SyntaxSet::load_defaults_newlines();
        let theme = ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .unwrap_or_default();
        (ss, theme)
    })
}

/// Tasks que renderizam como bloco multilinha (acumulam histórico de eventos).
/// Para outras tasks o comportamento é single-line (último evento substitui).
fn is_multiline_task(label: &str) -> bool {
    let l = label.to_ascii_lowercase();
    l.starts_with("deepresearch") || l.starts_with("deep research")
}

/// Quantas linhas visíveis o bloco multilinha mostra (janela rolante).
const DR_VISIBLE_LINES: usize = 5;

// ── Spinner sets variados (estilo rattles / cli-spinners) ────────────────
const SPINNER_DOTS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_DOTS3: &[&str] = &["⠋", "⠙", "⠚", "⠞", "⠖", "⠦", "⠴", "⠲", "⠳", "⠓"];
const SPINNER_ARROWS: &[&str] = &["←", "↖", "↑", "↗", "→", "↘", "↓", "↙"];
const SPINNER_EARTH: &[&str] = &["🌍", "🌎", "🌏"];
const SPINNER_SCAN: &[&str] = &["⣼", "⣹", "⢻", "⠿", "⡟", "⣏", "⣧", "⡖"];
const SPINNER_TRIANGLE: &[&str] = &["◢", "◣", "◤", "◥"];
const SPINNER_ARC: &[&str] = &["◜", "◠", "◝", "◞", "◡", "◟"];
const SPINNER_GROW: &[&str] = &["▁", "▃", "▄", "▅", "▆", "▇", "█", "▇", "▆", "▅", "▄", "▃"];

/// Escolhe o conjunto de frames do spinner conforme a "operação" atual,
/// deduzida pelo emoji/prefixo do último evento. Inspirado em cli-spinners.
fn spinner_for_msg(msg: &str) -> &'static [&'static str] {
    if msg.starts_with("🔎") || msg.contains("Query:") {
        SPINNER_ARROWS
    } else if msg.starts_with("🌐") {
        SPINNER_EARTH
    } else if msg.starts_with("💭") {
        SPINNER_DOTS3
    } else if msg.starts_with("🔍") {
        SPINNER_SCAN
    } else if msg.starts_with("⚡") || msg.contains("Batch") || msg.contains("paralelas") {
        SPINNER_TRIANGLE
    } else if msg.starts_with("✍️") || msg.contains("Compilando") {
        SPINNER_ARC
    } else if msg.starts_with("📡") {
        SPINNER_GROW
    } else {
        SPINNER_DOTS
    }
}

/// Quebra `text` em no máximo `max_lines` linhas de até `width` caracteres.
/// Quebra preferencialmente em espaços; se passar do limite, elide com "…".
fn wrap_event_lines(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(20);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let need = if current.is_empty() {
            word.chars().count()
        } else {
            current.chars().count() + 1 + word.chars().count()
        };
        if need <= width {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        } else {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                if lines.len() >= max_lines {
                    break;
                }
            }
            // Palavra maior que a largura: corta em chunks.
            let mut remaining: String = word.to_string();
            while remaining.chars().count() > width {
                let head: String = remaining.chars().take(width).collect();
                lines.push(head);
                remaining = remaining.chars().skip(width).collect();
                if lines.len() >= max_lines {
                    break;
                }
            }
            if lines.len() >= max_lines {
                break;
            }
            current = remaining;
        }
    }
    if !current.is_empty() && lines.len() < max_lines {
        lines.push(current);
    }

    // Se truncamos antes de exaurir o texto, marca a última com "…"
    if lines.len() == max_lines {
        let total_chars: usize = text.chars().count();
        let consumed: usize = lines.iter().map(|l| l.chars().count()).sum::<usize>() + lines.len();
        if consumed < total_chars {
            if let Some(last) = lines.last_mut() {
                if last.chars().count() >= width {
                    let trimmed: String = last.chars().take(width.saturating_sub(1)).collect();
                    *last = format!("{}…", trimmed);
                } else {
                    last.push('…');
                }
            }
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Brand color para o demarcador vertical de cada tipo de task.
/// Mesmo padrão de `lang_label_color` para blocos de código — cada task tem
/// sua cor pra ficar visualmente identificável (verde = bash, azul = deep, etc).
fn task_label_color(label: &str) -> ratatui::style::Color {
    let l = label.to_ascii_lowercase();
    if l.starts_with("deepresearch") || l.starts_with("deep research") {
        ratatui::style::Color::Rgb(0, 173, 216) // azul ciano
    } else if l.starts_with("verify") {
        ratatui::style::Color::Rgb(137, 224, 81) // verde (igual bash)
    } else if l.starts_with("agent") {
        ratatui::style::Color::Rgb(180, 130, 200) // roxo
    } else if l.starts_with("plugin") {
        ratatui::style::Color::Rgb(241, 224, 90) // amarelo
    } else {
        theme().info
    }
}

/// Language-specific label color — each language has its recognized brand color.
fn lang_label_color(lang: &str) -> ratatui::style::Color {
    match lang {
        "rust" | "rs" => ratatui::style::Color::Rgb(206, 100, 40),
        "python" | "py" => ratatui::style::Color::Rgb(53, 114, 165),
        "javascript" | "js" => ratatui::style::Color::Rgb(241, 224, 90),
        "typescript" | "ts" | "tsx" | "jsx" => ratatui::style::Color::Rgb(43, 116, 175),
        "go" => ratatui::style::Color::Rgb(0, 173, 216),
        "bash" | "sh" | "shell" | "zsh" | "fish" => ratatui::style::Color::Rgb(137, 224, 81),
        "json" => ratatui::style::Color::Rgb(200, 200, 200),
        "toml" | "yaml" | "yml" => ratatui::style::Color::Rgb(180, 130, 70),
        "html" => ratatui::style::Color::Rgb(227, 76, 38),
        "css" | "scss" | "sass" => ratatui::style::Color::Rgb(150, 90, 200),
        "c" => ratatui::style::Color::Rgb(85, 170, 200),
        "cpp" | "c++" | "cxx" => ratatui::style::Color::Rgb(243, 75, 125),
        "java" => ratatui::style::Color::Rgb(176, 114, 25),
        "ruby" | "rb" => ratatui::style::Color::Rgb(180, 40, 40),
        "sql" => ratatui::style::Color::Rgb(220, 80, 80),
        "diff" => ratatui::style::Color::Rgb(240, 200, 80),
        "markdown" | "md" => ratatui::style::Color::Rgb(100, 150, 200),
        _ => ratatui::style::Color::Rgb(160, 160, 160),
    }
}

/// Highlight a block of code using syntect and return ratatui Lines.
/// Each line is prefixed with the sidebar gutter `  │ `.
fn highlight_code_to_lines(code: &str, lang: &str, border_color: ratatui::style::Color) -> Vec<Line<'static>> {
    let (ss, syn_theme) = syntax_resources();
    let syntax = ss
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut h = HighlightLines::new(syntax, syn_theme);
    let mut result = Vec::new();

    for raw_line in LinesWithEndings::from(code) {
        let stripped = raw_line.trim_end_matches('\n').trim_end_matches('\r').to_string();
        let mut spans: Vec<Span<'static>> = vec![
            Span::styled("  │ ", Style::default().fg(border_color)),
        ];
        match h.highlight_line(raw_line, ss) {
            Ok(ranges) => {
                for (style, fragment) in ranges {
                    let text = fragment
                        .trim_end_matches('\n')
                        .trim_end_matches('\r')
                        .to_string();
                    if text.is_empty() {
                        continue;
                    }
                    let fg = ratatui::style::Color::Rgb(
                        style.foreground.r,
                        style.foreground.g,
                        style.foreground.b,
                    );
                    spans.push(Span::styled(text, Style::default().fg(fg)));
                }
            }
            Err(_) => {
                spans.push(Span::styled(stripped, Style::default().fg(theme().inline_code)));
            }
        }
        result.push(Line::from(spans));
    }
    result
}

// ── Markdown → ratatui Lines ──────────────────────────────────────────────────

fn markdown_to_tui_lines(text: &str, wrap_width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut bold = false;
    let mut italic = false;
    let mut heading: Option<u8> = None;
    let mut in_code = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();
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
            Some(1) => theme().info,
            Some(2) => theme().text_primary,
            Some(3) => theme().link,
            Some(_) => theme().text_secondary,
            None if bold => theme().warn,
            _ => theme().text_secondary,
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
                    Style::default().fg(theme().primary_accent),
                ));
            }
            MdEvent::End(TagEnd::Item) => flush(&mut lines, &mut spans),
            MdEvent::Start(Tag::CodeBlock(kind)) => {
                in_code = true;
                code_buffer.clear();
                flush(&mut lines, &mut spans);
                let (raw_lang, lang_display, original_label) = match kind {
                    CodeBlockKind::Fenced(l) if !l.is_empty() => {
                        let original = l.to_string();
                        (original.to_lowercase(), format!(" {original} "), original)
                    }
                    _ => (String::new(), String::new(), String::new()),
                };
                code_lang = raw_lang;
                let lc = lang_label_color(&code_lang);
                // Header: colored border + bold label + border continuation
                lines.push(Line::from(vec![
                    Span::styled("  ╭─", Style::default().fg(lc)),
                    Span::styled(lang_display, Style::default().fg(lc).add_modifier(Modifier::BOLD)),
                    Span::styled("─", Style::default().fg(lc)),
                ]));
                let _ = original_label;
            }
            MdEvent::End(TagEnd::CodeBlock) => {
                in_code = false;
                // Render accumulated code with syntect syntax highlighting.
                let lc = lang_label_color(&code_lang);
                let highlighted = highlight_code_to_lines(&code_buffer, &code_lang, lc);
                lines.extend(highlighted);
                // Footer border in lang color.
                lines.push(Line::from(Span::styled("  ╰──────", Style::default().fg(lc))));
                lines.push(Line::from(""));
                code_lang.clear();
                code_buffer.clear();
            }
            MdEvent::Text(t) => {
                let t = t.into_string();
                if in_code {
                    // Accumulate; rendering happens at End(CodeBlock) with syntect.
                    code_buffer.push_str(&t);
                } else {
                    let style = text_style(bold, italic, heading);
                    let mut first = true;
                    for raw_line in t.lines() {
                        if !first {
                            flush(&mut lines, &mut spans);
                        }
                        first = false;
                        // Preserva o texto inline exatamente como veio do parser,
                        // incluindo espaços nas bordas. `wrap_text` usa
                        // `split_whitespace` que descarta esses espaços e cola
                        // palavras de spans adjacentes (ex: bold/italic com texto).
                        if !raw_line.is_empty() {
                            spans.push(Span::styled(raw_line.to_string(), style));
                        }
                    }
                }
            }
            MdEvent::Code(t) => {
                spans.push(Span::styled(
                    t.into_string(),
                    Style::default().fg(theme().success),
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
                    Style::default().fg(theme().text_secondary),
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

/// Onboarding: renderiza a dica atual centralizada quando `app.chat` está vazio.
fn render_tips(app: &UiApp, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let Some((tip, idx, total)) = app.current_tip() else {
        return lines;
    };

    // Largura útil do conteúdo da dica (corpo wrapped). Limita a 80 cols mesmo
    // em telas largas para preservar legibilidade.
    let content_width = width.saturating_sub(8).min(80).max(20);
    // Padding lateral para centralizar o bloco na área disponível.
    let pad_left = width.saturating_sub(content_width) / 2;
    let pad = " ".repeat(pad_left);

    let title_style = Style::default()
        .fg(theme().primary_accent)
        .add_modifier(Modifier::BOLD);
    let body_style = Style::default().fg(theme().text_primary);
    let dim = Style::default().fg(theme().text_secondary);

    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(vec![
        Span::raw(pad.clone()),
        Span::styled(format!("✦ Dica {idx}/{total}"), dim),
    ]));
    lines.push(Line::from(vec![
        Span::raw(pad.clone()),
        Span::styled(tip.title.clone(), title_style),
    ]));
    lines.push(Line::from(Span::raw("")));

    for body_line in wrap_text(&tip.body, content_width) {
        if body_line.is_empty() {
            lines.push(Line::from(Span::raw("")));
        } else {
            lines.push(Line::from(vec![
                Span::raw(pad.clone()),
                Span::styled(body_line, body_style),
            ]));
        }
    }

    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(vec![
        Span::raw(pad.clone()),
        Span::styled(rust_i18n::t!("tui.nav.next_prev").to_string(), dim),
    ]));

    lines
}

fn chat_to_lines(app: &UiApp, width: usize) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    let wrap_width = width.saturating_sub(4).max(20);

    if app.show_tips && app.chat.is_empty() {
        return render_tips(app, width);
    }

    for entry in &app.chat {
        match entry {
            ChatEntry::UserMessage(msg) => {
                result.push(Line::from(Span::styled(
                    "> ".to_string(),
                    Style::default()
                        .fg(theme().primary_accent)
                        .add_modifier(Modifier::BOLD),
                )));
                for line in wrap_text(msg, wrap_width) {
                    result.push(Line::from(Span::styled(
                        format!("  {line}"),
                        Style::default().fg(theme().text_primary),
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
            ChatEntry::ToolBatchEntry { items, closed } => {
                // Header: `  ⚙ Tools (N)`
                result.push(Line::from(vec![
                    Span::styled(
                        "  \u{2699} ".to_string(),
                        Style::default().fg(theme().info),
                    ),
                    Span::styled(
                        format!("Tools ({})", items.len()),
                        Style::default()
                            .fg(theme().info)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                for item in items {
                    let (icon, color) = match item.status {
                        ToolItemStatus::Running => {
                            let frame = SPINNER[app.spinner_frame % SPINNER.len()];
                            (frame, theme().info)
                        }
                        ToolItemStatus::Ok => ("\u{2713}", theme().success), // ✓
                        ToolItemStatus::Err => ("\u{2717}", theme().error),  // ✗
                    };
                    result.push(Line::from(vec![
                        Span::styled(
                            format!("    {icon} "),
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            item.name.clone(),
                            Style::default()
                                .fg(theme().text_primary)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(" \u{00b7} "),
                        Span::styled(
                            item.input_summary.clone(),
                            Style::default().fg(theme().text_secondary),
                        ),
                    ]));
                }
                if *closed {
                    result.push(Line::from(""));
                }
            }
            ChatEntry::ThinkingBlock { text, finished } => {
                let (icon, label) = if *finished {
                    ("\u{1f4ad}", format!("Pensamento ({} chars)", text.len()))
                } else {
                    let frame = SPINNER[app.spinner_frame % SPINNER.len()];
                    (frame, "Pensando...".to_string())
                };
                result.push(Line::from(vec![
                    Span::styled(
                        format!("  {icon} "),
                        Style::default()
                            .fg(theme().thinking)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        label,
                        Style::default()
                            .fg(theme().thinking)
                            .add_modifier(if *finished { Modifier::DIM } else { Modifier::BOLD }),
                    ),
                ]));
                result.push(Line::from(""));
            }
            ChatEntry::SystemNote(note) => {
                for line in note.lines() {
                    result.push(Line::from(Span::styled(
                        format!("  {line}"),
                        Style::default().fg(theme().warn),
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
                        .fg(theme().primary_accent)
                        .add_modifier(Modifier::BOLD),
                )));
                for tx in transactions {
                    let (icon, color) = match &tx.outcome {
                        SwdOutcome::Verified => ("✓", theme().success),
                        SwdOutcome::Noop => ("·", theme().warn),
                        SwdOutcome::Drift { .. } => ("~", theme().warn),
                        SwdOutcome::Failed { .. } => ("✗", theme().error),
                        SwdOutcome::RolledBack => ("↩", theme().error),
                    };
                    let short_path: String = if tx.path.len() > 45 {
                        format!("…{}", &tx.path[tx.path.len() - 44..])
                    } else {
                        tx.path.clone()
                    };
                    result.push(Line::from(vec![
                        Span::styled(format!("    {icon} "), Style::default().fg(color)),
                        Span::styled(short_path, Style::default().fg(theme().text_primary)),
                        Span::styled(
                            format!("  [{}]", tx.tool_name),
                            Style::default().fg(theme().text_secondary),
                        ),
                    ]));
                }
                result.push(Line::from(""));
            }
            ChatEntry::CorrectionRetryEntry { attempt, max_attempts } => {
                result.push(Line::from(Span::styled(
                    format!("  \u{21a9} SWD retry {attempt}/{max_attempts}"),
                    Style::default()
                        .fg(theme().warn)
                        .add_modifier(Modifier::BOLD),
                )));
                result.push(Line::from(""));
            }
            ChatEntry::SwdDiffEntry { path, hunks } => {
                use crate::diff::DiffTag;
                result.push(Line::from(Span::styled(
                    format!("  --- {path}"),
                    Style::default().fg(theme().info).add_modifier(Modifier::BOLD),
                )));
                if hunks.is_empty() {
                    result.push(Line::from(Span::styled(
                        "  (Nenhuma alteração detectada)",
                        Style::default().fg(theme().text_secondary),
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
                            Style::default().fg(theme().diff_context),
                        )));
                        for line in &hunk.lines {
                            let (marker, style) = match line.tag {
                                DiffTag::Keep => (
                                    " ",
                                    Style::default().fg(theme().text_secondary),
                                ),
                                DiffTag::Remove => (
                                    "-",
                                    Style::default().fg(theme().error).add_modifier(Modifier::BOLD),
                                ),
                                DiffTag::Add => (
                                    "+",
                                    Style::default().fg(theme().success).add_modifier(Modifier::BOLD),
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
            ChatEntry::TaskProgress {
                label,
                msg,
                events,
                finished,
                status,
                ..
            } => {
                // Bloco visual igual aos code fences:
                //   ╭─ Label ─
                //   │ ⠋ msg…
                //   ╰──────
                let border_color = task_label_color(label);
                let (prefix, prefix_color) = if *finished {
                    match status {
                        Some(runtime::TaskStatus::Completed) => ("\u{2713}", theme().success), // ✓
                        Some(runtime::TaskStatus::Failed) => ("\u{2717}", theme().error),       // ✗
                        Some(runtime::TaskStatus::Killed) => ("\u{2298}", theme().warn),    // ⊘
                        _ => ("\u{2713}", theme().success),
                    }
                } else {
                    let frame = SPINNER[app.spinner_frame % SPINNER.len()];
                    (frame, border_color)
                };

                // Header: ╭─ {spinner} Label ─
                // Spinner muda conforme o tipo da última operação (URL, query,
                // thinking, etc), inspirado em cli-spinners/rattles.
                let header_spinner = if *finished {
                    prefix.to_string()
                } else {
                    let frames = spinner_for_msg(msg);
                    frames[app.spinner_frame % frames.len()].to_string()
                };
                result.push(Line::from(vec![
                    Span::styled("  ╭─ ", Style::default().fg(border_color)),
                    Span::styled(
                        format!("{header_spinner} "),
                        Style::default().fg(prefix_color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        label.clone(),
                        Style::default().fg(border_color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" ─", Style::default().fg(border_color)),
                ]));

                // Conteúdo: multilinha (events) com janela rolante das últimas
                // DR_VISIBLE_LINES linhas, ou linha única (msg).
                let inner_width = 88usize;
                if events.is_empty() {
                    result.push(Line::from(vec![
                        Span::styled("  │ ", Style::default().fg(border_color)),
                        Span::styled(msg.clone(), Style::default().fg(theme().text_secondary)),
                    ]));
                } else {
                    // Expande todos os eventos em linhas visuais, marcando qual é
                    // a primeira linha de cada evento (pra colocar o bullet "·").
                    let mut visual_lines: Vec<(bool, String)> = Vec::new();
                    for ev in events.iter() {
                        let chunks = wrap_event_lines(ev, inner_width, 4);
                        for (j, chunk) in chunks.into_iter().enumerate() {
                            visual_lines.push((j == 0, chunk));
                        }
                    }
                    // Janela rolante: pega últimas DR_VISIBLE_LINES.
                    let total = visual_lines.len();
                    let window: Vec<&(bool, String)> = if total > DR_VISIBLE_LINES {
                        visual_lines[total - DR_VISIBLE_LINES..].iter().collect()
                    } else {
                        visual_lines.iter().collect()
                    };
                    for (is_first, chunk) in window {
                        let lead = if *is_first { "·" } else { " " };
                        result.push(Line::from(vec![
                            Span::styled("  │ ", Style::default().fg(border_color)),
                            Span::styled(
                                format!("{lead} "),
                                Style::default().fg(theme().text_secondary),
                            ),
                            Span::styled(
                                chunk.clone(),
                                Style::default().fg(theme().text_secondary),
                            ),
                        ]));
                    }
                }

                // Footer: ╰──────
                result.push(Line::from(Span::styled(
                    "  ╰──────",
                    Style::default().fg(border_color),
                )));

                if *finished {
                    result.push(Line::from(""));
                }
            }
        }
    }

    if app.thinking {
        let frame = SPINNER[app.spinner_frame % SPINNER.len()];
        let (label, color) = if app.ultrathink_active {
            ("⚡ Ultrathink…", ratatui::style::Color::Rgb(255, 200, 50))
        } else {
            ("Thinking…", theme().thinking)
        };
        result.push(Line::from(Span::styled(
            format!("  {frame} {label}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
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
        theme().error
    } else if pct >= 80.0 {
        theme().warn
    } else {
        theme().success
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
        Style::default().fg(theme().warn)
    } else if app.ultrathink_active && app.thinking {
        Style::default().fg(ratatui::style::Color::Rgb(255, 200, 50))
    } else if app.thinking {
        Style::default().fg(theme().thinking)
    } else {
        Style::default().fg(theme().text_secondary)
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

// ── Input helpers ─────────────────────────────────────────────────────────────

/// Build visual rows for multi-line input. Each row is `(start, end)` into `chars`,
/// where `chars[start..end]` is the visible content (excludes any trailing `\n`).
/// Hard line breaks (`\n`) start a new row; soft wraps happen at `avail_w`.
fn build_input_rows(chars: &[char], avail_w: usize) -> Vec<(usize, usize)> {
    let avail_w = avail_w.max(1);
    if chars.is_empty() {
        return vec![(0, 0)];
    }
    let mut rows = Vec::new();
    let mut pos = 0;
    while pos <= chars.len() {
        // End of the current logical line (next \n or end of chars).
        let line_end = chars[pos..]
            .iter()
            .position(|&c| c == '\n')
            .map(|i| pos + i)
            .unwrap_or(chars.len());

        let line_len = line_end - pos;
        if line_len == 0 {
            // Empty logical line (blank line after \n or trailing \n).
            rows.push((pos, pos));
        } else {
            // Soft-wrap each segment that exceeds avail_w.
            let mut seg = pos;
            while seg < line_end {
                let end = (seg + avail_w).min(line_end);
                rows.push((seg, end));
                seg = end;
            }
        }

        if line_end >= chars.len() {
            break;
        }
        pos = line_end + 1; // skip past the \n
    }
    if rows.is_empty() {
        rows.push((0, 0));
    }
    rows
}

/// Count how many visual rows the input occupies (respects `\n` and `avail_w`).
fn count_input_rows(input: &str, avail_w: usize) -> usize {
    let chars: Vec<char> = input.chars().collect();
    build_input_rows(&chars, avail_w).len().max(1)
}

// ── Input box ─────────────────────────────────────────────────────────────────

fn draw_input(frame: &mut ratatui::Frame, area: Rect, app: &UiApp) {
    // In read mode show a distinct banner instead of the normal input box.
    if app.read_mode {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme().warn));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);
        frame.render_widget(
            Paragraph::new(Span::styled(
                "  MODO LEITURA — selecione e copie o texto livremente",
                Style::default().fg(theme().warn).add_modifier(Modifier::BOLD),
            )),
            layout[0],
        );
        frame.render_widget(
            Paragraph::new("  Pressione qualquer tecla para retomar o modo TUI")
                .style(Style::default().fg(theme().text_secondary)),
            layout[1],
        );
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().border_active));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let hint = " / comandos · ↑/↓ histórico · F2 modelo · F3 perm · F4 sessão · Ctrl+R leitura · Ctrl+C sair";

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let input_area = layout[0];

    // Hint line.
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(theme().text_secondary)),
        layout[1],
    );

    // Multi-line input: break text into rows respecting \n and avail_w wrap width.
    let avail_w = (input_area.width.saturating_sub(2) as usize).max(1); // minus "> " prefix
    let chars: Vec<char> = app.input.chars().collect();
    let rows = build_input_rows(&chars, avail_w);

    // Cursor row: last row whose start <= cursor_col.
    let cursor_row = rows
        .iter()
        .rposition(|(s, _)| *s <= app.cursor_col)
        .unwrap_or(0);

    // Scroll window: keep cursor visible (scroll from bottom).
    let max_visible = input_area.height as usize;
    let first_visible = if cursor_row + 1 > max_visible {
        cursor_row + 1 - max_visible
    } else {
        0
    };

    for (vis_i, row_i) in
        (first_visible..(first_visible + max_visible).min(rows.len())).enumerate()
    {
        let (row_start, row_end) = rows[row_i];
        let prefix = if row_i == 0 { "> " } else { "  " };
        let is_cursor_row = row_i == cursor_row;
        let row_area = Rect { y: input_area.y + vis_i as u16, height: 1, ..input_area };

        let mut spans = vec![Span::styled(prefix, Style::default().fg(theme().primary_accent))];

        if is_cursor_row {
            let local = app.cursor_col.saturating_sub(row_start);
            let row_chars = &chars[row_start..row_end];
            let before: String = row_chars[..local.min(row_chars.len())].iter().collect();
            let cursor_char = row_chars
                .get(local)
                .map(|c| c.to_string())
                .unwrap_or_else(|| " ".to_string());
            let after: String = row_chars
                .get(local + 1..)
                .unwrap_or(&[])
                .iter()
                .collect();
            if !before.is_empty() {
                spans.push(Span::styled(before, Style::default().fg(theme().text_primary)));
            }
            spans.push(Span::styled(
                cursor_char,
                Style::default()
                    .fg(theme().accent_on_primary_bg)
                    .bg(theme().primary_accent)
                    .add_modifier(Modifier::BOLD),
            ));
            if !after.is_empty() {
                spans.push(Span::styled(after, Style::default().fg(theme().text_primary)));
            }
        } else {
            let text: String = chars[row_start..row_end].iter().collect();
            if !text.is_empty() {
                spans.push(Span::styled(text, Style::default().fg(theme().text_primary)));
            }
        }

        frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
    }
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
        OverlayKind::LocalePicker { items, selected } => {
            let current = rust_i18n::locale().to_string();
            draw_picker(
                frame,
                area,
                "Idioma / Language",
                &items.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                *selected,
                None,
                &format!("atual: {current}"),
            );
        }
        OverlayKind::SlashPalette {
            items,
            filter,
            selected,
        } => {
            let rows = build_palette_rows(items, filter);
            draw_slash_palette_grouped(frame, area, &rows, *selected, filter);
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
        OverlayKind::AuthPicker { step } => {
            draw_auth_picker(frame, area, step);
        }
        OverlayKind::FirstRunWizard { step, state } => {
            draw_first_run_wizard(frame, area, step, state);
        }
        OverlayKind::DeepResearchKeyInput { input, cursor } => {
            draw_deepresearch_key_input(frame, area, input, *cursor);
        }
        OverlayKind::FileMentionPicker {
            items,
            filter,
            selected,
            ..
        } => {
            let filtered = filter_mention_items(items, filter);
            let title = if filter.is_empty() {
                format!(" Mention file ({} indexed) ", items.len())
            } else {
                format!(" Mention: {} ({} matches) ", filter, filtered.len())
            };
            let labels: Vec<String> = filtered
                .iter()
                .take(8)
                .map(|p| format!("  {p}"))
                .collect();
            draw_picker(
                frame,
                area,
                &title,
                &labels.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                (*selected).min(labels.len().saturating_sub(1)),
                Some(filter),
                "",
            );
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
        .border_style(Style::default().fg(theme().border_active));

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
                    .style(Style::default().fg(theme().accent_on_primary_bg).bg(theme().primary_accent))
            } else {
                ListItem::new(format!("  {item}"))
                    .style(Style::default().fg(theme().text_primary))
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
                .style(Style::default().fg(theme().text_secondary)),
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
        Paragraph::new(hint).style(Style::default().fg(theme().text_secondary)),
        hint_area,
    );
}

/// Render da paleta Ctrl+K com cabeçalhos de seção não-selecionáveis.
/// `selected` indexa diretamente `rows`; assume-se que aponta para um `Command`
/// (caller usa `first_selectable_row` / `next_selectable_row`).
fn draw_slash_palette_grouped(
    frame: &mut ratatui::Frame,
    area: Rect,
    rows: &[PaletteRow],
    selected: usize,
    filter: &str,
) {
    let width = (area.width / 2).max(50).min(area.width - 4);
    // +6 para borda + filtro + hint; usa mesmo cálculo do draw_picker.
    let height = (rows.len() as u16 + 6).min(area.height - 4);
    let x = (area.width.saturating_sub(width)) / 2 + area.x;
    let y = (area.height.saturating_sub(height)) / 2 + area.y;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Slash Commands (Ctrl+K) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().border_active));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let list_area = layout[0];
    let filter_area = layout[1];
    let hint_area = layout[2];

    let list_items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| match row {
            PaletteRow::Header(label) => ListItem::new(format!("  {label}")).style(
                Style::default()
                    .fg(theme().primary_accent)
                    .add_modifier(Modifier::BOLD),
            ),
            PaletteRow::Command { cmd, desc } => {
                let coming_soon = is_command_coming_soon(cmd);
                let suffix = if coming_soon { "  (em breve)" } else { "" };
                let body = format!("/{cmd:<12} {desc}{suffix}");
                if i == selected {
                    ListItem::new(format!("▶ {body}"))
                        .style(Style::default().fg(theme().accent_on_primary_bg).bg(theme().primary_accent))
                } else if coming_soon {
                    // Cinza apagado — ainda selecionável mas visualmente "off".
                    ListItem::new(format!("  {body}"))
                        .style(Style::default().fg(theme().text_secondary))
                } else {
                    ListItem::new(format!("  {body}")).style(Style::default().fg(theme().text_primary))
                }
            }
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(selected));
    frame.render_stateful_widget(List::new(list_items), list_area, &mut list_state);

    frame.render_widget(
        Paragraph::new(format!("  filtro: {filter}_"))
            .style(Style::default().fg(theme().text_secondary)),
        filter_area,
    );
    frame.render_widget(
        Paragraph::new("  ↑/↓ navegar · Enter aplicar · Esc cancelar")
            .style(Style::default().fg(theme().text_secondary)),
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
        .title(format!(" {} ", rust_i18n::t!("tui.tool_approval.title")))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().warn));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!("  {:<11} ", rust_i18n::t!("tui.tool_approval.tool_label")),
                Style::default().fg(theme().text_secondary),
            ),
            Span::styled(
                tool_name.to_string(),
                Style::default()
                    .fg(theme().info)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!("  {:<11} ", rust_i18n::t!("tui.tool_approval.required_label")),
                Style::default().fg(theme().text_secondary),
            ),
            Span::styled(required_mode.to_string(), Style::default().fg(theme().warn)),
        ]),
        Line::from(vec![
            Span::styled(
                format!("  {:<11} ", rust_i18n::t!("tui.tool_approval.input_label")),
                Style::default().fg(theme().text_secondary),
            ),
            Span::styled(
                input_preview.chars().take(60).collect::<String>(),
                Style::default().fg(theme().text_primary),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  [ Y ] ", Style::default().fg(theme().success).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{}   ", rust_i18n::t!("tui.tool_approval.yes_once")),
                Style::default().fg(theme().success),
            ),
            Span::styled("[ A ] ", Style::default().fg(theme().info).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{}   ", rust_i18n::t!("tui.tool_approval.always")),
                Style::default().fg(theme().info),
            ),
            Span::styled("[ N ] ", Style::default().fg(theme().error).add_modifier(Modifier::BOLD)),
            Span::styled(
                rust_i18n::t!("tui.tool_approval.no").to_string(),
                Style::default().fg(theme().error),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", rust_i18n::t!("tui.tool_approval.hint")),
            Style::default().fg(theme().text_secondary),
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
        .title(format!(
            " {} ",
            rust_i18n::t!("tui.swd_confirm.title", count = action_count.to_string())
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().border_active));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  [A] ",
                Style::default().fg(theme().success).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}    ", rust_i18n::t!("tui.swd_confirm.accept")),
                Style::default().fg(theme().text_primary),
            ),
            Span::styled(
                "[R] ",
                Style::default().fg(theme().error).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                rust_i18n::t!("tui.swd_confirm.reject").to_string(),
                Style::default().fg(theme().text_primary),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", rust_i18n::t!("tui.swd_confirm.hint")),
            Style::default().fg(theme().text_secondary),
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
        .title(format!(" {} ", rust_i18n::t!("tui.uninstall.title")))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().error));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let install_dir = std::env::var("ELAI_INSTALL_DIR").unwrap_or_else(|_| "/usr/local/bin".into());
    let home = std::env::var("HOME").unwrap_or_else(|_| "~".into());

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", rust_i18n::t!("tui.uninstall.will_remove")),
            Style::default().fg(theme().text_primary).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  • {install_dir}/elai"),
            Style::default().fg(theme().error),
        )),
        Line::from(Span::styled(
            format!("  • {home}/.elai/"),
            Style::default().fg(theme().error),
        )),
        Line::from(Span::styled(
            format!("  • {}", rust_i18n::t!("tui.uninstall.shell_rc_lines")),
            Style::default().fg(theme().error),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", rust_i18n::t!("tui.uninstall.irreversible")),
            Style::default().fg(theme().warn),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", rust_i18n::t!("tui.uninstall.confirm_hint")),
            Style::default().fg(theme().text_secondary),
        )),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_deepresearch_key_input(
    frame: &mut ratatui::Frame,
    area: Rect,
    input: &str,
    cursor: usize,
) {
    let width = 64u16.min(area.width.saturating_sub(4));
    let height = 11u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2 + area.x;
    let y = (area.height.saturating_sub(height)) / 2 + area.y;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Ativar DeepResearch ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().primary_accent));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let n_chars = input.chars().count();
    let masked: String = "\u{2022}".repeat(n_chars);

    // Reconstrói a linha do input com cursor visual em bloco invertido.
    let before: String = masked.chars().take(cursor).collect();
    let cur_char = if cursor < n_chars { "\u{2022}" } else { " " };
    let after: String = if cursor < n_chars {
        masked.chars().skip(cursor + 1).collect()
    } else {
        String::new()
    };

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Cole sua API key do serviço de deep research:",
            Style::default().fg(theme().text_primary).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  > ", Style::default().fg(theme().primary_accent)),
            Span::raw(before),
            Span::styled(
                cur_char.to_string(),
                Style::default()
                    .fg(theme().accent_on_primary_bg)
                    .bg(theme().primary_accent),
            ),
            Span::raw(after),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {} caracteres", n_chars),
            Style::default().fg(theme().text_secondary),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Enter: salvar e testar  ·  Esc: cancelar",
            Style::default().fg(theme().text_secondary),
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
        .title(format!(" {} ", rust_i18n::t!("tui.setup.title")))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().border_active));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let lines: Vec<Line> = if step == 0 {
        let anthropic_note = rust_i18n::t!("tui.setup.provider_anthropic_note").to_string();
        let openai_note = rust_i18n::t!("tui.setup.provider_openai_note").to_string();
        let both_label = rust_i18n::t!("tui.setup.provider_both").to_string();
        let providers: [(&str, &str); 3] = [
            ("Anthropic", anthropic_note.as_str()),
            ("OpenAI", openai_note.as_str()),
            (both_label.as_str(), ""),
        ];
        let mut v: Vec<Line> = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  {}", rust_i18n::t!("tui.setup.provider_question")),
                Style::default().fg(theme().text_primary),
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
                        .fg(theme().accent_on_primary_bg)
                        .bg(theme().primary_accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme().text_primary)
                },
            )));
        }
        v.push(Line::from(""));
        v.push(Line::from(Span::styled(
            format!("  {}", rust_i18n::t!("tui.setup.nav_select")),
            Style::default().fg(theme().text_secondary),
        )));
        v
    } else {
        let provider_name = match provider_sel {
            0 => "Anthropic",
            1 => "OpenAI",
            _ => if step == 1 { "Anthropic" } else { "OpenAI" },
        };
        let field_label = format!(
            "  {}",
            rust_i18n::t!("tui.setup.field_label", provider = provider_name.to_string())
        );
        let masked: String = "\u{2022}".repeat(input.chars().count());
        let display = format!("  > {masked}");
        vec![
            Line::from(""),
            Line::from(Span::styled(field_label, Style::default().fg(theme().text_primary))),
            Line::from(""),
            Line::from(Span::styled(
                display,
                Style::default().fg(theme().primary_accent),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("  {}", rust_i18n::t!("tui.setup.nav_confirm")),
                Style::default().fg(theme().text_secondary),
            )),
        ]
    };

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_first_run_wizard(
    frame: &mut ratatui::Frame,
    area: Rect,
    step: &WizardStep,
    state: &WizardState,
) {
    let width = (area.width * 2 / 3).max(60).min(area.width.saturating_sub(4));
    let height = 18u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2 + area.x;
    let y = (area.height.saturating_sub(height)) / 2 + area.y;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let (step_label, total_steps) = match step {
        WizardStep::Welcome => ("1", "5"),
        WizardStep::Model { .. } => ("2", "5"),
        WizardStep::Permissions { .. } => ("3", "5"),
        WizardStep::Defaults { .. } => ("4", "5"),
        WizardStep::Done => ("5", "5"),
    };

    let block = Block::default()
        .title(format!(
            " {} ",
            rust_i18n::t!("tui.wizard.title", step = step_label, total = total_steps)
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().border_active));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let lines: Vec<Line> = match step {
        WizardStep::Welcome => vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  {}", rust_i18n::t!("tui.wizard.welcome.title")),
                Style::default().fg(theme().text_primary).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("  {}", rust_i18n::t!("tui.wizard.welcome.intro")),
                Style::default().fg(theme().text_secondary),
            )),
            Line::from(Span::styled(
                format!("   • {}", rust_i18n::t!("tui.wizard.welcome.bullet_model")),
                Style::default().fg(theme().text_primary),
            )),
            Line::from(Span::styled(
                format!("   • {}", rust_i18n::t!("tui.wizard.welcome.bullet_permissions")),
                Style::default().fg(theme().text_primary),
            )),
            Line::from(Span::styled(
                format!("   • {}", rust_i18n::t!("tui.wizard.welcome.bullet_defaults")),
                Style::default().fg(theme().text_primary),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("  {}", rust_i18n::t!("tui.wizard.welcome.auth_hint")),
                Style::default().fg(theme().warn),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("  {}", rust_i18n::t!("tui.wizard.welcome.start_hint")),
                Style::default().fg(theme().text_secondary),
            )),
        ],

        WizardStep::Model { selected } => {
            let mut lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.wizard.model.title")),
                    Style::default().fg(theme().text_primary).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            let recommended = rust_i18n::t!("tui.wizard.model.recommended");
            let fallback = rust_i18n::t!("tui.wizard.model.fallback");
            let labels = [
                format!("claude-opus-4-7        {recommended}"),
                "claude-opus-4-6".to_string(),
                "claude-sonnet-4-6".to_string(),
                "claude-haiku-4-5-20251001".to_string(),
                format!("gpt-4o-mini            {fallback}"),
            ];
            for (i, label) in labels.iter().enumerate() {
                if i == *selected {
                    lines.push(Line::from(Span::styled(
                        format!("  ▶ {label}"),
                        Style::default()
                            .fg(theme().accent_on_primary_bg)
                            .bg(theme().primary_accent)
                            .add_modifier(Modifier::BOLD),
                    )));
                } else {
                    lines.push(Line::from(Span::styled(
                        format!("    {label}"),
                        Style::default().fg(theme().text_primary),
                    )));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("  {}", rust_i18n::t!("tui.wizard.nav.up_down_enter_esc")),
                Style::default().fg(theme().text_secondary),
            )));
            lines
        }

        WizardStep::Permissions { selected } => {
            let mut lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.wizard.permissions.title")),
                    Style::default().fg(theme().text_primary).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            let read_only = rust_i18n::t!("tui.wizard.permissions.read_only_desc").to_string();
            let workspace_write = rust_i18n::t!("tui.wizard.permissions.workspace_write_desc").to_string();
            let full_access = rust_i18n::t!("tui.wizard.permissions.full_access_desc").to_string();
            let labels: [(&str, &str); 3] = [
                ("read-only", read_only.as_str()),
                ("workspace-write", workspace_write.as_str()),
                ("danger-full-access", full_access.as_str()),
            ];
            for (i, (mode, desc)) in labels.iter().enumerate() {
                if i == *selected {
                    lines.push(Line::from(Span::styled(
                        format!("  ▶ {mode:<22} {desc}"),
                        Style::default()
                            .fg(theme().accent_on_primary_bg)
                            .bg(theme().primary_accent)
                            .add_modifier(Modifier::BOLD),
                    )));
                } else {
                    lines.push(Line::from(Span::styled(
                        format!("    {mode:<22} {desc}"),
                        Style::default().fg(theme().text_primary),
                    )));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("  {}", rust_i18n::t!("tui.wizard.nav.up_down_enter_esc")),
                Style::default().fg(theme().text_secondary),
            )));
            lines
        }

        WizardStep::Defaults { focused } => {
            let auto_update = rust_i18n::t!("tui.wizard.defaults.label_auto_update").to_string();
            let telemetry = rust_i18n::t!("tui.wizard.defaults.label_telemetry").to_string();
            let indexing = rust_i18n::t!("tui.wizard.defaults.label_indexing").to_string();
            let toggles: [(&str, bool); 3] = [
                (auto_update.as_str(), state.features.auto_update),
                (telemetry.as_str(), state.features.telemetry),
                (indexing.as_str(), state.features.indexing),
            ];
            let mut lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.wizard.defaults.title")),
                    Style::default().fg(theme().text_primary).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            for (i, (label, enabled)) in toggles.iter().enumerate() {
                let check = if *enabled { "[x]" } else { "[ ]" };
                let check_color = if *enabled { theme().success } else { theme().text_secondary };
                let is_focused = i == *focused;
                let prefix = if is_focused { "  ▶ " } else { "    " };
                if is_focused {
                    lines.push(Line::from(vec![
                        Span::styled(
                            prefix,
                            Style::default().fg(theme().primary_accent),
                        ),
                        Span::styled(
                            check,
                            Style::default().fg(check_color).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("  {label}"),
                            Style::default().fg(theme().text_primary).add_modifier(Modifier::BOLD),
                        ),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled(prefix, Style::default().fg(theme().text_secondary)),
                        Span::styled(check, Style::default().fg(check_color)),
                        Span::styled(
                            format!("  {label}"),
                            Style::default().fg(theme().text_secondary),
                        ),
                    ]));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("  {}", rust_i18n::t!("tui.wizard.defaults.hint")),
                Style::default().fg(theme().text_secondary),
            )));
            lines
        }

        WizardStep::Done => {
            let on = rust_i18n::t!("tui.wizard.state.on").to_string();
            let off = rust_i18n::t!("tui.wizard.state.off").to_string();
            let bool_str = |v: bool| -> String { if v { on.clone() } else { off.clone() } };
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.wizard.done.title")),
                    Style::default().fg(theme().success).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<13}", rust_i18n::t!("tui.wizard.done.label_model")),
                        Style::default().fg(theme().text_secondary),
                    ),
                    Span::styled(state.model.clone(), Style::default().fg(theme().info)),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<13}", rust_i18n::t!("tui.wizard.done.label_permissions")),
                        Style::default().fg(theme().text_secondary),
                    ),
                    Span::styled(
                        state.permission_mode.clone(),
                        Style::default().fg(theme().warn),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<13}", rust_i18n::t!("tui.wizard.done.label_auto_update")),
                        Style::default().fg(theme().text_secondary),
                    ),
                    Span::styled(
                        bool_str(state.features.auto_update),
                        Style::default().fg(theme().text_primary),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<13}", rust_i18n::t!("tui.wizard.done.label_telemetry")),
                        Style::default().fg(theme().text_secondary),
                    ),
                    Span::styled(
                        bool_str(state.features.telemetry),
                        Style::default().fg(theme().text_primary),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("  {:<13}", rust_i18n::t!("tui.wizard.done.label_indexing")),
                        Style::default().fg(theme().text_secondary),
                    ),
                    Span::styled(
                        bool_str(state.features.indexing),
                        Style::default().fg(theme().text_primary),
                    ),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.wizard.done.start_hint")),
                    Style::default().fg(theme().text_secondary),
                )),
            ]
        },
    };

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_auth_picker(frame: &mut ratatui::Frame, area: Rect, step: &AuthStep) {
    let width = (area.width * 2 / 3).max(60).min(area.width.saturating_sub(4));
    let height = 18u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2 + area.x;
    let y = (area.height.saturating_sub(height)) / 2 + area.y;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(format!(" {} ", rust_i18n::t!("tui.auth.title")))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().border_active));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    match step {
        AuthStep::MethodList { selected, claude_code_detected } => {
            let methods = auth_methods_visible(*claude_code_detected);
            let mut lines: Vec<Line> = Vec::new();

            if *claude_code_detected {
                lines.push(Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.auth.claude_code_detected")),
                    Style::default().fg(theme().success).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
            }

            for (i, (_method, label)) in methods.iter().enumerate() {
                let sel = i == *selected;
                if sel {
                    lines.push(Line::from(Span::styled(
                        format!("  {:>2}. {}", i + 1, label),
                        Style::default().fg(theme().accent_on_primary_bg).bg(theme().primary_accent).add_modifier(Modifier::BOLD),
                    )));
                } else {
                    lines.push(Line::from(Span::styled(
                        format!("     {}. {}", i + 1, label),
                        Style::default().fg(theme().text_primary),
                    )));
                }
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("  {}", rust_i18n::t!("tui.auth.nav.list")),
                Style::default().fg(theme().text_secondary),
            )));

            frame.render_widget(Paragraph::new(lines), inner);
        }

        AuthStep::EmailInput { input, cursor, .. } => {
            let before: String = input.chars().take(*cursor).collect();
            let cur: String = input.chars().nth(*cursor).map(|c| c.to_string()).unwrap_or_else(|| " ".to_string());
            let after: String = input.chars().skip(*cursor + 1).collect();
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.auth.email_input.label")),
                    Style::default().fg(theme().text_primary),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled("  > ", Style::default().fg(theme().primary_accent)),
                    Span::raw(before),
                    Span::styled(cur, Style::default().fg(theme().accent_on_primary_bg).bg(theme().primary_accent)),
                    Span::raw(after),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.auth.nav.enter_esc")),
                    Style::default().fg(theme().text_secondary),
                )),
            ];
            frame.render_widget(Paragraph::new(lines), inner);
        }

        AuthStep::PasteSecret { method, input, masked, .. } => {
            let display = if *masked {
                "\u{2022}".repeat(input.chars().count())
            } else {
                input.clone()
            };
            let label = match method {
                AuthMethodChoice::PasteApiKey => rust_i18n::t!("tui.auth.paste.api_key_label").to_string(),
                AuthMethodChoice::PasteOpenAiKey => rust_i18n::t!("tui.auth.paste.openai_key_label").to_string(),
                _ => rust_i18n::t!("tui.auth.paste.token_label").to_string(),
            };
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(format!("  {label}"), Style::default().fg(theme().text_primary))),
                Line::from(""),
                Line::from(Span::styled(
                    format!("  > {display}"),
                    Style::default().fg(theme().primary_accent),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.auth.nav.enter_esc")),
                    Style::default().fg(theme().text_secondary),
                )),
            ];
            frame.render_widget(Paragraph::new(lines), inner);
        }

        AuthStep::BrowserFlow { url, port, started_at, .. } => {
            let elapsed = started_at.elapsed().as_secs();
            let spinner_chars = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let spin = spinner_chars[(elapsed as usize) % spinner_chars.len()];
            let short_url: String = url.chars().take(70).collect();
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!(
                        "  {spin} {}",
                        rust_i18n::t!("tui.auth.browser.waiting", port = port.to_string())
                    ),
                    Style::default().fg(theme().thinking).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.auth.browser.url_label")),
                    Style::default().fg(theme().text_secondary),
                )),
                Line::from(Span::styled(short_url, Style::default().fg(theme().info))),
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.auth.nav.esc_cancel")),
                    Style::default().fg(theme().text_secondary),
                )),
            ];
            frame.render_widget(Paragraph::new(lines), inner);
        }

        AuthStep::Confirm3p { env_var, .. } => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!(
                        "  {}",
                        rust_i18n::t!("tui.auth.confirm3p.title", env_var = env_var.to_string())
                    ),
                    Style::default().fg(theme().text_primary).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.auth.confirm3p.shell_rc")),
                    Style::default().fg(theme().text_secondary),
                )),
                Line::from(Span::styled(
                    format!("    export {env_var}=1"),
                    Style::default().fg(theme().warn),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.auth.confirm3p.hint")),
                    Style::default().fg(theme().text_secondary),
                )),
            ];
            frame.render_widget(Paragraph::new(lines), inner);
        }

        AuthStep::Done { label } => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  \u{2713} {label}"),
                    Style::default().fg(theme().success).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.auth.done.close_hint")),
                    Style::default().fg(theme().text_secondary),
                )),
            ];
            frame.render_widget(Paragraph::new(lines), inner);
        }

        AuthStep::Failed { error } => {
            let short: String = error.chars().take(120).collect();
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  \u{2717} {}", rust_i18n::t!("tui.auth.failed.title")),
                    Style::default().fg(theme().error).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(short, Style::default().fg(theme().warn))),
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.auth.failed.back_hint")),
                    Style::default().fg(theme().text_secondary),
                )),
            ];
            frame.render_widget(Paragraph::new(lines), inner);
        }
    }
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
    fn text_chunks_append_to_existing_assistant_entry() {
        // Chunks fragmentados do mesmo parágrafo devem ser concatenados na
        // mesma `AssistantText` — preserva markdown / bullets / parágrafos
        // multi-linha exatamente como o agente emitiu.
        let mut app = make_app();
        app.apply_tui_msg(TuiMsg::TextChunk("Hello".to_string()));
        app.apply_tui_msg(TuiMsg::TextChunk(", World".to_string()));
        assert_eq!(app.chat.len(), 1);
        match &app.chat[0] {
            ChatEntry::AssistantText(text) => assert_eq!(text, "Hello, World"),
            other => panic!("expected AssistantText, got {other:?}"),
        }
    }

    #[test]
    fn multiline_text_only_turn_stays_as_single_assistant_text() {
        // Resposta com bullets / parágrafos sem tool call vira UMA entry só
        // — sem fragmentação, sem batch.
        let mut app = make_app();
        app.apply_tui_msg(TuiMsg::TextChunk(
            "Vou analisar:\n- locales/ — i18n\n- crates/api/ — providers\n\nO que prefere?".into(),
        ));
        app.apply_tui_msg(TuiMsg::Done);
        assert_eq!(app.chat.len(), 1);
        match &app.chat[0] {
            ChatEntry::AssistantText(text) => {
                assert!(text.contains("- locales/"));
                assert!(text.contains("O que prefere?"));
            }
            other => panic!("expected AssistantText, got {other:?}"),
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
    fn tool_calls_in_sequence_form_single_batch() {
        let mut app = make_app();
        for cmd in ["ls", "pwd", "whoami"] {
            app.apply_tui_msg(TuiMsg::ToolCall {
                name: "bash".to_string(),
                input: format!(r#"{{"command":"{cmd}"}}"#),
            });
            app.apply_tui_msg(TuiMsg::ToolResult { ok: true });
        }
        assert_eq!(app.chat.len(), 1);
        match &app.chat[0] {
            ChatEntry::ToolBatchEntry { items, closed } => {
                assert_eq!(items.len(), 3);
                assert!(items.iter().all(|it| it.status == ToolItemStatus::Ok));
                assert!(!closed, "batch stays open until something else interrupts");
            }
            other => panic!("expected ToolBatchEntry, got {other:?}"),
        }
    }

    #[test]
    fn tool_result_marks_last_running_item() {
        let mut app = make_app();
        app.apply_tui_msg(TuiMsg::ToolCall {
            name: "bash".into(),
            input: r#"{"command":"ls"}"#.into(),
        });
        app.apply_tui_msg(TuiMsg::ToolCall {
            name: "read_file".into(),
            input: r#"{"path":"x"}"#.into(),
        });
        // Apenas um result chega — deve marcar o ÚLTIMO item Running (read_file).
        app.apply_tui_msg(TuiMsg::ToolResult { ok: false });
        match &app.chat[0] {
            ChatEntry::ToolBatchEntry { items, .. } => {
                assert_eq!(items[0].status, ToolItemStatus::Running);
                assert_eq!(items[1].status, ToolItemStatus::Err);
            }
            _ => panic!("expected ToolBatchEntry"),
        }
    }

    #[test]
    fn text_chunk_between_tools_closes_batch_and_creates_assistant_text() {
        // Texto entre tools encerra o batch atual e abre uma `AssistantText`
        // separada — o próximo tool call iniciará um NOVO batch.
        let mut app = make_app();
        app.apply_tui_msg(TuiMsg::ToolCall {
            name: "bash".into(),
            input: r#"{"command":"ls"}"#.into(),
        });
        app.apply_tui_msg(TuiMsg::ToolResult { ok: true });
        app.apply_tui_msg(TuiMsg::TextChunk("Encontrei tudo.".into()));
        app.apply_tui_msg(TuiMsg::ToolCall {
            name: "bash".into(),
            input: r#"{"command":"pwd"}"#.into(),
        });
        // Esperado: [Tools (fechado)] [AssistantText] [Tools (aberto)]
        assert_eq!(app.chat.len(), 3);
        match &app.chat[0] {
            ChatEntry::ToolBatchEntry { items, closed } => {
                assert_eq!(items.len(), 1);
                assert!(*closed, "first batch closed by the assistant text that follows");
            }
            _ => panic!("expected first Tools batch"),
        }
        match &app.chat[1] {
            ChatEntry::AssistantText(text) => assert_eq!(text, "Encontrei tudo."),
            _ => panic!("expected AssistantText"),
        }
        match &app.chat[2] {
            ChatEntry::ToolBatchEntry { items, closed } => {
                assert_eq!(items.len(), 1);
                assert!(!closed);
            }
            _ => panic!("expected second Tools batch"),
        }
    }

    #[test]
    fn tool_input_summary_uses_human_readable_form() {
        // O `input_summary` deve ser o comando legível (extraído via
        // `tool_input_one_line`), NÃO o JSON literal completo. Garante que
        // turns longos não fiquem mostrando `{"command":"cd /Users/..."` cru.
        let mut app = make_app();
        app.apply_tui_msg(TuiMsg::ToolCall {
            name: "bash".into(),
            input: r#"{"command":"git status --short"}"#.into(),
        });
        match &app.chat[0] {
            ChatEntry::ToolBatchEntry { items, .. } => {
                assert_eq!(items[0].input_summary, "git status --short");
                assert!(
                    !items[0].input_summary.contains("{\"command\""),
                    "summary não pode incluir o JSON cru"
                );
            }
            _ => panic!("expected ToolBatchEntry"),
        }
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

    // ─── AuthPicker tests ─────────────────────────────────────────────────────

    #[test]
    fn open_auth_picker_seeds_method_list() {
        let mut app = make_app();
        // Simulating open_auth_picker without calling runtime (no credentials expected here).
        app.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::MethodList {
                selected: 0,
                claude_code_detected: false,
            },
        });
        assert!(matches!(
            app.overlay,
            Some(OverlayKind::AuthPicker {
                step: AuthStep::MethodList { selected: 0, claude_code_detected: false }
            })
        ));
    }

    #[test]
    fn auth_picker_method_list_navigation_clamps() {
        let mut app = make_app();
        let methods = auth_methods_visible(false);
        let count = methods.len();

        app.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::MethodList { selected: 0, claude_code_detected: false },
        });

        // Navigate up at 0 — should stay at 0.
        let key_up = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        );
        handle_overlay_key(&mut app, key_up);
        if let Some(OverlayKind::AuthPicker { step: AuthStep::MethodList { selected, .. } }) = &app.overlay {
            assert_eq!(*selected, 0, "should not go below 0");
        } else {
            panic!("overlay should still be MethodList");
        }

        // Navigate down past the end — should clamp at count-1.
        for _ in 0..count + 5 {
            let key_down = crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Down,
                crossterm::event::KeyModifiers::NONE,
            );
            handle_overlay_key(&mut app, key_down);
        }
        if let Some(OverlayKind::AuthPicker { step: AuthStep::MethodList { selected, .. } }) = &app.overlay {
            assert_eq!(*selected, count - 1, "should clamp at last item");
        } else {
            panic!("overlay should still be MethodList");
        }
    }

    #[test]
    fn auth_picker_esc_closes_overlay() {
        let mut app = make_app();
        app.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::MethodList { selected: 0, claude_code_detected: false },
        });
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        );
        handle_overlay_key(&mut app, key);
        assert!(app.overlay.is_none(), "Esc should close the overlay");
    }

    #[test]
    fn auth_picker_paste_secret_masks_input() {
        let mut app = make_app();
        app.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::PasteSecret {
                method: AuthMethodChoice::PasteApiKey,
                input: String::new(),
                cursor: 0,
                masked: true,
            },
        });

        // Type "sk-ant-abc".
        for c in "sk-ant-abc".chars() {
            let key = crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            );
            handle_overlay_key(&mut app, key);
        }

        if let Some(OverlayKind::AuthPicker {
            step: AuthStep::PasteSecret { input, masked, .. },
        }) = &app.overlay
        {
            assert_eq!(input, "sk-ant-abc", "input should store real value");
            assert!(*masked, "masked flag should remain true");
        } else {
            panic!("overlay should still be PasteSecret");
        }
    }

    #[test]
    fn auth_picker_done_step_returns_auth_complete_on_enter() {
        let mut app = make_app();
        app.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::Done { label: "test-label".to_string() },
        });
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        let action = handle_overlay_key(&mut app, key);
        assert!(
            matches!(action, TuiAction::AuthComplete { label } if label == "test-label"),
            "Done+Enter should return AuthComplete"
        );
        assert!(app.overlay.is_none(), "overlay should be closed after Done");
    }

    #[test]
    fn auth_picker_failed_step_goes_back_to_method_list_on_enter() {
        let mut app = make_app();
        app.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::Failed { error: "some error".to_string() },
        });
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        handle_overlay_key(&mut app, key);
        assert!(
            matches!(
                app.overlay,
                Some(OverlayKind::AuthPicker { step: AuthStep::MethodList { .. } })
            ),
            "Failed+Enter should go back to MethodList"
        );
    }

    #[test]
    fn auth_methods_visible_includes_import_when_detected() {
        let without = auth_methods_visible(false);
        let with_cc = auth_methods_visible(true);
        assert_eq!(with_cc.len(), without.len() + 1);
        assert_eq!(with_cc[0].0, AuthMethodChoice::ImportClaudeCode);
    }

    #[test]
    fn auth_picker_email_input_edits_correctly() {
        let mut app = make_app();
        app.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::EmailInput {
                method: AuthMethodChoice::SsoOAuth,
                input: String::new(),
                cursor: 0,
            },
        });

        for c in "test@example.com".chars() {
            let key = crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            );
            handle_overlay_key(&mut app, key);
        }

        if let Some(OverlayKind::AuthPicker { step: AuthStep::EmailInput { input, cursor, .. } }) = &app.overlay {
            assert_eq!(input, "test@example.com");
            assert_eq!(*cursor, "test@example.com".len());
        } else {
            panic!("expected EmailInput step");
        }
    }

    // ─── FileMentionPicker tests ──────────────────────────────────────────────

    fn make_key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn file_mention_picker_opens_with_at_char() {
        let mut app = make_app();
        app.input = String::new();
        app.cursor_col = 0;
        let anchor_pos = app.cursor_col;
        app.input_char('@');
        app.open_file_mention_picker(std::path::Path::new("/tmp"), anchor_pos);
        assert_eq!(app.input, "@");
        assert_eq!(app.cursor_col, 1);
        assert!(matches!(
            app.overlay,
            Some(OverlayKind::FileMentionPicker { anchor_pos: 0, .. })
        ));
    }

    #[test]
    fn file_mention_picker_filters_by_substring() {
        let items = vec![
            "src/foo.rs".to_string(),
            "src/bar.rs".to_string(),
            "tests/baz.rs".to_string(),
        ];
        let filtered = filter_mention_items(&items, "foo");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0], "src/foo.rs");
    }

    #[test]
    fn file_mention_picker_ranks_basename_first() {
        // Items where only one has basename match (the other only path-component match)
        let items = vec![
            "src/foo_match/bar.rs".to_string(), // path match, not basename
            "match_b.rs".to_string(),            // basename match
        ];
        let filtered = filter_mention_items(&items, "match");
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0], "match_b.rs", "basename match should come first");
        assert_eq!(filtered[1], "src/foo_match/bar.rs");
    }

    #[test]
    fn file_mention_picker_enter_inserts_path_in_input() {
        let mut app = make_app();
        app.input = "@".to_string();
        app.cursor_col = 1;
        app.overlay = Some(OverlayKind::FileMentionPicker {
            items: vec!["src/foo.rs".to_string()],
            filter: String::new(),
            selected: 0,
            anchor_pos: 0,
        });
        handle_overlay_key(&mut app, make_key(crossterm::event::KeyCode::Enter));
        assert_eq!(app.input, "@src/foo.rs ");
        assert!(app.overlay.is_none());
    }

    #[test]
    fn file_mention_picker_esc_closes_without_modifying_input() {
        let mut app = make_app();
        app.input = "hello @".to_string();
        app.cursor_col = 7;
        app.overlay = Some(OverlayKind::FileMentionPicker {
            items: vec!["src/foo.rs".to_string()],
            filter: String::new(),
            selected: 0,
            anchor_pos: 6,
        });
        handle_overlay_key(&mut app, make_key(crossterm::event::KeyCode::Esc));
        assert!(app.overlay.is_none());
        assert_eq!(app.input, "hello @");
    }

    #[test]
    fn file_mention_picker_backspace_at_empty_filter_removes_at_char() {
        let mut app = make_app();
        app.input = "@".to_string();
        app.cursor_col = 1;
        app.overlay = Some(OverlayKind::FileMentionPicker {
            items: vec![],
            filter: String::new(),
            selected: 0,
            anchor_pos: 0,
        });
        handle_overlay_key(&mut app, make_key(crossterm::event::KeyCode::Backspace));
        assert_eq!(app.input, "");
        assert!(app.overlay.is_none());
    }

    #[test]
    fn load_indexed_paths_reads_metadata_json_when_present() {
        let dir = std::env::temp_dir().join(format!(
            "elai_test_meta_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        let index_dir = dir.join(".elai").join("index");
        std::fs::create_dir_all(&index_dir).unwrap();
        let metadata = r#"{"indexed_paths": ["a.rs", "b.rs"]}"#;
        std::fs::write(index_dir.join("metadata.json"), metadata).unwrap();
        let paths = load_indexed_paths(&dir);
        assert_eq!(paths, vec!["a.rs".to_string(), "b.rs".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_indexed_paths_falls_back_to_walk_when_no_metadata() {
        let dir = std::env::temp_dir().join(format!(
            "elai_test_walk_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("foo.rs"), "fn main() {}").unwrap();
        let paths = load_indexed_paths(&dir);
        assert!(
            paths.contains(&"foo.rs".to_string()),
            "should find foo.rs via walk; got: {:?}",
            paths
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn filter_mention_items_returns_top_50_when_filter_empty() {
        let items: Vec<String> = (0..100).map(|i| format!("file_{i}.rs")).collect();
        let result = filter_mention_items(&items, "");
        assert_eq!(result.len(), 50);
    }

    // ─── First-run wizard tests ───────────────────────────────────────────────

    #[test]
    fn setup_wizard_starts_at_welcome() {
        let mut app = make_app();
        app.open_first_run_wizard();
        match &app.overlay {
            Some(OverlayKind::FirstRunWizard { step: WizardStep::Welcome, .. }) => {}
            other => panic!("expected Welcome step, got: {other:?}"),
        }
    }

    fn wizard_enter(app: &mut UiApp) {
        handle_overlay_key(app, make_key(KeyCode::Enter));
    }

    #[test]
    fn setup_wizard_enter_advances_through_steps() {
        let mut app = make_app();
        app.open_first_run_wizard();

        // Welcome -> Model
        wizard_enter(&mut app);
        assert!(
            matches!(
                app.overlay,
                Some(OverlayKind::FirstRunWizard { step: WizardStep::Model { .. }, .. })
            ),
            "expected Model step"
        );

        // Model -> Permissions
        wizard_enter(&mut app);
        assert!(
            matches!(
                app.overlay,
                Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Permissions { .. },
                    ..
                })
            ),
            "expected Permissions step"
        );

        // Permissions -> Defaults
        wizard_enter(&mut app);
        assert!(
            matches!(
                app.overlay,
                Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Defaults { .. },
                    ..
                })
            ),
            "expected Defaults step"
        );

        // Defaults -> Done
        wizard_enter(&mut app);
        assert!(
            matches!(
                app.overlay,
                Some(OverlayKind::FirstRunWizard { step: WizardStep::Done, .. })
            ),
            "expected Done step"
        );
    }

    #[test]
    fn setup_wizard_model_selection_is_captured() {
        let mut app = make_app();
        app.open_first_run_wizard();
        wizard_enter(&mut app); // -> Model

        // Navigate down twice to select index 2 (claude-sonnet-4-6)
        handle_overlay_key(&mut app, make_key(KeyCode::Down));
        handle_overlay_key(&mut app, make_key(KeyCode::Down));
        wizard_enter(&mut app); // -> Permissions

        // Skip to Done
        wizard_enter(&mut app); // -> Defaults
        wizard_enter(&mut app); // -> Done

        match &app.overlay {
            Some(OverlayKind::FirstRunWizard {
                step: WizardStep::Done,
                state,
            }) => {
                assert_eq!(state.model, "claude-sonnet-4-6");
            }
            other => panic!("expected Done, got: {other:?}"),
        }
    }

    #[test]
    fn setup_wizard_defaults_toggle_works() {
        let mut app = make_app();
        app.open_first_run_wizard();
        wizard_enter(&mut app); // Welcome -> Model
        wizard_enter(&mut app); // Model -> Permissions
        wizard_enter(&mut app); // Permissions -> Defaults

        // Initially features.auto_update = true; Space should toggle it off
        handle_overlay_key(&mut app, make_key(KeyCode::Char(' ')));
        match &app.overlay {
            Some(OverlayKind::FirstRunWizard {
                step: WizardStep::Defaults { .. },
                state,
            }) => {
                assert!(!state.features.auto_update, "auto_update should be toggled off");
            }
            other => panic!("expected Defaults, got: {other:?}"),
        }
    }

    #[test]
    fn setup_wizard_esc_from_welcome_closes_overlay() {
        let mut app = make_app();
        app.open_first_run_wizard();
        handle_overlay_key(&mut app, make_key(KeyCode::Esc));
        assert!(app.overlay.is_none(), "overlay should be closed after Esc on Welcome");
    }

    #[test]
    fn setup_wizard_done_persists_global_config() {
        use std::sync::Mutex;
        static HOME_LOCK: Mutex<()> = Mutex::new(());

        let td = tempfile::TempDir::new().unwrap();
        let _lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        struct HomeRestore(Option<std::ffi::OsString>);
        impl Drop for HomeRestore {
            fn drop(&mut self) {
                match &self.0 {
                    Some(p) => std::env::set_var("HOME", p),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
        let _restore = HomeRestore(std::env::var_os("HOME"));
        std::env::set_var("HOME", td.path());

        let mut app = make_app();
        app.open_first_run_wizard();
        wizard_enter(&mut app); // Welcome -> Model
        wizard_enter(&mut app); // Model -> Permissions
        wizard_enter(&mut app); // Permissions -> Defaults
        wizard_enter(&mut app); // Defaults -> Done
        wizard_enter(&mut app); // Done -> close + persist

        assert!(app.overlay.is_none(), "overlay should be closed after Done+Enter");
        let cfg = runtime::load_global_config().expect("config should be loadable");
        assert!(cfg.setup_complete, "setup_complete should be true after wizard");
    }

    // ── Slash palette: categorização Ctrl+K ──────────────────────────────────

    #[test]
    fn palette_items_have_valid_categories() {
        let items = slash_palette_items();
        let specs = commands::slash_command_specs();
        let visible_spec_count = specs
            .iter()
            .filter(|s| !s.hidden && (s.is_enabled)())
            .count();
        // 5 comandos REPL-local: swd, keys, theme, uninstall, exit.
        assert_eq!(items.len(), visible_spec_count + 5);

        for (cat, name, _desc) in &items {
            if let Some(spec) = specs.iter().find(|s| s.name == name.as_str()) {
                assert_eq!(
                    spec.category, *cat,
                    "/{name} deveria herdar a categoria do spec"
                );
            } else {
                // Comando local (swd, keys, uninstall, exit) — só validamos
                // que a categoria atribuída renderiza um label.
                let _ = category_label_pt(*cat);
            }
        }
    }

    #[test]
    fn palette_includes_all_visible_specs() {
        let items = slash_palette_items();
        for spec in commands::slash_command_specs() {
            if spec.hidden || !(spec.is_enabled)() {
                continue;
            }
            let display = spec.user_facing_name.unwrap_or(spec.name);
            assert!(
                items.iter().any(|(_, name, _)| name == display),
                "/{display} deveria aparecer na paleta",
            );
        }
    }

    #[test]
    fn category_label_pt_covers_all_variants() {
        for cat in [
            SlashCategory::Session,
            SlashCategory::Behavior,
            SlashCategory::Project,
            SlashCategory::Git,
            SlashCategory::Analysis,
            SlashCategory::System,
            SlashCategory::Plugins,
            SlashCategory::Custom,
        ] {
            let label = category_label_pt(cat);
            assert!(!label.is_empty(), "label PT-BR vazia para {cat:?}");
        }
    }

    #[test]
    fn build_palette_rows_interleaves_headers() {
        let items = slash_palette_items();
        let rows = build_palette_rows(&items, "");
        assert!(!rows.is_empty());
        // Primeira linha é sempre um Header.
        assert!(matches!(rows[0], PaletteRow::Header(_)));
        // Cada Header deve ser seguido por pelo menos um Command (sem header órfão).
        for (i, row) in rows.iter().enumerate() {
            if matches!(row, PaletteRow::Header(_)) {
                let next = rows.get(i + 1);
                assert!(
                    matches!(next, Some(PaletteRow::Command { .. })),
                    "header em {i} sem Command logo a seguir"
                );
            }
        }
        // Quantidade de Commands deve bater com items.
        let cmd_count = rows
            .iter()
            .filter(|r| matches!(r, PaletteRow::Command { .. }))
            .count();
        assert_eq!(cmd_count, items.len());
    }

    #[test]
    fn build_palette_rows_filter_drops_empty_categories() {
        let items = slash_palette_items();
        // "diff" → Git; só deve sobrar a categoria Git.
        let rows = build_palette_rows(&items, "diff");
        assert!(matches!(rows[0], PaletteRow::Header(_)));
        let header_count = rows
            .iter()
            .filter(|r| matches!(r, PaletteRow::Header(_)))
            .count();
        assert_eq!(header_count, 1, "filtro deve produzir apenas o header Git");
    }

    #[test]
    fn navigation_skips_headers() {
        let items = slash_palette_items();
        let rows = build_palette_rows(&items, "");
        // first_selectable nunca cai num Header.
        let first = first_selectable_row(&rows);
        assert!(matches!(rows[first], PaletteRow::Command { .. }));
        // Down a partir do primeiro Command pula para o próximo Command,
        // não para o próximo Header.
        let after_down = next_selectable_row(&rows, first);
        assert!(matches!(rows[after_down], PaletteRow::Command { .. }));
        // Up a partir do segundo Command volta para o primeiro (pulando Header se houver).
        let after_up = prev_selectable_row(&rows, after_down);
        assert_eq!(after_up, first);
        // Up no topo é idempotente.
        assert_eq!(prev_selectable_row(&rows, first), first);
    }

    #[test]
    fn coming_soon_set_matches_dispatcher_stub() {
        // Lista deve casar exatamente com a alternância no dispatcher TUI
        // (handle_tui_slash_command em main.rs). Se um destes for migrado para
        // implementação real no TUI, remova-o de ambos os lugares.
        for cmd in [
            "bughunter",
            "ultraplan",
            "teleport",
            "commit",
            "commit-push-pr",
            "pr",
            "issue",
        ] {
            assert!(
                is_command_coming_soon(cmd),
                "/{cmd} deveria estar marcado como em breve"
            );
        }
        assert!(!is_command_coming_soon("help"));
        assert!(!is_command_coming_soon("branch"));
        assert!(!is_command_coming_soon("update"));
    }

    #[test]
    fn enter_on_header_does_not_dispatch() {
        // Quando rows.get(selected) é Header, o handler não emite SlashCommand.
        // Validamos a forma direta: rows[0] é Header → caller deve não despachar.
        let items = slash_palette_items();
        let rows = build_palette_rows(&items, "");
        let header_idx = 0usize;
        assert!(matches!(rows[header_idx], PaletteRow::Header(_)));
        // Simula a checagem do handler:
        let dispatched = matches!(rows.get(header_idx), Some(PaletteRow::Command { .. }));
        assert!(!dispatched);
    }
}
