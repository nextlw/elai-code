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
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use pulldown_cmark::{CodeBlockKind, Event as MdEvent, Options, Parser, Tag, TagEnd};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Terminal;
use ratatui_cheese::fieldset::{Fieldset, FieldsetFill, FieldsetStyles};
use ratatui_cheese::help::{Binding as HelpBinding, Help, HelpStyles};
use ratatui_cheese::list::{
    List as CheeseList, ListItem as CheeseListItem, ListItemContext as CheeseListItemContext,
    ListState as CheeseListState,
};
use ratatui_cheese::spinner::{SpinnerState, SpinnerType};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme as SyntectTheme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

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

#[inline]
fn cheese_palette_from_theme(t: RatatuiTheme) -> ratatui_cheese::theme::Palette {
    crate::cheese_theme::palette_from_ratatui(&t)
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
    ToolCall {
        name: String,
        input: String,
    },
    ToolResult {
        ok: bool,
    },
    Usage {
        input_tokens: u32,
        output_tokens: u32,
    },
    Done,
    Error(String),
    SwdResult(crate::swd::SwdTransaction),
    SwdBatchResult(Vec<crate::swd::SwdTransaction>),
    #[allow(dead_code)]
    BudgetWarning {
        pct: f32,
        dimension: String,
    },
    #[allow(dead_code)]
    BudgetExhausted {
        reason: String,
    },
    #[allow(dead_code)]
    BudgetUpdate {
        pct: f32,
        cost_usd: f64,
    },
    CorrectionRetry {
        attempt: u8,
        max_attempts: u8,
    },
    /// Diff de uma tool de modificação de arquivo (`edit_file/write_file`).
    /// Enviado sempre em modo SWD partial (e full) para mostrar o que mudou.
    ToolDiff {
        path: String,
        hunks: Vec<crate::diff::DiffHunk>,
    },
    /// Resumo truncado do output de tools como `grep_search/glob_search` (máx 5 linhas).
    /// Renderizado inline no chat após o `ToolBatchEntry` correspondente.
    ToolOutputSummary { summary: String },
    SwdDiffPreview {
        actions: Vec<(String, Vec<crate::diff::DiffHunk>)>,
        reply_tx: std::sync::mpsc::SyncSender<bool>,
    },
    #[allow(dead_code)]
    SystemNote(String),
    /// Bloco ANSI cru (ex.: sprite Pokémon). Empurrado como `ChatEntry::AnsiBlock`.
    #[allow(dead_code)]
    AnsiBlock(String),
    /// Companion totalmente resolvido (após o LLM gerar nome/personalidade).
    /// Atualiza o painel de stats e o sprite no header.
    #[allow(dead_code)]
    CompanionLoaded(runtime::buddy::Companion),
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
    /// Bloco com sequências ANSI 256-color (ex.: sprite Pokémon). É parseado
    /// com `ansi-to-tui` para virar `Line`s coloridas no painel ratatui.
    AnsiBlock(String),
    SwdLogEntry {
        transactions: Vec<crate::swd::SwdTransaction>,
        mode: crate::swd::SwdLevel,
    },
    CorrectionRetryEntry {
        attempt: u8,
        max_attempts: u8,
    },
    /// Diff de tool de modificação de arquivo (`edit_file/write_file`).
    /// Renderizado inline no chat após o `ToolBatchEntry` correspondente.
    ToolDiff {
        path: String,
        hunks: Vec<crate::diff::DiffHunk>,
    },
    /// Resumo truncado do output de tools como `grep_search/glob_search` (máx 5 linhas).
    /// Renderizado inline no chat após o `ToolBatchEntry` correspondente.
    ToolOutputSummary { summary: String },
    SwdDiffEntry {
        path: String,
        hunks: Vec<crate::diff::DiffHunk>,
    },
    /// Linha viva de uma task. Mutável até `finished = true`; depois congela.
    /// Para tasks "multi-line" (ex.: `DeepResearch`), `events` acumula o histórico
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
        category_selected: usize,
        selected: usize,
        focus: SlashPaletteFocus,
    },
    FileMentionPicker {
        items: Vec<String>, // paths relativos do projeto (cache)
        filter: String,     // texto após o `@` (live)
        selected: usize,    // índice na lista filtrada
        anchor_pos: usize,  // posição do `@` no input (em chars, não bytes)
    },
    SessionPicker {
        items: Vec<(String, Option<String>, usize)>,
        selected: usize,
    },
    ScriptPicker {
        items: Vec<(String, String)>, // (nome display, path absoluto)
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
    /// Legacy `OpenAI` key setup wizard (kept for compatibility, accessible via `/auth` if needed).
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
    /// Modal para colar a API key do `DeepResearch` (input mascarado).
    DeepResearchKeyInput {
        input: String,
        cursor: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashPaletteFocus {
    Categories,
    Commands,
}

/// Which authentication method the user selected in the `AuthPicker`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethodChoice {
    ClaudeAiOAuth,
    ConsoleOAuth,
    SsoOAuth,
    CodexOAuth,
    PasteApiKey,
    PasteAuthToken,
    /// API key da `OpenAI` (sk-... ou sk-proj-...). Salva como
    /// `AuthMethod::OpenAiApiKey` no credentials store.
    PasteOpenAiKey,
    PasteOpenCodeGoKey,
    PasteXaiKey,
    UseBedrock,
    UseVertex,
    UseFoundry,
    ImportClaudeCode,
    ImportCodex,
    LegacyElai,
}

/// Group of authentication methods belonging to a single provider.
#[derive(Debug, Clone)]
pub(crate) struct ProviderAuthGroup {
    name: &'static str,
    id: &'static str,
    methods: &'static [AuthMethodChoice],
}

fn opencode_zen_provider_group() -> ProviderAuthGroup {
    ProviderAuthGroup {
        name: "OpenCode Zen (free)",
        id: "opencode-zen",
        methods: &[],
    }
}

fn provider_auth_groups() -> Vec<ProviderAuthGroup> {
    vec![
        ProviderAuthGroup {
            name: "Anthropic",
            id: "anthropic",
            methods: &[
                AuthMethodChoice::ClaudeAiOAuth,
                AuthMethodChoice::ConsoleOAuth,
                AuthMethodChoice::SsoOAuth,
                AuthMethodChoice::PasteApiKey,
                AuthMethodChoice::PasteAuthToken,
                AuthMethodChoice::ImportClaudeCode,
            ],
        },
        ProviderAuthGroup {
            name: "OpenAI",
            id: "openai",
            methods: &[
                AuthMethodChoice::CodexOAuth,
                AuthMethodChoice::PasteOpenAiKey,
                AuthMethodChoice::ImportCodex,
            ],
        },
        ProviderAuthGroup {
            name: "OpenCode Go",
            id: "opencode-go",
            methods: &[AuthMethodChoice::PasteOpenCodeGoKey],
        },
        opencode_zen_provider_group(),
        ProviderAuthGroup {
            name: "xAI (Grok)",
            id: "xai",
            methods: &[AuthMethodChoice::PasteXaiKey],
        },
        ProviderAuthGroup {
            name: "AWS Bedrock",
            id: "bedrock",
            methods: &[AuthMethodChoice::UseBedrock],
        },
        ProviderAuthGroup {
            name: "Google Vertex AI",
            id: "vertex",
            methods: &[AuthMethodChoice::UseVertex],
        },
        ProviderAuthGroup {
            name: "Azure Foundry",
            id: "foundry",
            methods: &[AuthMethodChoice::UseFoundry],
        },
        ProviderAuthGroup {
            name: "Elai (legacy)",
            id: "elai-legacy",
            methods: &[AuthMethodChoice::LegacyElai],
        },
    ]
}

fn is_provider_connected(group: &ProviderAuthGroup) -> bool {
    match group.id {
        "anthropic" => {
            std::env::var_os("ANTHROPIC_API_KEY").is_some_and(|v| !v.is_empty())
                || std::env::var_os("ANTHROPIC_AUTH_TOKEN").is_some_and(|v| !v.is_empty())
                || matches!(
                    runtime::load_auth_method().ok().flatten(),
                    Some(
                        runtime::AuthMethod::ClaudeAiOAuth { .. }
                            | runtime::AuthMethod::ConsoleApiKey { .. }
                            | runtime::AuthMethod::AnthropicAuthToken { .. }
                    )
                )
        }
        "openai" => {
            std::env::var_os("OPENAI_API_KEY").is_some_and(|v| !v.is_empty())
                || matches!(
                    runtime::load_auth_method().ok().flatten(),
                    Some(
                        runtime::AuthMethod::OpenAiApiKey { .. }
                            | runtime::AuthMethod::OpenAiCodexOAuth { .. }
                    )
                )
        }
        "opencode-go" => std::env::var_os("OPENCODE_GO_API_KEY").is_some_and(|v| !v.is_empty()),
        // The upstream OpenCode client uses Bearer `public` for free Zen models.
        "opencode-zen" => true,
        "xai" => std::env::var_os("XAI_API_KEY").is_some_and(|v| !v.is_empty()),
        "bedrock" => {
            matches!(
                runtime::load_auth_method().ok().flatten(),
                Some(runtime::AuthMethod::Bedrock)
            )
        }
        "vertex" => {
            matches!(
                runtime::load_auth_method().ok().flatten(),
                Some(runtime::AuthMethod::Vertex)
            )
        }
        "foundry" => {
            matches!(
                runtime::load_auth_method().ok().flatten(),
                Some(runtime::AuthMethod::Foundry)
            )
        }
        _ => false,
    }
}

/// Step state machine for the `AuthPicker` overlay.
#[derive(Debug)]
pub enum AuthStep {
    /// Provider selection (first step, after `/auth`).
    ProviderList { selected: usize },
    /// List of methods for the chosen provider.
    MethodList {
        provider: ProviderAuthGroup,
        selected: usize,
        claude_code_detected: bool,
        codex_detected: bool,
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
        model: Option<String>,
    },
    /// Error: show message and ask Esc/Enter.
    Failed { error: String },
    /// Provider already connected - show available models.
    ConnectedModels {
        provider: ProviderAuthGroup,
        selected: usize,
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
    pub provider: WizardProvider,
    pub model: String,
    pub permission_mode: String,
    pub features: runtime::FeatureFlags,
}

impl Default for WizardState {
    fn default() -> Self {
        Self {
            provider: WizardProvider::Anthropic,
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
    /// Provider/channel selection.
    Provider { selected: usize },
    /// Model selection for the selected provider/channel.
    Model {
        provider: WizardProvider,
        selected: usize,
    },
    /// Permission mode selection (3 choices).
    Permissions { selected: usize },
    /// Optional defaults toggle (auto-update / telemetry / indexing).
    /// `focused` is 0..2 indicating which toggle the cursor is on.
    Defaults { focused: usize },
    /// Summary + persist — press Enter to close.
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiChannel {
    Codex,
    ApiKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardProvider {
    Anthropic,
    OpenAi(OpenAiChannel),
    Xai,
    OpenCodeGo,
}

const WIZARD_PROVIDER_OPTIONS: [WizardProvider; 5] = [
    WizardProvider::Anthropic,
    WizardProvider::OpenAi(OpenAiChannel::Codex),
    WizardProvider::OpenAi(OpenAiChannel::ApiKey),
    WizardProvider::Xai,
    WizardProvider::OpenCodeGo,
];

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

#[allow(clippy::struct_excessive_bools)]
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
    pub recent_sessions: Vec<(String, Option<String>, usize)>,
    pub indexed_paths: Vec<String>, // cache lazy de `.elai/index/metadata.json` ou re-walk
    pub should_quit: bool,
    /// Quando `true`, o próximo `render` força `terminal.clear()` antes do
    /// draw — usado depois de overlays de tela cheia (ex.: buddy picker) que
    /// invalidam o buffer interno do ratatui.
    pub force_clear: bool,
    /// Companion ativo do usuário — carregado em background na inicialização.
    /// Usado pelo header (sprite) e pelo painel lateral (stats).
    pub companion: Option<runtime::buddy::Companion>,
    pub history: Vec<String>,
    pub history_index: Option<usize>,
    pub history_backup: String,
    /// Olhos animados para o rodapé "thinking" (olhinhos mexendo).
    pub think_eyes_spinner: SpinnerState,
    pub tool_spinner: SpinnerState,
    pub dots_spinner: SpinnerState,
    pub last_tick: Instant,
    /// Índice aleatório da frase atual no rodapé thinking.
    pub caption_idx: usize,
    /// Instante em que a próxima frase deve ser sorteada.
    pub caption_deadline: Instant,
    pub read_mode: bool,
    /// Captura de mouse no terminal. Padrão: `true` (scroll com a roda funciona
    /// direto). Para selecionar/copiar texto, segure **SHIFT** e arraste — todos
    /// os terminais modernos fazem o "SHIFT-bypass" do protocolo de mouse.
    /// Configurável via `ELAI_TUI_MOUSE_CAPTURE=0` (terminais sem SHIFT-bypass).
    pub mouse_capture_enabled: bool,
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
    /// `true` para header completo (ASCII art + info); `false` para modo compacto (só info).
    /// Escondido automaticamente após a 1ª mensagem do usuário.
    pub show_header: bool,
    /// `true` se o header está em modo compacto (sem ASCII art).
    /// Toggle via Ctrl+H.
    pub header_compact: bool,
    /// Mensagens digitadas enquanto `thinking = true`, aguardando envio.
    pub message_queue: std::collections::VecDeque<String>,
    /// Próxima mensagem a ser despachada logo que `thinking` voltar a `false`.
    pub pending_outgoing: Option<String>,
    /// `true` se a mensagem atual contém a keyword `ultrathink`.
    pub ultrathink_active: bool,
    /// Contador sequencial de pastes longos (≥3 linhas) — gera o `#N` exibido
    /// no placeholder `[Pasted text #N +M lines]` no input. Nunca é decrementado
    /// para que histórico (↑) e mensagens enfileiradas continuem expandindo
    /// corretamente após múltiplos envios.
    pub paste_counter: u32,
    /// Conteúdo real de cada paste longo, indexado pelo seu `#N`. Mantido pela
    /// sessão inteira para que placeholders em mensagens enfileiradas, no
    /// histórico e re-editadas continuem expansíveis.
    pub pasted_contents: std::collections::HashMap<u32, String>,
    /// Texto do toast atualmente exibido (se houver).
    pub toast_message: Option<String>,
    /// Instante em que o toast deve sumir (auto-dismiss após 2 segundos).
    pub toast_deadline: Option<std::time::Instant>,
    /// Snapshot do último buffer renderizado (linha+coluna→célula). Usado para
    /// extrair texto da seleção via mouse de forma terminal-agnóstica.
    /// `None` antes do primeiro frame.
    pub last_buffer: Option<Buffer>,
    /// Posição (col,row) do `MouseDown` esquerdo. `Some` durante drag ativo.
    pub drag_anchor: Option<(u16, u16)>,
    /// Posição (col,row) atual do cursor durante drag. `Some` enquanto arrasta.
    pub drag_current: Option<(u16, u16)>,
}

/// Spinner de "rosto pensando" com timing orgânico.
///
/// Timing real via repetição de frames (base = 80ms por tick):
///   piscada rápida     = 1 tick  (80ms)
///   saccade (movimento)= 1 tick  (80ms)
///   segurar direção    = 3-5 ticks (240-400ms)
///   descanso neutro    = 5-10 ticks (400-800ms)
///   spacing out        = 10-14 ticks (800ms-1.1s)
///
/// Padrões humanos incluídos: double-blink após stare longo,
/// micro-glance involuntário, re-olhar para o mesmo ponto,
/// squint de concentração, "flash" de ideia.
pub fn make_mega_eyes_spinner() -> SpinnerState {
    use std::time::Duration;

    // Macro para repetir um frame n vezes (simula duração = n * 80ms)
    macro_rules! rep {
        ($frame:expr, $n:expr) => {
            std::iter::repeat_n($frame.to_string(), $n)
        };
    }

    let frames: Vec<String> = std::iter::empty()
        // ── Fase 1: Acomodando (2.08s) ──────────────────────────────────────
        .chain(rep!("( · · )", 5))  // 400ms — neutro inicial
        .chain(rep!("( - - )", 1))  // 80ms  — piscada rápida
        .chain(rep!("( · · )", 6))  // 480ms — voltou, acomodando
        .chain(rep!("( - - )", 1))  // 80ms  — double-blink (natural)
        .chain(rep!("( · · )", 4))  // 320ms — estabilizou
        // ── Fase 2: Olha pra direita (0.8s) ─────────────────────────────────
        .chain(rep!("( > > )", 1))  // 80ms  — saccade rápido
        .chain(rep!("( > > )", 3))  // 240ms — segura olhando
        .chain(rep!("( · · )", 1))  // 80ms  — volta
        .chain(rep!("( · · )", 4))  // 320ms — descanso
        // ── Fase 3: Spacing out (2.88s) ──────────────────────────────────────
        .chain(rep!("( · · )", 10)) // 800ms — olhar vago
        .chain(rep!("( - - )", 1))  // 80ms  — piscada
        .chain(rep!("( · · )", 8))  // 640ms — continua espacando
        .chain(rep!("( - - )", 1))  // 80ms  — piscada
        .chain(rep!("( - - )", 1))  // 80ms  — double-blink (após stare longo)
        .chain(rep!("( · · )", 4))  // 320ms — voltou
        // ── Fase 4: Pensando pra cima (1.44s) ───────────────────────────────
        .chain(rep!("( ^ ^ )", 1))  // 80ms  — saccade pra cima
        .chain(rep!("( ^ ^ )", 5))  // 400ms — processando
        .chain(rep!("( · · )", 2))  // 160ms — voltou
        .chain(rep!("( ^ ^ )", 2))  // 160ms — re-olha (reconfirmando pensamento)
        .chain(rep!("( · · )", 3))  // 240ms — volta final
        // ── Fase 5: Scan rápido esquerda-direita (0.88s) ────────────────────
        .chain(rep!("( < < )", 1))  // 80ms  — esquerda
        .chain(rep!("( < < )", 2))  // 160ms — segura
        .chain(rep!("( · · )", 1))  // 80ms  — centro
        .chain(rep!("( > > )", 1))  // 80ms  — direita
        .chain(rep!("( · · )", 3))  // 240ms — descansa
        .chain(rep!("( - - )", 1))  // 80ms  — piscada
        .chain(rep!("( · · )", 2))  // 160ms
        // ── Fase 6: Olhos arregalados — processa algo (1.6s) ────────────────
        .chain(rep!("( o o )", 1))  // 80ms  — flash de surpresa
        .chain(rep!("( o o )", 5))  // 400ms — processando com foco
        .chain(rep!("( - - )", 1))  // 80ms  — pisca saindo do estado
        .chain(rep!("( · · )", 5))  // 400ms — normalizou
        .chain(rep!("( - - )", 1))  // 80ms  — piscada de confirmação
        .chain(rep!("( · · )", 3))  // 240ms
        // ── Fase 7: Squint de concentração (1.28s) ──────────────────────────
        .chain(rep!("( - - )", 1))  // 80ms  — fecha (concentrado, não piscada)
        .chain(rep!("( - - )", 5))  // 400ms — squinting
        .chain(rep!("( · · )", 1))  // 80ms  — abre
        .chain(rep!("( - - )", 1))  // 80ms  — re-fecha (frustração leve)
        .chain(rep!("( · · )", 4))  // 320ms — desiste do squint
        // ── Fase 8: Descanso longo com micro-glance (2.48s) ─────────────────
        .chain(rep!("( · · )", 9))  // 720ms — olhar vago
        .chain(rep!("( > > )", 1))  // 80ms  — micro-glance involuntário
        .chain(rep!("( · · )", 7))  // 560ms — voltou sem querer
        .chain(rep!("( - - )", 1))  // 80ms  — piscada
        .chain(rep!("( - - )", 1))  // 80ms  — double-blink
        .chain(rep!("( · · )", 4))  // 320ms — acomodou
        // ── Fase 9: Olha pra baixo (0.8s) ───────────────────────────────────
        .chain(rep!("( v v )", 1))  // 80ms  — saccade pra baixo
        .chain(rep!("( v v )", 3))  // 240ms — olhando abaixo
        .chain(rep!("( · · )", 2))  // 160ms — volta
        .chain(rep!("( - - )", 1))  // 80ms  — piscada
        .chain(rep!("( · · )", 2))  // 160ms
        // ── Fase 10: Flash de ideia + settling (1.68s) ──────────────────────
        .chain(rep!("( * * )", 1))  // 80ms  — flash de ideia!
        .chain(rep!("( * * )", 2))  // 160ms — segura a centelha
        .chain(rep!("( o o )", 1))  // 80ms  — olhos arregalados de empolgação
        .chain(rep!("( · · )", 4))  // 320ms — calma
        .chain(rep!("( - - )", 1))  // 80ms  — piscada
        .chain(rep!("( · · )", 6))  // 480ms — settle final antes de loopear
        .collect();

    SpinnerState::custom(frames, Duration::from_millis(80))
}

impl UiApp {
    pub fn new(
        model: String,
        permission_mode: String,
        session_id: String,
        recent_sessions: Vec<(String, Option<String>, usize)>,
        swd_level: Arc<AtomicU8>,
    ) -> Self {
        // Padrão: mouse capture LIGADO — scroll com a roda funciona out-of-the-box.
        // Para selecionar/copiar texto, o usuário segura SHIFT enquanto arrasta:
        // praticamente todos os terminais modernos (iTerm2, Terminal.app, Alacritty,
        // Kitty, Wezterm, GNOME Terminal, Konsole, Windows Terminal) fazem o
        // "SHIFT-bypass" do protocolo de mouse da aplicação e ativam a seleção
        // nativa do terminal automaticamente.
        // `ELAI_TUI_MOUSE_CAPTURE=0` desliga o capture (para terminais raros que
        // não suportam SHIFT-bypass — perde scroll, ganha seleção livre).
        let mouse_capture_enabled = env::var("ELAI_TUI_MOUSE_CAPTURE")
            .ok()
            .map_or(true, |v| !matches!(v.trim(), "0" | "false" | "no" | "off"));

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
            force_clear: false,
            companion: load_initial_companion(),
            history: Vec::new(),
            history_index: None,
            history_backup: String::new(),
            think_eyes_spinner: make_mega_eyes_spinner(),
            tool_spinner: SpinnerState::new(SpinnerType::Ellipsis),
            dots_spinner: SpinnerState::new(SpinnerType::Ellipsis),
            last_tick: Instant::now(),
            caption_idx: fastrand::usize(..),
            caption_deadline: Instant::now(),
            read_mode: false,
            mouse_capture_enabled,
            swd_level,
            budget_pct: 0.0,
            budget_cost_usd: 0.0,
            budget_enabled: false,
            tips: { crate::tips::load_tips() },
            tips_order: Vec::new(),
            tips_cursor: 0,
            show_tips: true,
            show_header: true,
            header_compact: false,
            message_queue: std::collections::VecDeque::new(),
            pending_outgoing: None,
            ultrathink_active: false,
            paste_counter: 0,
            pasted_contents: std::collections::HashMap::new(),
            toast_message: None,
            toast_deadline: None,
            last_buffer: None,
            drag_anchor: None,
            drag_current: None,
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
        self.header_compact = false; // Restaura header completo após /clear
        self.toast_message = None;
        self.toast_deadline = None;
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
        const CAPTION_DELAYS_MS: [u64; 5] = [1_000, 2_000, 5_000, 8_000, 10_000];

        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick);
        self.last_tick = now;
        if self.thinking {
            self.think_eyes_spinner.tick(elapsed);
            self.dots_spinner.tick(elapsed);
            if now >= self.caption_deadline {
                self.caption_idx = fastrand::usize(..);
                let delay_ms = CAPTION_DELAYS_MS[fastrand::usize(..CAPTION_DELAYS_MS.len())];
                self.caption_deadline = now + std::time::Duration::from_millis(delay_ms);
            }
        }
        // Auto-dismiss do toast após 2 segundos.
        if let Some(deadline) = self.toast_deadline {
            if now >= deadline {
                self.toast_message = None;
                self.toast_deadline = None;
            }
        }
    }

    pub fn push_chat(&mut self, entry: ChatEntry) {
        if matches!(entry, ChatEntry::UserMessage(_)) {
            self.show_tips = false;
            self.header_compact = true;
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

    #[allow(clippy::too_many_lines)]
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
                if let Some(ChatEntry::ThinkingBlock {
                    text: buf,
                    finished: false,
                }) = self.chat.last_mut()
                {
                    buf.push_str(&text);
                } else {
                    self.chat.push(ChatEntry::ThinkingBlock {
                        text,
                        finished: false,
                    });
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
                if let Some(ChatEntry::ToolBatchEntry {
                    items,
                    closed: false,
                }) = self.chat.last_mut()
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
                        item.status = if ok {
                            ToolItemStatus::Ok
                        } else {
                            ToolItemStatus::Err
                        };
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
                let appended = matches!(self.chat.last_mut(), Some(ChatEntry::SwdLogEntry { .. }));
                if appended {
                    if let Some(ChatEntry::SwdLogEntry { transactions, .. }) = self.chat.last_mut()
                    {
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
            TuiMsg::CorrectionRetry {
                attempt,
                max_attempts,
            } => {
                self.push_chat(ChatEntry::CorrectionRetryEntry {
                    attempt,
                    max_attempts,
                });
            }
            TuiMsg::ToolDiff { path, hunks } => {
                self.push_chat(ChatEntry::ToolDiff { path, hunks });
            }
            TuiMsg::ToolOutputSummary { summary } => {
                self.push_chat(ChatEntry::ToolOutputSummary { summary });
            }
            TuiMsg::SwdDiffPreview { actions, reply_tx } => {
                let action_count = actions.len();
                for (path, hunks) in actions {
                    self.push_chat(ChatEntry::SwdDiffEntry { path, hunks });
                }
                self.overlay = Some(OverlayKind::SwdConfirmApply {
                    action_count,
                    reply_tx,
                });
            }
            TuiMsg::SystemNote(note) => {
                self.push_chat(ChatEntry::SystemNote(note));
            }
            TuiMsg::AnsiBlock(ansi) => {
                self.push_chat(ChatEntry::AnsiBlock(ansi));
            }
            TuiMsg::CompanionLoaded(c) => {
                self.companion = Some(c);
            }
            TuiMsg::TaskProgress {
                task_id,
                label,
                msg,
            } => {
                let multiline = is_multiline_task(&label);
                // Scan reverso curto (≤ 8 entries) — cobre o caso comum onde
                // tool calls / assistant text se intercalam com updates.
                let found = self
                    .chat
                    .iter_mut()
                    .rev()
                    .take(8)
                    .find(|e| matches_task_progress(e, &task_id));
                if let Some(ChatEntry::TaskProgress {
                    msg: m,
                    label: l,
                    events,
                    ..
                }) = found
                {
                    if multiline {
                        // Acumula no histórico, dedup do último.
                        if events.last().map(String::as_str) != Some(msg.as_str()) {
                            events.push(msg.clone());
                        }
                    }
                    *m = msg;
                    *l = label;
                } else {
                    let events = if multiline {
                        vec![msg.clone()]
                    } else {
                        Vec::new()
                    };
                    self.push_chat(ChatEntry::TaskProgress {
                        task_id,
                        label,
                        msg,
                        events,
                        finished: false,
                        status: None,
                    });
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
        if !line.is_empty() && self.history.last().map(std::string::String::as_str) != Some(&line) {
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
            .map_or(self.input.len(), |(i, _)| i);
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
                .map_or(self.input.len(), |(i, _)| i);
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

    /// Reseta o estado de paste (placeholders `[Pasted text #N ...]`) para que
    /// `/clear` libere memória e a próxima colagem volte a numerar a partir de `#1`.
    /// Agent C estenderá este método para limpar também `pasted_attachments`/
    /// `attachment_counter` quando esses campos existirem.
    pub fn clear_paste_state(&mut self) {
        self.pasted_contents.clear();
        self.paste_counter = 0;
    }

    #[allow(dead_code)]
    fn take_input(&mut self) -> String {
        let text = self.input.clone();
        self.clear_input();
        text
    }

    #[allow(clippy::unused_self)]
    fn active_provider_model_items(&self) -> Vec<String> {
        // Mostra modelos do provedor a que o modelo ATUAL pertence —
        // logar em múltiplos provedores não mistura os modelos.
        if let Some(md) = api::metadata_for_model(&self.model) {
            match md.provider {
                api::ProviderKind::OpenCodeGo => {
                    return models_for_provider(WizardProvider::OpenCodeGo);
                }
                api::ProviderKind::Xai => {
                    return models_for_provider(WizardProvider::Xai);
                }
                api::ProviderKind::OpenAi => {
                    if matches!(
                        runtime::load_auth_method().ok().flatten(),
                        Some(runtime::AuthMethod::OpenAiCodexOAuth { .. })
                    ) {
                        return models_for_provider(WizardProvider::OpenAi(OpenAiChannel::Codex));
                    }
                    return models_for_provider(WizardProvider::OpenAi(OpenAiChannel::ApiKey));
                }
                // Anthropic, Ollama, LmStudio — mostra Anthropic + ant overrides no topo
                _ => return anthropic_model_items(),
            }
        }
        anthropic_model_items()
    }

    fn filter_model_items(items: &[String], filter: &str) -> Vec<String> {
        let f = filter.to_lowercase();
        items
            .iter()
            .filter(|m| f.is_empty() || m.to_lowercase().contains(&f))
            .cloned()
            .collect()
    }

    pub fn open_model_picker(&mut self) {
        let items = self.active_provider_model_items();
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
        self.overlay = Some(OverlayKind::LocalePicker {
            items: locales,
            selected,
        });
    }

    pub fn open_slash_palette(&mut self) {
        let items = slash_palette_items();
        self.overlay = Some(OverlayKind::SlashPalette {
            filter: String::new(),
            items,
            category_selected: 0,
            selected: 0,
            focus: SlashPaletteFocus::Commands,
        });
    }

    pub fn open_session_picker(&mut self) {
        self.overlay = Some(OverlayKind::SessionPicker {
            items: self.recent_sessions.clone(),
            selected: 0,
        });
    }

    pub fn open_script_picker(&mut self, items: Vec<(String, String)>) {
        self.overlay = Some(OverlayKind::ScriptPicker { items, selected: 0 });
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
        self.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::ProviderList { selected: 0 },
        });
    }

    pub fn open_opencode_zen_free_models(&mut self) {
        self.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::ConnectedModels {
                provider: opencode_zen_provider_group(),
                selected: 0,
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
        "verify" => "Verificar codebase vs memória",
        "theme" => "Ajustar tema (cinza secundário)",
        "provedores" => "Conectar provedores / gerenciar API keys",
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
            SlashCategory::Session,
            "provedores".into(),
            "Conectar provedores / gerenciar API keys".into(),
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
        (
            SlashCategory::Session,
            "logout".into(),
            "Clear saved authentication credentials".into(),
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
        "bughunter" | "ultraplan" | "teleport" | "commit" | "commit-push-pr" | "pr" | "issue"
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
fn build_palette_rows(items: &[(SlashCategory, String, String)], filter: &str) -> Vec<PaletteRow> {
    type CatBucket<'a> = (SlashCategory, Vec<(&'a String, &'a String)>);
    let filtered = filter_slash_items(items, filter);
    let mut by_cat: std::collections::BTreeMap<u8, CatBucket> = std::collections::BTreeMap::new();
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

/// Constrói as colunas da paleta (categorias + comandos filtrados por categoria).
fn build_palette_columns(
    items: &[(SlashCategory, String, String)],
    filter: &str,
) -> Vec<(SlashCategory, Vec<(String, String)>)> {
    type CatBucket = (SlashCategory, Vec<(String, String)>);
    let filtered = filter_slash_items(items, filter);
    let mut by_cat: std::collections::BTreeMap<u8, CatBucket> = std::collections::BTreeMap::new();
    for (cat, cmd, desc) in &filtered {
        by_cat
            .entry(cat.order())
            .or_insert_with(|| (*cat, Vec::new()))
            .1
            .push((cmd.clone(), desc.clone()));
    }
    by_cat.into_values().collect()
}

/// Próximo índice selecionável (pulando `Header`). Mantém posição se já está no fim.
#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
fn first_selectable_row(rows: &[PaletteRow]) -> usize {
    rows.iter()
        .position(|r| matches!(r, PaletteRow::Command { .. }))
        .unwrap_or(0)
}

// ─── Terminal lifecycle helpers ───────────────────────────────────────────────

/// Enter alternate screen + raw mode (sem captura de mouse por padrão — ver `apply_mouse_capture`).
pub fn enter_tui(stdout: &mut impl io::Write) -> io::Result<()> {
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    Ok(())
}

/// Ativa ou desativa a captura de mouse no backend crossterm (rolagem do chat via roda).
/// Preferência do usuário fica em `UiApp::mouse_capture_enabled`; em modo leitura o terminal
/// fica sempre sem captura para liberar seleção/cópia nativa.
pub fn apply_mouse_capture(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    enabled: bool,
) -> io::Result<()> {
    let w = terminal.backend_mut();
    if enabled {
        execute!(w, EnableMouseCapture)?;
    } else {
        execute!(w, DisableMouseCapture)?;
    }
    Ok(())
}

/// Restore terminal on exit (always call even on error).
pub fn leave_tui(stdout: &mut impl io::Write) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        stdout,
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    Ok(())
}

// ─── Main TUI loop ────────────────────────────────────────────────────────────

/// Expande placeholders `[Pasted text #N +M lines]` no `text` substituindo
/// pelo conteúdo original guardado em `map`. Placeholders cujo `#N` não esteja
/// no mapa, ou que estejam malformados (ex.: usuário editou no meio), são
/// preservados literalmente. Operação O(n) sem regex.
pub fn expand_paste_placeholders(
    text: &str,
    map: &std::collections::HashMap<u32, String>,
) -> String {
    const PREFIX: &str = "[Pasted text #";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(PREFIX) {
        out.push_str(&rest[..pos]);
        let after_prefix = &rest[pos + PREFIX.len()..];
        // Tenta parsear `<digits> +<digits> lines]`; se não casar, copia o
        // PREFIX literal e segue do próximo char para tentar de novo.
        let (parsed_n, after_n) = take_u32(after_prefix);
        if let Some(n) = parsed_n {
            if let Some(after_sep) = after_n.strip_prefix(" +") {
                let (parsed_m, after_m) = take_u32(after_sep);
                if parsed_m.is_some() {
                    if let Some(after_close) = after_m.strip_prefix(" lines]") {
                        if let Some(content) = map.get(&n) {
                            out.push_str(content);
                            rest = after_close;
                            continue;
                        }
                    }
                }
            }
        }
        out.push_str(PREFIX);
        rest = after_prefix;
    }
    out.push_str(rest);
    out
}

fn take_u32(s: &str) -> (Option<u32>, &str) {
    let end = s.bytes().take_while(u8::is_ascii_digit).count();
    if end == 0 {
        (None, s)
    } else {
        (s[..end].parse::<u32>().ok(), &s[end..])
    }
}

/// Result returned when the user submits input or picks an action inside the TUI.
pub enum TuiAction {
    SendMessage(String),
    SetModel(String),
    SetPermissions(String),
    ResumeSession(String),
    RunScript(String), // path absoluto do script
    SlashCommand(String),
    CopyToClipboard(String),
    EnterReadMode,
    ExitReadMode,
    /// Reaplica `UiApp::mouse_capture_enabled` no terminal (após toggle ou sair do modo leitura).
    #[allow(dead_code)]
    SyncMouseCapture,
    SetupComplete,
    AuthComplete {
        label: String,
        model: Option<String>,
    },
    Uninstall,
    Quit,
    None,
}

fn drain_runtime_messages(app: &mut UiApp, msg_rx: &mpsc::Receiver<TuiMsg>) {
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
}

fn check_pending_permission(app: &mut UiApp, perm_rx: &mpsc::Receiver<PermRequest>) {
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
}

/// Drive a single frame-cycle: poll events, update state, return an action.
pub fn poll_and_handle(
    app: &mut UiApp,
    msg_rx: &mpsc::Receiver<TuiMsg>,
    perm_rx: &mpsc::Receiver<PermRequest>,
) -> TuiAction {
    // Drain runtime messages first (non-blocking).
    drain_runtime_messages(app, msg_rx);

    // Despacha mensagem enfileirada assim que thinking voltou a false.
    if let Some(msg) = app.pending_outgoing.take() {
        app.ultrathink_active = crate::ultrathink::message_contains_ultrathink_keyword(&msg);
        return TuiAction::SendMessage(msg);
    }

    // Check for permission requests (non-blocking).
    check_pending_permission(app, perm_rx);

    // Poll terminal events with short timeout so the loop stays responsive.
    if !event::poll(Duration::from_millis(50)).unwrap_or(false) {
        return TuiAction::None;
    }

    let Ok(ev) = event::read() else {
        return TuiAction::None;
    };

    match ev {
        Event::Mouse(mouse) => {
            if !app.read_mode {
                match mouse.kind {
                    MouseEventKind::ScrollUp => app.scroll_chat_up(2),
                    MouseEventKind::ScrollDown => app.scroll_chat_down(2),
                    // Início de seleção via drag: ancora a posição inicial.
                    MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                        app.drag_anchor = Some((mouse.column, mouse.row));
                        app.drag_current = Some((mouse.column, mouse.row));
                    }
                    // Atualiza a posição corrente do drag para feedback visual.
                    MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                        if app.drag_anchor.is_some() {
                            app.drag_current = Some((mouse.column, mouse.row));
                        }
                    }
                    // Fim do drag: se houve movimento, extrai o texto da seleção
                    // a partir do snapshot do último buffer renderizado e copia
                    // para a área de transferência.
                    MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
                        let anchor = app.drag_anchor.take();
                        let current = app.drag_current.take();
                        if let (Some(a), Some(c)) = (anchor, current) {
                            // Considera drag apenas se moveu pelo menos 1 célula.
                            let moved = a != c;
                            if moved {
                                if let Some(text) = extract_buffer_text(app, a, c) {
                                    if !text.trim().is_empty() {
                                        app.toast_message = Some(
                                            rust_i18n::t!("tui.toast.copied_to_clipboard")
                                                .into_owned(),
                                        );
                                        app.toast_deadline = Some(
                                            std::time::Instant::now()
                                                + std::time::Duration::from_secs(2),
                                        );
                                        return TuiAction::CopyToClipboard(text);
                                    }
                                }
                            }
                        }
                    }
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

        Event::Paste(pasted) => {
            if app.read_mode {
                return TuiAction::None;
            }
            // Bracketed paste: aceita no campo principal e nos modais com campo
            // de texto (auth secret e deep-research key).
            // No auth token removemos quebras de linha e espaços extras de borda.
            match app.overlay.take() {
                Some(OverlayKind::AuthPicker {
                    step:
                        AuthStep::PasteSecret {
                            method,
                            input,
                            cursor,
                            masked,
                        },
                }) => {
                    let mut input = input;
                    let mut cursor = cursor;
                    let cleaned = pasted
                        .replace("\r\n", "\n")
                        .replace('\r', "\n")
                        .replace('\n', "")
                        .trim()
                        .to_string();
                    if !cleaned.is_empty() {
                        let idx = input
                            .char_indices()
                            .nth(cursor)
                            .map_or(input.len(), |(i, _)| i);
                        input.insert_str(idx, &cleaned);
                        cursor += cleaned.chars().count();
                    }
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::PasteSecret {
                            method,
                            input,
                            cursor,
                            masked,
                        },
                    });
                    return TuiAction::None;
                }
                Some(OverlayKind::DeepResearchKeyInput { mut input, mut cursor }) => {
                    let cleaned = pasted
                        .replace("\r\n", "\n")
                        .replace('\r', "\n")
                        .replace('\n', "")
                        .trim()
                        .to_string();
                    if !cleaned.is_empty() {
                        let idx = input
                            .char_indices()
                            .nth(cursor)
                            .map_or(input.len(), |(i, _)| i);
                        input.insert_str(idx, &cleaned);
                        cursor += cleaned.chars().count();
                    }
                    app.overlay = Some(OverlayKind::DeepResearchKeyInput { input, cursor });
                    return TuiAction::None;
                }
                Some(other) => {
                    app.overlay = Some(other);
                    return TuiAction::None;
                }
                None => {}
            }
            // Normaliza CRLF/CR para '\n' para manter consistência no editor multilinha.
            let normalized = pasted.replace("\r\n", "\n").replace('\r', "\n");
            if normalized.is_empty() {
                return TuiAction::None;
            }
            // Pastes ≥3 linhas viram placeholder `[Pasted text #N +M lines]`
            // (mesmo padrão do claude-code/opencode): mantém o input enxuto,
            // evita custo de wrap/render em cada keystroke depois e o conteúdo
            // real é guardado em `pasted_contents` para expansão no envio.
            let line_count = normalized.lines().count().max(1);
            let to_insert: String = if line_count >= 3 {
                app.paste_counter += 1;
                let n = app.paste_counter;
                let placeholder = format!("[Pasted text #{n} +{line_count} lines]");
                app.pasted_contents.insert(n, normalized);
                placeholder
            } else {
                normalized
            };
            // `insert_str` em chamada única: `String::insert` char-a-char era
            // O(n²) e travava o terminal antes de voltar ao `event::poll`.
            let idx = app
                .input
                .char_indices()
                .nth(app.cursor_col)
                .map_or(app.input.len(), |(i, _)| i);
            app.input.insert_str(idx, &to_insert);
            app.cursor_col += to_insert.chars().count();
            app.history_index = None;
            TuiAction::None
        }

        _ => TuiAction::None,
    }
}

#[allow(clippy::too_many_lines)]
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
        (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
            // Ctrl+H: alterna entre header completo e compacto
            app.header_compact = !app.header_compact;
            // Se mudou para completo, garante que show_header está ativo
            if !app.header_compact {
                app.show_header = true;
            }
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
            let raw = app.input.trim().to_string();
            if raw.is_empty() {
                return TuiAction::None;
            }
            // Histórico guarda o input cru (com placeholder) — ↑ devolve a
            // versão compacta exatamente como o usuário a viu.
            app.push_history(raw.clone());
            // O runtime e o chat recebem a versão expandida (com o conteúdo
            // real dos pastes). Slash commands e /exit ficam pré-expansão
            // porque nunca contêm placeholders.
            let text = expand_paste_placeholders(&raw, &app.pasted_contents);

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

            if matches!(raw.as_str(), "/exit" | "/quit") {
                return TuiAction::Quit;
            }

            // Slash commands that open overlays.
            if raw == "/model" {
                app.clear_input();
                app.open_model_picker();
                return TuiAction::None;
            }
            if raw == "/permissions" {
                app.clear_input();
                app.open_permission_picker();
                return TuiAction::None;
            }
            if raw == "/session" {
                app.clear_input();
                app.open_session_picker();
                return TuiAction::None;
            }

            app.clear_input();
            app.push_chat(ChatEntry::UserMessage(text.clone()));
            app.scroll_to_bottom();

            app.ultrathink_active = crate::ultrathink::message_contains_ultrathink_keyword(&text);

            // Detect other slash commands.
            if raw.starts_with('/') {
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
        (KeyModifiers::NONE, KeyCode::Backspace) | (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
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

#[allow(clippy::too_many_lines)]
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
                (KeyModifiers::NONE, KeyCode::Char('y') | KeyCode::Enter) => {
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
                    app.overlay = Some(OverlayKind::ModelPicker {
                        items,
                        filter,
                        selected,
                    });
                }
                (KeyModifiers::NONE, KeyCode::Down) => {
                    let filtered = UiApp::filter_model_items(&items, &filter);
                    selected = (selected + 1).min(filtered.len().saturating_sub(1));
                    app.overlay = Some(OverlayKind::ModelPicker {
                        items,
                        filter,
                        selected,
                    });
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    let filtered = UiApp::filter_model_items(&items, &filter);
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
                    app.overlay = Some(OverlayKind::ModelPicker {
                        items,
                        filter,
                        selected,
                    });
                }
                (_, KeyCode::Char(c)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    filter.push(c);
                    selected = 0;
                    app.overlay = Some(OverlayKind::ModelPicker {
                        items,
                        filter,
                        selected,
                    });
                }
                _ => {
                    app.overlay = Some(OverlayKind::ModelPicker {
                        items,
                        filter,
                        selected,
                    });
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
            mut category_selected,
            mut selected,
            mut focus,
        }) => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Up) => {
                    let columns = build_palette_columns(&items, &filter);
                    match focus {
                        SlashPaletteFocus::Categories => {
                            category_selected = category_selected.saturating_sub(1);
                            selected = 0;
                        }
                        SlashPaletteFocus::Commands => {
                            let cmd_len = columns
                                .get(category_selected.min(columns.len().saturating_sub(1)))
                                .map_or(0, |(_, cmds)| cmds.len());
                            selected = selected.saturating_sub(1).min(cmd_len.saturating_sub(1));
                        }
                    }
                    app.overlay = Some(OverlayKind::SlashPalette {
                        items,
                        filter,
                        category_selected,
                        selected,
                        focus,
                    });
                }
                (KeyModifiers::NONE, KeyCode::Down) => {
                    let columns = build_palette_columns(&items, &filter);
                    match focus {
                        SlashPaletteFocus::Categories => {
                            category_selected =
                                (category_selected + 1).min(columns.len().saturating_sub(1));
                            selected = 0;
                        }
                        SlashPaletteFocus::Commands => {
                            let cmd_len = columns
                                .get(category_selected.min(columns.len().saturating_sub(1)))
                                .map_or(0, |(_, cmds)| cmds.len());
                            selected = (selected + 1).min(cmd_len.saturating_sub(1));
                        }
                    }
                    app.overlay = Some(OverlayKind::SlashPalette {
                        items,
                        filter,
                        category_selected,
                        selected,
                        focus,
                    });
                }
                (KeyModifiers::NONE, KeyCode::Left) => {
                    focus = SlashPaletteFocus::Categories;
                    app.overlay = Some(OverlayKind::SlashPalette {
                        items,
                        filter,
                        category_selected,
                        selected,
                        focus,
                    });
                }
                (KeyModifiers::NONE, KeyCode::Right) => {
                    focus = SlashPaletteFocus::Commands;
                    app.overlay = Some(OverlayKind::SlashPalette {
                        items,
                        filter,
                        category_selected,
                        selected,
                        focus,
                    });
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    let columns = build_palette_columns(&items, &filter);
                    if matches!(focus, SlashPaletteFocus::Categories) {
                        focus = SlashPaletteFocus::Commands;
                        selected = 0;
                        app.overlay = Some(OverlayKind::SlashPalette {
                            items,
                            filter,
                            category_selected,
                            selected,
                            focus,
                        });
                        return TuiAction::None;
                    }
                    if let Some((_, cmds)) =
                        columns.get(category_selected.min(columns.len().saturating_sub(1)))
                    {
                        if let Some((cmd, _)) = cmds.get(selected.min(cmds.len().saturating_sub(1)))
                        {
                            app.overlay = None;
                            app.clear_input();
                            return TuiAction::SlashCommand(format!("/{cmd}"));
                        }
                    }
                    // Se o filtro contém espaço, tenta "/<cmd> <arg>" — suporte a argumentos inline.
                    let stripped = filter.trim_start_matches('/');
                    if let Some((cmd_name, arg)) = stripped.split_once(' ') {
                        let arg = arg.trim();
                        if !arg.is_empty() {
                            let cmd_rows = build_palette_rows(&items, cmd_name);
                            let exact = cmd_rows.iter().find_map(|r| {
                                if let PaletteRow::Command { cmd, .. } = r {
                                    if cmd.as_str() == cmd_name {
                                        Some(cmd.clone())
                                    } else {
                                        None
                                    }
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
                    app.overlay = Some(OverlayKind::SlashPalette {
                        items,
                        filter,
                        category_selected,
                        selected,
                        focus,
                    });
                }
                (KeyModifiers::NONE, KeyCode::Esc) => {
                    app.overlay = None;
                    app.clear_input();
                }
                (KeyModifiers::NONE, KeyCode::Backspace) => {
                    filter.pop();
                    let _columns = build_palette_columns(&items, &filter);
                    category_selected = 0;
                    selected = 0;
                    app.overlay = Some(OverlayKind::SlashPalette {
                        items,
                        filter,
                        category_selected,
                        selected,
                        focus,
                    });
                }
                (_, KeyCode::Char(c)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    filter.push(c);
                    let _columns = build_palette_columns(&items, &filter);
                    category_selected = 0;
                    selected = 0;
                    app.overlay = Some(OverlayKind::SlashPalette {
                        items,
                        filter,
                        category_selected,
                        selected,
                        focus,
                    });
                }
                _ => {
                    app.overlay = Some(OverlayKind::SlashPalette {
                        items,
                        filter,
                        category_selected,
                        selected,
                        focus,
                    });
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
                        let path_s = path.clone();
                        let insert_pos = anchor_pos + 1; // após o `@`
                        let byte_idx = app
                            .input
                            .char_indices()
                            .nth(insert_pos)
                            .map_or(app.input.len(), |(i, _)| i);
                        let chars_inserted = path_s.chars().count() + 1; // +1 do espaço
                        app.input.insert_str(byte_idx, &format!("{path_s} "));
                        app.cursor_col = insert_pos + chars_inserted;
                    }
                    app.overlay = None;
                }
                (KeyModifiers::NONE, KeyCode::Backspace) => {
                    if filter.is_empty() {
                        // Remove o @ e fecha overlay
                        let byte_idx = app.input.char_indices().nth(anchor_pos).map(|(i, _)| i);
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
                    if let Some((session_id, _, _)) = items.get(selected) {
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

        Some(OverlayKind::ScriptPicker {
            items,
            mut selected,
        }) => {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Esc) => {
                    app.overlay = None;
                }
                (KeyModifiers::NONE, KeyCode::Up) => {
                    selected = selected.saturating_sub(1);
                    app.overlay = Some(OverlayKind::ScriptPicker { items, selected });
                }
                (KeyModifiers::NONE, KeyCode::Down) => {
                    selected = (selected + 1).min(items.len().saturating_sub(1));
                    app.overlay = Some(OverlayKind::ScriptPicker { items, selected });
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    if let Some((_name, path)) = items.get(selected) {
                        let p = path.clone();
                        app.overlay = None;
                        return TuiAction::RunScript(p);
                    }
                    app.overlay = None;
                }
                _ => {
                    app.overlay = Some(OverlayKind::ScriptPicker { items, selected });
                }
            }
            TuiAction::None
        }

        Some(OverlayKind::UninstallConfirm) => {
            if let (KeyModifiers::NONE, KeyCode::Enter) = (key.modifiers, key.code) {
                app.overlay = None;
                return TuiAction::Uninstall;
            }
            app.overlay = None;
            app.push_chat(ChatEntry::SystemNote("Desinstalação cancelada.".into()));
            TuiAction::None
        }

        Some(OverlayKind::SwdConfirmApply {
            action_count: _,
            reply_tx,
        }) => {
            if let (KeyModifiers::NONE, KeyCode::Char('a') | KeyCode::Enter) =
                (key.modifiers, key.code)
            {
                let _ = reply_tx.send(true);
                app.push_chat(ChatEntry::SystemNote(
                    "✅ SWD: batch aceito — aplicando...".into(),
                ));
            } else {
                let _ = reply_tx.send(false);
                app.push_chat(ChatEntry::SystemNote(
                    "⛔ SWD: batch rejeitado pelo usuário.".into(),
                ));
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
                                    .map_or(new_input.len(), |(i, _)| i);
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
                                    if let Some(model) = setup_wizard_default_model(provider_sel) {
                                        app.model = model;
                                    }
                                    app.overlay = None;
                                    TuiAction::SetupComplete
                                }
                            } else {
                                // step == 2: finished typing key2
                                let new_key2 = input.clone();
                                let _ = save_setup_keys(provider_sel, &key1, &new_key2);
                                if let Some(model) = setup_wizard_default_model(provider_sel) {
                                    app.model = model;
                                }
                                app.overlay = None;
                                TuiAction::SetupComplete
                            }
                        }
                        (_, KeyCode::Char(c)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let mut new_input = input.clone();
                            let mut new_cursor = cursor;
                            let byte_idx = new_input
                                .char_indices()
                                .nth(new_cursor)
                                .map_or(new_input.len(), |(i, _)| i);
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

        Some(OverlayKind::AuthPicker { step }) => handle_auth_picker_key(app, key, step),

        Some(OverlayKind::FirstRunWizard { step, state }) => {
            handle_first_run_wizard_key(app, key, step, state)
        }

        Some(OverlayKind::DeepResearchKeyInput {
            mut input,
            mut cursor,
        }) => match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Esc) => {
                app.overlay = None;
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let trimmed = input.trim().to_string();
                if trimmed.is_empty() {
                    app.overlay = Some(OverlayKind::DeepResearchKeyInput { input, cursor });
                    TuiAction::None
                } else {
                    app.overlay = None;
                    TuiAction::SlashCommand(format!("/deepresearch {trimmed}"))
                }
            }
            (KeyModifiers::NONE, KeyCode::Backspace)
            | (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
                if cursor > 0 {
                    cursor -= 1;
                    let idx = input
                        .char_indices()
                        .nth(cursor)
                        .map_or(input.len(), |(i, _)| i);
                    let next = input
                        .char_indices()
                        .nth(cursor + 1)
                        .map_or(input.len(), |(i, _)| i);
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
                    .map_or(input.len(), |(i, _)| i);
                input.insert(idx, c);
                cursor += 1;
                app.overlay = Some(OverlayKind::DeepResearchKeyInput { input, cursor });
                TuiAction::None
            }
            _ => {
                app.overlay = Some(OverlayKind::DeepResearchKeyInput { input, cursor });
                TuiAction::None
            }
        },

        None => TuiAction::None,
    }
}

fn detect_importable_auth_sources() -> (bool, bool) {
    (
        runtime::detect_claude_code_credentials().is_some(),
        runtime::detect_codex_credentials().is_some(),
    )
}

fn auth_methods_visible_for_provider(
    group: &ProviderAuthGroup,
    claude_code_detected: bool,
    codex_detected: bool,
) -> Vec<(AuthMethodChoice, &'static str)> {
    let mut methods: Vec<(AuthMethodChoice, &'static str)> = group
        .methods
        .iter()
        .map(|&m| (m, auth_method_label(m)))
        .collect();
    if matches!(group.id, "anthropic") && claude_code_detected {
        methods.insert(
            0,
            (
                AuthMethodChoice::ImportClaudeCode,
                "Importar Claude Code credentials  [detectado]",
            ),
        );
    }
    if matches!(group.id, "openai") && codex_detected {
        methods.insert(
            0,
            (
                AuthMethodChoice::ImportCodex,
                "Importar Codex auth.json  [detectado]",
            ),
        );
    }
    methods
}

fn auth_method_label(method: AuthMethodChoice) -> &'static str {
    match method {
        AuthMethodChoice::ClaudeAiOAuth => "Claude.ai OAuth  (Pro/Max)",
        AuthMethodChoice::ConsoleOAuth => "Console OAuth    (cria API key)",
        AuthMethodChoice::SsoOAuth => "SSO OAuth        (claude.ai + SSO)",
        AuthMethodChoice::CodexOAuth => "Codex/OpenAI OAuth (codex login)",
        AuthMethodChoice::PasteApiKey => "Colar API key    (sk-ant-...)",
        AuthMethodChoice::PasteAuthToken => "Colar Auth Token (Bearer)",
        AuthMethodChoice::PasteOpenAiKey => "Colar OpenAI key (sk-...)",
        AuthMethodChoice::PasteOpenCodeGoKey => "Colar OpenCode Go API key",
        AuthMethodChoice::PasteXaiKey => "Colar xAI (Grok) API key",
        AuthMethodChoice::UseBedrock => "AWS Bedrock",
        AuthMethodChoice::UseVertex => "Google Vertex AI",
        AuthMethodChoice::UseFoundry => "Azure Foundry",
        AuthMethodChoice::ImportClaudeCode => "Importar Claude Code credentials",
        AuthMethodChoice::ImportCodex => "Importar Codex auth.json",
        AuthMethodChoice::LegacyElai => "Elai OAuth legacy (elai.dev)",
    }
}

fn provider_for_method(method: AuthMethodChoice) -> Option<ProviderAuthGroup> {
    provider_auth_groups()
        .into_iter()
        .find(|g| g.methods.contains(&method))
}

fn default_model_for_auth_method(method: AuthMethodChoice) -> Option<String> {
    let env_or = |key: &str, fallback: &str| {
        std::env::var(key)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| fallback.to_string())
    };
    match method {
        AuthMethodChoice::PasteApiKey
        | AuthMethodChoice::PasteAuthToken
        | AuthMethodChoice::ClaudeAiOAuth
        | AuthMethodChoice::ConsoleOAuth
        | AuthMethodChoice::SsoOAuth
        | AuthMethodChoice::ImportClaudeCode
        | AuthMethodChoice::UseBedrock
        | AuthMethodChoice::UseVertex
        | AuthMethodChoice::UseFoundry => Some(env_or(
            "ELAI_DEFAULT_ANTHROPIC_MODEL",
            "claude-haiku-4-5-20251001",
        )),
        AuthMethodChoice::PasteOpenAiKey
        | AuthMethodChoice::CodexOAuth
        | AuthMethodChoice::ImportCodex => Some(env_or("ELAI_DEFAULT_OPENAI_MODEL", "gpt-5.5")),
        AuthMethodChoice::PasteOpenCodeGoKey => {
            Some(env_or("ELAI_DEFAULT_OPENCODE_GO_MODEL", "kimi-k2.6"))
        }
        AuthMethodChoice::PasteXaiKey => Some(env_or("ELAI_DEFAULT_XAI_MODEL", "grok-4.1.1")),
        AuthMethodChoice::LegacyElai => None,
    }
}

fn handle_provider_list_key(app: &mut UiApp, key: KeyEvent, selected: usize) -> TuiAction {
    let groups = provider_auth_groups();
    let count = groups.len();
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Esc) => {
            app.overlay = None;
            TuiAction::None
        }
        (KeyModifiers::NONE, KeyCode::Up) => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::ProviderList {
                    selected: selected.saturating_sub(1),
                },
            });
            TuiAction::None
        }
        (KeyModifiers::NONE, KeyCode::Down) => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::ProviderList {
                    selected: (selected + 1).min(count.saturating_sub(1)),
                },
            });
            TuiAction::None
        }
        (KeyModifiers::NONE, KeyCode::Enter) => {
            let Some(group) = groups.get(selected).cloned() else {
                app.overlay = None;
                return TuiAction::None;
            };
            if is_provider_connected(&group) {
                app.overlay = Some(OverlayKind::AuthPicker {
                    step: AuthStep::ConnectedModels {
                        provider: group,
                        selected: 0,
                    },
                });
            } else {
                let (claude_detected, codex_detected) = detect_importable_auth_sources();
                app.overlay = Some(OverlayKind::AuthPicker {
                    step: AuthStep::MethodList {
                        provider: group,
                        selected: 0,
                        claude_code_detected: claude_detected,
                        codex_detected,
                    },
                });
            }
            TuiAction::None
        }
        _ => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::ProviderList { selected },
            });
            TuiAction::None
        }
    }
}

fn start_anthropic_oauth_flow(app: &mut UiApp, method: AuthMethodChoice) {
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

fn handle_codex_oauth(app: &mut UiApp) {
    match crate::auth::login_codex_oauth(false) {
        Ok(()) => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::Done {
                    label: "Codex/OpenAI OAuth concluido".to_string(),
                    model: default_model_for_auth_method(AuthMethodChoice::CodexOAuth),
                },
            });
        }
        Err(e) => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::Failed {
                    error: e.to_string(),
                },
            });
        }
    }
}

fn handle_import_claude_code(app: &mut UiApp) {
    match runtime::import_claude_code_credentials() {
        Ok(Some(_)) => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::Done {
                    label: "Imported Claude Code credentials".to_string(),
                    model: default_model_for_auth_method(AuthMethodChoice::ImportClaudeCode),
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

fn handle_import_codex(app: &mut UiApp) {
    match runtime::import_codex_credentials() {
        Ok(Some(_)) => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::Done {
                    label: "Imported Codex auth.json credentials".to_string(),
                    model: default_model_for_auth_method(AuthMethodChoice::ImportCodex),
                },
            });
        }
        Ok(None) => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::Failed {
                    error: "No Codex auth.json found (set cli_auth_credentials_store=\"file\" and run codex login)".to_string(),
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

fn handle_method_list_enter(app: &mut UiApp, method: AuthMethodChoice) {
    match method {
        AuthMethodChoice::ClaudeAiOAuth
        | AuthMethodChoice::ConsoleOAuth
        | AuthMethodChoice::SsoOAuth => start_anthropic_oauth_flow(app, method),
        AuthMethodChoice::CodexOAuth => handle_codex_oauth(app),
        AuthMethodChoice::PasteApiKey
        | AuthMethodChoice::PasteAuthToken
        | AuthMethodChoice::PasteOpenAiKey
        | AuthMethodChoice::PasteOpenCodeGoKey
        | AuthMethodChoice::PasteXaiKey => {
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
        AuthMethodChoice::ImportClaudeCode => handle_import_claude_code(app),
        AuthMethodChoice::ImportCodex => handle_import_codex(app),
        AuthMethodChoice::LegacyElai => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::Done {
                    label: "Use `elai login --legacy-elai` no terminal".to_string(),
                    model: None,
                },
            });
        }
    }
}

fn handle_method_list_key(
    app: &mut UiApp,
    key: KeyEvent,
    selected: usize,
    provider: ProviderAuthGroup,
    claude_code_detected: bool,
    codex_detected: bool,
) -> TuiAction {
    let methods =
        auth_methods_visible_for_provider(&provider, claude_code_detected, codex_detected);
    let count = methods.len();
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Esc) => {
            // Go back to provider list instead of closing.
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::ProviderList { selected: 0 },
            });
            TuiAction::None
        }
        (KeyModifiers::NONE, KeyCode::Up) => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::MethodList {
                    provider,
                    selected: selected.saturating_sub(1),
                    claude_code_detected,
                    codex_detected,
                },
            });
            TuiAction::None
        }
        (KeyModifiers::NONE, KeyCode::Down) => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::MethodList {
                    provider,
                    selected: (selected + 1).min(count.saturating_sub(1)),
                    claude_code_detected,
                    codex_detected,
                },
            });
            TuiAction::None
        }
        (KeyModifiers::NONE, KeyCode::Enter) => {
            let Some((method, _)) = methods.get(selected).copied() else {
                app.overlay = None;
                return TuiAction::None;
            };
            handle_method_list_enter(app, method);
            TuiAction::None
        }
        _ => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::MethodList {
                    provider,
                    selected,
                    claude_code_detected,
                    codex_detected,
                },
            });
            TuiAction::None
        }
    }
}

fn handle_email_input_key(
    app: &mut UiApp,
    key: KeyEvent,
    method: AuthMethodChoice,
    mut input: String,
    mut cursor: usize,
) -> TuiAction {
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Esc) => {
            back_to_provider_or_method_list(app, method);
        }
        (KeyModifiers::NONE, KeyCode::Backspace)
        | (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
            if cursor > 0 {
                cursor -= 1;
                let idx = input
                    .char_indices()
                    .nth(cursor)
                    .map_or(input.len(), |(i, _)| i);
                input.remove(idx);
            }
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::EmailInput {
                    method,
                    input,
                    cursor,
                },
            });
        }
        (KeyModifiers::NONE, KeyCode::Enter) => {
            let email = if input.trim().is_empty() {
                None
            } else {
                Some(input.clone())
            };
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
            let idx = input
                .char_indices()
                .nth(cursor)
                .map_or(input.len(), |(i, _)| i);
            input.insert(idx, c);
            cursor += 1;
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::EmailInput {
                    method,
                    input,
                    cursor,
                },
            });
        }
        _ => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::EmailInput {
                    method,
                    input,
                    cursor,
                },
            });
        }
    }
    TuiAction::None
}

fn paste_secret_save(method: AuthMethodChoice, input: &str) -> Result<(), crate::auth::AuthError> {
    match method {
        AuthMethodChoice::PasteApiKey => crate::auth::save_pasted_api_key(input),
        AuthMethodChoice::PasteAuthToken => crate::auth::save_pasted_auth_token(input),
        AuthMethodChoice::PasteOpenAiKey => crate::auth::save_pasted_openai_key(input),
        AuthMethodChoice::PasteOpenCodeGoKey => crate::auth::save_pasted_opencode_go_key(input),
        AuthMethodChoice::PasteXaiKey => crate::auth::save_pasted_xai_key(input),
        _ => Err(crate::auth::AuthError::InvalidInput(
            "unexpected method".into(),
        )),
    }
}

fn paste_secret_label(method: AuthMethodChoice) -> String {
    match method {
        AuthMethodChoice::PasteApiKey => "API key salva".to_string(),
        AuthMethodChoice::PasteOpenAiKey => "OpenAI key salva".to_string(),
        AuthMethodChoice::PasteOpenCodeGoKey => "OpenCode Go key salva".to_string(),
        AuthMethodChoice::PasteXaiKey => "xAI key salva".to_string(),
        _ => "Auth token salvo".to_string(),
    }
}

fn paste_secret_finish(app: &mut UiApp, method: AuthMethodChoice, input: &str) {
    match paste_secret_save(method, input) {
        Ok(()) => {
            if let Some(provider) = provider_for_method(method) {
                let models = provider_models(&provider);
                let default_model = default_model_for_auth_method(method);
                let selected = default_model
                    .as_ref()
                    .and_then(|m| models.iter().position(|id| id == m))
                    .unwrap_or(0);
                app.overlay = Some(OverlayKind::AuthPicker {
                    step: AuthStep::ConnectedModels { provider, selected },
                });
            } else {
                let label = paste_secret_label(method);
                app.overlay = Some(OverlayKind::AuthPicker {
                    step: AuthStep::Done {
                        label,
                        model: default_model_for_auth_method(method),
                    },
                });
            }
        }
        Err(e) => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::Failed {
                    error: e.to_string(),
                },
            });
        }
    }
}

fn back_to_provider_or_method_list(app: &mut UiApp, method: AuthMethodChoice) {
    if let Some(provider) = provider_for_method(method) {
        let (claude_detected, codex_detected) = detect_importable_auth_sources();
        app.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::MethodList {
                provider,
                selected: 0,
                claude_code_detected: claude_detected,
                codex_detected,
            },
        });
    } else {
        app.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::ProviderList { selected: 0 },
        });
    }
}

fn handle_paste_secret_key(
    app: &mut UiApp,
    key: KeyEvent,
    method: AuthMethodChoice,
    mut input: String,
    mut cursor: usize,
    masked: bool,
) -> TuiAction {
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Esc) => {
            back_to_provider_or_method_list(app, method);
        }
        (KeyModifiers::NONE, KeyCode::Backspace)
        | (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
            if cursor > 0 {
                cursor -= 1;
                let idx = input
                    .char_indices()
                    .nth(cursor)
                    .map_or(input.len(), |(i, _)| i);
                input.remove(idx);
            }
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::PasteSecret {
                    method,
                    input,
                    cursor,
                    masked,
                },
            });
        }
        (KeyModifiers::NONE, KeyCode::Enter) => {
            paste_secret_finish(app, method, &input);
        }
        (_, KeyCode::Char(c)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let idx = input
                .char_indices()
                .nth(cursor)
                .map_or(input.len(), |(i, _)| i);
            input.insert(idx, c);
            cursor += 1;
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::PasteSecret {
                    method,
                    input,
                    cursor,
                    masked,
                },
            });
        }
        _ => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::PasteSecret {
                    method,
                    input,
                    cursor,
                    masked,
                },
            });
        }
    }
    TuiAction::None
}

fn handle_browser_flow_key(app: &mut UiApp, key: KeyEvent, step_data: AuthStep) -> TuiAction {
    let AuthStep::BrowserFlow {
        method,
        url,
        port,
        started_at,
        rx,
        cancel_flag,
    } = step_data
    else {
        return TuiAction::None;
    };
    if let (KeyModifiers::NONE, KeyCode::Esc) = (key.modifiers, key.code) {
        cancel_flag.store(true, Ordering::Relaxed);
        back_to_provider_or_method_list(app, method);
    } else {
        // Drain events from channel while keeping step alive.
        let mut next_step = AuthStep::BrowserFlow {
            method,
            url,
            port,
            started_at,
            rx,
            cancel_flag,
        };
        if let AuthStep::BrowserFlow { ref rx, .. } = next_step {
            if let Ok(event) = rx.try_recv() {
                next_step = match event {
                    AuthEvent::Success(label) => AuthStep::Done {
                        label,
                        model: default_model_for_auth_method(method),
                    },
                    AuthEvent::Error(msg) => AuthStep::Failed { error: msg },
                    AuthEvent::Progress(_) => next_step,
                };
            }
        }
        // Reconstruct if still BrowserFlow (workaround for partial move).
        app.overlay = Some(OverlayKind::AuthPicker { step: next_step });
    }
    TuiAction::None
}

fn handle_confirm3p_key(
    app: &mut UiApp,
    key: KeyEvent,
    method: AuthMethodChoice,
    env_var: &'static str,
) -> TuiAction {
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Char('y' | 'Y') | KeyCode::Enter) => {
            match crate::auth::save_3p_named(env_var) {
                Ok(()) => {
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::Done {
                            label: format!(
                                "{method:?} salvo. Adicione `export {env_var}=1` ao seu shell RC."
                            ),
                            model: default_model_for_auth_method(method),
                        },
                    });
                }
                Err(e) => {
                    app.overlay = Some(OverlayKind::AuthPicker {
                        step: AuthStep::Failed {
                            error: e.to_string(),
                        },
                    });
                }
            }
        }
        _ => {
            back_to_provider_or_method_list(app, method);
        }
    }
    TuiAction::None
}

fn handle_done_key(app: &mut UiApp, key: KeyEvent, label: String, model: Option<String>) -> TuiAction {
    if let (KeyModifiers::NONE, KeyCode::Esc | KeyCode::Enter) = (key.modifiers, key.code) {
        app.overlay = None;
        TuiAction::AuthComplete { label, model }
    } else {
        app.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::Done { label, model },
        });
        TuiAction::None
    }
}

fn handle_failed_key(app: &mut UiApp, key: KeyEvent, error: String) -> TuiAction {
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Esc | KeyCode::Enter) => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::ProviderList { selected: 0 },
            });
        }
        _ => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::Failed { error },
            });
        }
    }
    TuiAction::None
}

fn handle_connected_models_key(
    app: &mut UiApp,
    key: KeyEvent,
    provider: ProviderAuthGroup,
    selected: usize,
) -> TuiAction {
    let models = provider_models(&provider);
    let count = models.len();
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Esc) => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::ProviderList { selected: 0 },
            });
        }
        (KeyModifiers::NONE, KeyCode::Up) => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::ConnectedModels {
                    provider,
                    selected: selected.saturating_sub(1),
                },
            });
        }
        (KeyModifiers::NONE, KeyCode::Down) => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::ConnectedModels {
                    provider,
                    selected: (selected + 1).min(count.saturating_sub(1)),
                },
            });
        }
        (KeyModifiers::NONE, KeyCode::Enter) => {
            if let Some(model) = models.get(selected) {
                let label = format!("Model set to: {model}");
                app.overlay = None;
                return TuiAction::AuthComplete {
                    label,
                    model: Some(model.clone()),
                };
            }
        }
        _ => {
            app.overlay = Some(OverlayKind::AuthPicker {
                step: AuthStep::ConnectedModels { provider, selected },
            });
        }
    }
    TuiAction::None
}

fn handle_auth_picker_key(app: &mut UiApp, key: KeyEvent, step: AuthStep) -> TuiAction {
    match step {
        AuthStep::ProviderList { selected } => handle_provider_list_key(app, key, selected),
        AuthStep::MethodList {
            selected,
            provider,
            claude_code_detected,
            codex_detected,
        } => handle_method_list_key(
            app,
            key,
            selected,
            provider,
            claude_code_detected,
            codex_detected,
        ),
        AuthStep::EmailInput {
            method,
            input,
            cursor,
        } => handle_email_input_key(app, key, method, input, cursor),
        AuthStep::PasteSecret {
            method,
            input,
            cursor,
            masked,
        } => handle_paste_secret_key(app, key, method, input, cursor, masked),
        step_data @ AuthStep::BrowserFlow { .. } => handle_browser_flow_key(app, key, step_data),
        AuthStep::Confirm3p { method, env_var } => handle_confirm3p_key(app, key, method, env_var),
        AuthStep::Done { label, model } => handle_done_key(app, key, label, model),
        AuthStep::Failed { error } => handle_failed_key(app, key, error),
        AuthStep::ConnectedModels { provider, selected } => {
            handle_connected_models_key(app, key, provider, selected)
        }
    }
}

/// Drain `AuthEvents` from a `BrowserFlow` channel and advance the overlay step if needed.
/// Call this from the main tick loop so the UI updates without requiring a keypress.
pub fn drain_auth_events(app: &mut UiApp) {
    // We need to take the overlay, drain, and put it back to avoid borrow conflicts.
    let overlay = app.overlay.take();
    if let Some(OverlayKind::AuthPicker { step }) = overlay {
        let next_step = match step {
            AuthStep::BrowserFlow {
                method,
                url,
                port,
                started_at,
                rx,
                cancel_flag,
            } => match rx.try_recv() {
                Ok(AuthEvent::Success(label)) => AuthStep::Done {
                    label,
                    model: default_model_for_auth_method(method),
                },
                Ok(AuthEvent::Error(msg)) => AuthStep::Failed { error: msg },
                Ok(AuthEvent::Progress(_)) | Err(_) => AuthStep::BrowserFlow {
                    method,
                    url,
                    port,
                    started_at,
                    rx,
                    cancel_flag,
                },
            },
            other => other,
        };
        app.overlay = Some(OverlayKind::AuthPicker { step: next_step });
    } else {
        app.overlay = overlay;
    }
}

const ANTHROPIC_MODELS: &[&str] = &[
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-sonnet-4-6",
    "claude-haiku-4-5-20251001",
];

// Keep this list aligned with Codex-supported model IDs.
const OPENAI_CODEX_MODELS: &[&str] = &[
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.3-codex",
    "gpt-5.2",
    "gpt-5",
    "gpt-5-mini",
    "gpt-5-nano",
];

const OPENCODE_GO_MODELS: &[&str] = &[
    "kimi-k2.6",
    "kimi-k2.5",
    "glm-5.1",
    "glm-5",
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "qwen3.6-plus",
    "qwen3.5-plus",
    "mimo-v2-pro",
    "mimo-v2-omni",
    "mimo-v2.5-pro",
    "mimo-v2.5",
    "minimax-m2.5",
    "minimax-m2.7",
];

const OPENCODE_ZEN_FREE_MODELS: &[&str] = &[
    // destaque / flagship free
    "big-pickle",
    // minimax
    "minimax-m2.1",
    "minimax-m2.1-free",
    "minimax-m2.5-free",
    // mimo (xiaomi)
    "mimo-v2-flash-free",
    "mimo-v2-omni-free",
    "mimo-v2-pro-free",
    // kimi / moonshot
    "kimi-k2",
    "kimi-k2-thinking",
    "kimi-k2.5-free",
    // glm (zhipu)
    "glm-5-free",
    "glm-4.7-free",
    // qwen (alibaba)
    "qwen3.6-plus-free",
    // arcee / outros
    "trinity-large-preview-free",
    "nemotron-3-super-free",
    "hy3-preview-free",
    "ling-2.6-flash-free",
];

const XAI_MODELS: &[&str] = &["grok-4.1.1", "grok-4.1", "grok-4"];

fn provider_models(group: &ProviderAuthGroup) -> Vec<String> {
    match group.id {
        "openai" => OPENAI_CODEX_MODELS.iter().map(|s| (*s).to_string()).collect(),
        "opencode-go" => OPENCODE_GO_MODELS.iter().map(|s| (*s).to_string()).collect(),
        "opencode-zen" => OPENCODE_ZEN_FREE_MODELS.iter().map(|s| (*s).to_string()).collect(),
        "xai" => XAI_MODELS.iter().map(|s| (*s).to_string()).collect(),
        "anthropic" | "bedrock" | "vertex" | "foundry" => anthropic_model_items(),
        _ => vec![],
    }
}

const OPENAI_API_MODELS: &[&str] = &["gpt-4o", "gpt-4o-mini", "gpt-4.5", "o1", "o3", "o4-mini"];

/// Anthropic model list with dynamic ant overrides prepended (alias — label format).
fn anthropic_model_items() -> Vec<String> {
    let mut items: Vec<String> = api::get_ant_models()
        .into_iter()
        .map(|m| format!("{} — {}", m.alias, m.label))
        .collect();
    items.extend(models_for_provider(WizardProvider::Anthropic));
    items
}

fn models_for_provider(provider: WizardProvider) -> Vec<String> {
    let ids: &[&str] = match provider {
        WizardProvider::Anthropic => ANTHROPIC_MODELS,
        WizardProvider::OpenAi(OpenAiChannel::Codex) => OPENAI_CODEX_MODELS,
        WizardProvider::OpenAi(OpenAiChannel::ApiKey) => OPENAI_API_MODELS,
        WizardProvider::Xai => XAI_MODELS,
        WizardProvider::OpenCodeGo => OPENCODE_GO_MODELS,
    };
    ids.iter().map(|s| (*s).to_string()).collect()
}

fn provider_label(provider: WizardProvider) -> &'static str {
    match provider {
        WizardProvider::Anthropic => "Anthropic",
        WizardProvider::OpenAi(OpenAiChannel::Codex) => "OpenAI (ChatGPT/Codex)",
        WizardProvider::OpenAi(OpenAiChannel::ApiKey) => "OpenAI (API key)",
        WizardProvider::Xai => "xAI (Grok)",
        WizardProvider::OpenCodeGo => "OpenCode Go",
    }
}

const WIZARD_PERMS: &[&str] = &["read-only", "workspace-write", "danger-full-access"];

fn setup_wizard_default_model(provider_sel: usize) -> Option<String> {
    match provider_sel {
        // Anthropic
        0 => Some(
            std::env::var("ELAI_DEFAULT_ANTHROPIC_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "claude-haiku-4-5-20251001".to_string()),
        ),
        // OpenAI
        1 => Some(
            std::env::var("ELAI_DEFAULT_OPENAI_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "gpt-4o-mini".to_string()),
        ),
        // Both: keep current model unchanged.
        _ => None,
    }
}

#[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
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
                    step: WizardStep::Provider { selected: 0 },
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

        WizardStep::Provider { selected } => match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Esc) => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Welcome,
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Up) => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Provider {
                        selected: selected.saturating_sub(1),
                    },
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                let next = (selected + 1).min(WIZARD_PROVIDER_OPTIONS.len().saturating_sub(1));
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Provider { selected: next },
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let provider = WIZARD_PROVIDER_OPTIONS
                    .get(selected)
                    .copied()
                    .unwrap_or(WizardProvider::Anthropic);
                let mut new_state = state;
                new_state.provider = provider;
                let models = models_for_provider(provider);
                if let Some(default_model) = models.first() {
                    new_state.model.clone_from(default_model);
                }
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Model {
                        provider,
                        selected: 0,
                    },
                    state: new_state,
                });
                TuiAction::None
            }
            _ => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Provider { selected },
                    state,
                });
                TuiAction::None
            }
        },

        WizardStep::Model { provider, selected } => match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Esc) => {
                let provider_selected = WIZARD_PROVIDER_OPTIONS
                    .iter()
                    .position(|p| *p == provider)
                    .unwrap_or(0);
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Provider {
                        selected: provider_selected,
                    },
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Up) => {
                let next = selected.saturating_sub(1);
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Model {
                        provider,
                        selected: next,
                    },
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                let models = models_for_provider(provider);
                let next = (selected + 1).min(models.len().saturating_sub(1));
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Model {
                        provider,
                        selected: next,
                    },
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                let model = models_for_provider(provider)
                    .get(selected)
                    .cloned()
                    .unwrap_or_else(|| "claude-opus-4-6".to_string())
                    .clone();
                let new_state = WizardState { model, ..state };
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Permissions { selected: 0 },
                    state: new_state,
                });
                TuiAction::None
            }
            _ => {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Model { provider, selected },
                    state,
                });
                TuiAction::None
            }
        },

        WizardStep::Permissions { selected } => match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Esc) => {
                let provider = state.provider;
                let selected_model = models_for_provider(provider)
                    .iter()
                    .position(|m| m == &state.model)
                    .unwrap_or(0);
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Model {
                        provider,
                        selected: selected_model,
                    },
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
            (KeyModifiers::NONE, KeyCode::Tab | KeyCode::Down) => {
                let next = (focused + 1) % 3;
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Defaults { focused: next },
                    state,
                });
                TuiAction::None
            }
            (KeyModifiers::SHIFT, KeyCode::BackTab) | (KeyModifiers::NONE, KeyCode::Up) => {
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

        WizardStep::Done => {
            if let (KeyModifiers::NONE, KeyCode::Enter | KeyCode::Esc) = (key.modifiers, key.code) {
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
                app.model.clone_from(&state.model);
                app.permission_mode.clone_from(&state.permission_mode);
                app.overlay = None;
                TuiAction::SetupComplete
            } else {
                app.overlay = Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Done,
                    state,
                });
                TuiAction::None
            }
        }
    }
}

#[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
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
            return (String::new(), port, rx, cancel);
        }
    };
    let state = match runtime::generate_state() {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(AuthEvent::Error(format!("state: {e}")));
            return (String::new(), port, rx, cancel);
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
        use std::io::{Read, Write};
        let _ = tx.send(AuthEvent::Progress("Opening browser...".into()));
        let _ = crate::auth::open_browser(&url_for_thread);
        let _ = tx.send(AuthEvent::Progress(format!(
            "Waiting for callback on port {port}..."
        )));

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
        let Some(code) = cb.code else {
            let _ = tx.send(AuthEvent::Error("no auth code in callback".into()));
            return;
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

        if method == AuthMethodChoice::ConsoleOAuth {
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
            let _ = tx.send(AuthEvent::Success(
                "Console OAuth concluido — API key salva".to_string(),
            ));
        } else {
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
    });

    (url, port, rx, cancel)
}

/// Carrega lista de paths para o picker. Tenta:
/// 1. `.elai/index/metadata.json` se existe (rápido).
/// 2. Fallback: re-walk do projeto via `crate::verify::walk_project`.
///    Limita a 5000 paths.
fn load_indexed_paths(cwd: &std::path::Path) -> Vec<String> {
    const MAX_PATHS: usize = 5000;
    let metadata_path = cwd.join(".elai").join("index").join("metadata.json");
    if metadata_path.is_file() {
        if let Ok(s) = std::fs::read_to_string(&metadata_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                if let Some(arr) = v.get("indexed_paths").and_then(|x| x.as_array()) {
                    let paths: Vec<String> = arr
                        .iter()
                        .filter_map(|x| x.as_str().map(std::string::ToString::to_string))
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
            f.is_empty() || cmd.to_lowercase().contains(&f) || desc.to_lowercase().contains(&f)
        })
        .collect()
}

// ─── Rendering ────────────────────────────────────────────────────────────────

pub fn render(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut UiApp,
) -> io::Result<()> {
    if app.force_clear {
        terminal.clear()?;
        app.force_clear = false;
    }
    let completed = terminal.draw(|frame| {
        let size = frame.area();

        // Compute how many rows the input text needs so the box grows with content.
        // avail_w = terminal_width - 2 (block borders) - 2 ("> " prompt prefix), min 1
        let avail_w = (size.width.saturating_sub(4) as usize).max(1);
        let text_rows = count_input_rows(&app.input, avail_w);
        let visible_input_rows = text_rows.min(6_usize); // grow up to 6 rows, then scroll
                                                         // area height = top_border(1) + input_rows + hint(1) + bottom_border(1) = rows + 3
        let input_area_h: u16 = (visible_input_rows + 3).try_into().unwrap_or(u16::MAX);

        // Outer vertical split: header (toggleável), body, margin, status, input.
        // Header ocupa 12 linhas fixas se visível.
        // Quando oculto, o chat ocupa todo o espaço disponível (sem "buraco").

        let total_h = size.height;
        let margin_h = 1_u16;
        let status_h = 1_u16;
        let input_h = input_area_h;
        let header_h: u16 = if app.header_compact { 5 } else { 12 };
        let chat_h = total_h.saturating_sub(header_h + margin_h + status_h + input_h);

        // Calcula Rects manualmente para evitar espaço vazio do ratatui
        let mut y = 0;
        let header_rect = Rect::new(size.x, y, size.width, header_h);
        y += header_h;
        let chat_rect = Rect::new(size.x, y, size.width, chat_h);
        y += chat_h;
        let _margin_rect = Rect::new(size.x, y, size.width, margin_h); // espaço visual
        y += margin_h;
        let status_rect = Rect::new(size.x, y, size.width, status_h);
        y += status_h;
        let input_rect = Rect::new(size.x, y, size.width, input_h);

        // Render header apenas se visível
        if app.show_header {
            draw_header(frame, header_rect, app);
        }
        draw_chat(frame, chat_rect, app);
        draw_status(frame, status_rect, app);
        draw_input(frame, input_rect, app);

        // Draw overlays on top (toast sobre tudo).
        if let Some(ref overlay) = app.overlay {
            draw_overlay(frame, size, overlay, app);
        } else if let Some(ref msg) = app.toast_message {
            // Toast não é um overlay normal (não bloqueia input) — desenhado aqui
            // se nenhum outro overlay está ativo.
            draw_toast(frame, size, msg);
        }

        // Highlight de seleção via mouse-drag (por cima de tudo, exceto overlays
        // modais). Pinta as células do retângulo entre `drag_anchor` e
        // `drag_current` com `Modifier::REVERSED` para feedback visual estilo
        // "marca-texto", ainda que o terminal não tenha SHIFT-bypass.
        if app.overlay.is_none() {
            if let (Some(a), Some(c)) = (app.drag_anchor, app.drag_current) {
                paint_selection(frame.buffer_mut(), a, c);
            }
        }
    })?;
    // Snapshot do buffer recém-renderizado: usado para extrair texto da
    // seleção quando o usuário soltar o botão do mouse. `CompletedFrame.buffer`
    // referencia o buffer "frontal" pós-flush.
    app.last_buffer = Some(completed.buffer.clone());
    Ok(())
}

/// Pinta um retângulo de seleção (estilo seleção de texto) no buffer.
/// `a` e `c` são `(col,row)` das pontas; o retângulo é normalizado.
fn paint_selection(buf: &mut Buffer, a: (u16, u16), c: (u16, u16)) {
    let area = buf.area;
    let (x0, x1) = if a.0 <= c.0 { (a.0, c.0) } else { (c.0, a.0) };
    let (y0, y1) = if a.1 <= c.1 { (a.1, c.1) } else { (c.1, a.1) };
    // Clamp ao buffer.
    let x0 = x0.max(area.x);
    let y0 = y0.max(area.y);
    let x1 = x1.min(area.right().saturating_sub(1));
    let y1 = y1.min(area.bottom().saturating_sub(1));
    if x0 > x1 || y0 > y1 {
        return;
    }
    for y in y0..=y1 {
        for x in x0..=x1 {
            if let Some(cell) = buf.cell_mut(Position { x, y }) {
                cell.modifier.insert(Modifier::REVERSED);
            }
        }
    }
}

/// Extrai texto do snapshot do último buffer renderizado entre dois pontos
/// `(col,row)`. A seleção é "linear" (estilo seleção de texto):
/// - Se a âncora e o cursor estão na mesma linha, retorna o trecho daquela
///   linha entre as colunas.
/// - Caso contrário, da âncora até o fim da linha, linhas inteiras
///   intermediárias, e da primeira coluna até o cursor na última linha.
/// Trailing whitespace de cada linha é removido.
fn extract_buffer_text(app: &UiApp, a: (u16, u16), c: (u16, u16)) -> Option<String> {
    let buf = app.last_buffer.as_ref()?;
    let area = buf.area;
    if area.width == 0 || area.height == 0 {
        return None;
    }
    // Normaliza: garante que `start` vem antes de `end` na ordem de leitura.
    let (start, end) = if (a.1, a.0) <= (c.1, c.0) {
        (a, c)
    } else {
        (c, a)
    };
    let max_x = area.right().saturating_sub(1);
    let max_y = area.bottom().saturating_sub(1);
    let sy = start.1.min(max_y);
    let ey = end.1.min(max_y);
    let mut out = String::new();

    let read_row = |y: u16, x_from: u16, x_to: u16, out: &mut String| {
        let xa = x_from.max(area.x);
        let xb = x_to.min(max_x);
        if xa > xb {
            return;
        }
        let mut line = String::new();
        for x in xa..=xb {
            if let Some(cell) = buf.cell(Position { x, y }) {
                line.push_str(cell.symbol());
            }
        }
        // Remove trailing whitespace para evitar arrastar borda da tela.
        let trimmed = line.trim_end();
        out.push_str(trimmed);
    };

    if sy == ey {
        let xa = start.0.min(end.0);
        let xb = start.0.max(end.0);
        read_row(sy, xa, xb, &mut out);
    } else {
        // Primeira linha: da âncora até o fim.
        read_row(sy, start.0, max_x, &mut out);
        out.push('\n');
        // Linhas intermediárias inteiras.
        for y in (sy + 1)..ey {
            read_row(y, area.x, max_x, &mut out);
            out.push('\n');
        }
        // Última linha: do início até o cursor.
        read_row(ey, area.x, end.0, &mut out);
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

// ── Header ───────────────────────────────────────────────────────────────────

fn draw_header(frame: &mut ratatui::Frame, area: Rect, app: &UiApp) {
    // Modo compacto: mesmo layout do completo, sem ASCII art.
    // 5 linhas: borda + título(1) + welcome(1) + cwd(1) + borda = 5 totais.
    if app.header_compact {
        let t = theme();
        let username = whoami_user();
        let cwd_budget = (area.width as usize).saturating_sub(4);

        let title = Span::styled(
            format!(" Elai Code v{} ", env!("CARGO_PKG_VERSION")),
            Style::default().fg(t.easter_egg.warm).add_modifier(Modifier::BOLD),
        );

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.easter_egg.warm))
            .title(title);

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Mesmo split horizontal do modo completo.
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(52),
                Constraint::Length(1),
                Constraint::Min(20),
            ])
            .split(inner);

        // Coluna esquerda: welcome + cwd (sem ASCII art).
        let welcome_lines = vec![
            Line::from(Span::styled(
                rust_i18n::t!("tui.header.welcome", username = username),
                Style::default()
                    .fg(t.easter_egg.warm)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                shorten_cwd(cwd_budget),
                Style::default().fg(t.easter_egg.dark),
            )),
        ];
        let welcome_para = Paragraph::new(welcome_lines)
            .block(Block::default())
            .alignment(Alignment::Center);
        frame.render_widget(welcome_para, cols[0]);

        // Divisor.
        let divider_block = Block::default()
            .borders(Borders::LEFT)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.easter_egg.warm));
        frame.render_widget(divider_block, cols[1]);

        // Coluna direita: side panel completo.
        draw_side_panel(frame, cols[2], app);
        return;
    }

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


/// Encurta o caminho atual:
/// 1. Substitui `$HOME` por `~`.
/// 2. Se ainda exceder `max_width`, elide segmentos do meio com `…`,
///    preservando a raiz e o(s) último(s) segmento(s) do caminho.
fn shorten_cwd(max_width: usize) -> String {
    let raw = env::current_dir().map_or_else(|_| "~".to_string(), |p| p.display().to_string());

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

// Mascote + "ELAI" — banner ASCII original do header.
const ELAI_ASCII: &str = "\
██████████████████   ███████╗██╗      █████╗ ██╗\n\
████████▓▓▄▄▓▓▄▄▓▓   ██╔════╝██║     ██╔══██╗██║\n\
████████▓▓██▓▓██▓▓   █████╗  ██║     ███████║██║\n\
████████▓▓▀▀▓▓▀▀▓▓   ██╔══╝  ██║     ██╔══██║██║\n\
██████████████████   ███████╗███████╗██║  ██║██║\n\
";

const ELAI_BLOCK_WIDTH: usize = 48;

fn draw_elai_card(frame: &mut ratatui::Frame, area: Rect, _app: &UiApp) {
    let body_style = Style::default().fg(theme().easter_egg.body);
    let eye_style = Style::default().fg(theme().easter_egg.warm);
    let dot_style = Style::default().fg(theme().easter_egg.dark);
    let dim = Style::default().fg(theme().text_secondary);

    let username = whoami_user();
    let cwd_budget = (area.width as usize).saturating_sub(2);
    let cwd = shorten_cwd(cwd_budget);

    let mut lines: Vec<Line> = vec![Line::from(Span::raw(""))];
    lines.extend(ELAI_ASCII.lines().map(|l| {
        #[derive(Clone, Copy, PartialEq)]
        enum Seg {
            Body,
            Dot,
            Eye,
        }
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
                spans.push(Span::styled(
                    current.clone(),
                    match seg {
                        Seg::Body => body_style,
                        Seg::Dot => dot_style,
                        Seg::Eye => eye_style,
                    },
                ));
                current.clear();
            }
            seg = next;
            current.push(ch);
        }
        if !current.is_empty() {
            spans.push(Span::styled(
                current,
                match seg {
                    Seg::Body => body_style,
                    Seg::Dot => dot_style,
                    Seg::Eye => eye_style,
                },
            ));
        }
        Line::from(spans)
    }));
    lines.push(Line::from(vec![
        Span::raw("         "),
        Span::styled("███", body_style),
        Span::raw("   "),
        Span::styled("███", body_style),
        Span::raw("    "),
        Span::styled("╚══════╝╚══════╝╚═╝  ╚═╝╚═╝", body_style),
        Span::raw(" ".repeat(ELAI_BLOCK_WIDTH.saturating_sub(46))),
    ]));
    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(vec![Span::styled(
        rust_i18n::t!("tui.header.welcome", username = username).to_string(),
        Style::default()
            .fg(theme().easter_egg.warm)
            .add_modifier(Modifier::BOLD),
    )]));
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
    ];
    lines.push(Line::from(Span::styled(
        format!("  {}", rust_i18n::t!("tui.side_panel.run_init")),
        muted,
    )));
    lines.push(Line::from(Span::styled(
        format!("  {}", rust_i18n::t!("tui.side_panel.shortcuts")),
        muted,
    )));
    lines.push(Line::from(Span::styled(
        format!("  {}", rust_i18n::t!("tui.side_panel.slash_palette")),
        muted,
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "  {}",
            if app.read_mode {
                rust_i18n::t!("tui.side_panel.read_mode_hint")
            } else {
                rust_i18n::t!("tui.side_panel.mouse_hint")
            }
        ),
        muted,
    )));
    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            rust_i18n::t!("tui.side_panel.recent_activity_header").to_string(),
            Style::default()
                .fg(theme().primary_accent)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    if app.recent_sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {}", rust_i18n::t!("tui.side_panel.no_recent")),
            muted,
        )));
    } else {
        for (session_id, title, msg_count) in app.recent_sessions.iter().take(3) {
            let display = if let Some(ref t) = title {
                t.clone()
            } else {
                session_id
                    .strip_prefix("session-")
                    .unwrap_or(session_id)
                    .chars()
                    .take(12)
                    .collect::<String>()
            };
            let msgs_label =
                rust_i18n::t!("tui.side_panel.session_msgs", count = msg_count.to_string());
            lines.push(Line::from(Span::styled(
                format!("  • {display} ({msgs_label})"),
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
        .borders(Borders::NONE);


    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = chat_to_lines(app, inner.width as usize);
    // Reserva uma linha "gutter" no fim do histórico para evitar que a última
    // linha útil fique colada (ou visualmente coberta) pela faixa inferior.
    lines.push(Line::from(""));
    let total = lines.len();
    let visible = inner.height as usize;
    let max_scroll = total.saturating_sub(visible);
    let scroll = app.chat_scroll.min(max_scroll);
    // Keep the state normalized to the current viewport/content dimensions.
    app.chat_scroll = scroll;

    let display: Vec<Line> = lines.into_iter().skip(scroll).take(visible).collect();

    // As linhas do chat já vêm pré-quebradas por `chat_to_lines`/`wrap_text`.
    // Evitar wrap aqui mantém `total` e `max_scroll` em sincronia com o render.
    let paragraph = Paragraph::new(display);
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
                    *last = format!("{trimmed}…");
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
fn highlight_code_to_lines(
    code: &str,
    lang: &str,
    border_color: ratatui::style::Color,
) -> Vec<Line<'static>> {
    let (ss, syn_theme) = syntax_resources();
    let syntax = ss
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut h = HighlightLines::new(syntax, syn_theme);
    let mut result = Vec::new();

    for raw_line in LinesWithEndings::from(code) {
        let stripped = raw_line
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string();
        let mut spans: Vec<Span<'static>> =
            vec![Span::styled("  │ ", Style::default().fg(border_color))];
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
                spans.push(Span::styled(
                    stripped,
                    Style::default().fg(theme().inline_code),
                ));
            }
        }
        result.push(Line::from(spans));
    }
    result
}

// ── Markdown → ratatui Lines ──────────────────────────────────────────────────

#[allow(clippy::too_many_lines)]
fn markdown_to_tui_lines(text: &str, wrap_width: usize) -> Vec<Line<'static>> {
    fn flush(lines: &mut Vec<Line<'static>>, spans: &mut Vec<Span<'static>>, width: &mut usize) {
        if !spans.is_empty() {
            lines.push(Line::from(std::mem::take(spans)));
        }
        *width = 0;
    }

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_line_width: usize = 0;
    let mut bold = false;
    let mut italic = false;
    let mut heading: Option<u8> = None;
    let mut in_code = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();
    let mut list_depth: usize = 0;

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
            _ => theme().text_primary,
        })
    };

    for event in Parser::new_ext(text, Options::all()) {
        match event {
            MdEvent::Start(Tag::Heading { level, .. }) => {
                flush(&mut lines, &mut spans, &mut current_line_width);
                heading = Some(level as u8);
            }
            MdEvent::End(TagEnd::Heading(..)) => {
                flush(&mut lines, &mut spans, &mut current_line_width);
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
                flush(&mut lines, &mut spans, &mut current_line_width);
                let indent = "  ".repeat(list_depth.saturating_sub(1));
                spans.push(Span::styled(
                    format!("{indent}• "),
                    Style::default().fg(theme().primary_accent),
                ));
            }
            MdEvent::End(TagEnd::Item) | MdEvent::HardBreak => flush(&mut lines, &mut spans, &mut current_line_width),
            MdEvent::Start(Tag::CodeBlock(kind)) => {
                in_code = true;
                code_buffer.clear();
                flush(&mut lines, &mut spans, &mut current_line_width);
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
                    Span::styled(
                        lang_display,
                        Style::default().fg(lc).add_modifier(Modifier::BOLD),
                    ),
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
                lines.push(Line::from(Span::styled(
                    "  ╰──────",
                    Style::default().fg(lc),
                )));
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
                    // Word-wrap: split on whitespace, track current line width,
                    // flush to a new line when adding the next word would exceed wrap_width.
                    for word in t.split_whitespace() {
                        let wlen = word.chars().count();
                        if wlen >= wrap_width {
                            // Token longer than the full width: hard-break it char by char.
                            if current_line_width > 0 {
                                flush(&mut lines, &mut spans, &mut current_line_width);
                            }
                            let chars: Vec<char> = word.chars().collect();
                            for chunk in chars.chunks(wrap_width) {
                                let s: String = chunk.iter().collect();
                                let clen = chunk.len();
                                spans.push(Span::styled(s, style));
                                current_line_width += clen;
                                if current_line_width >= wrap_width {
                                    flush(&mut lines, &mut spans, &mut current_line_width);
                                }
                            }
                        } else {
                            // Normal word: prepend space separator if not at line start.
                            let need = if current_line_width > 0 { 1 + wlen } else { wlen };
                            if current_line_width > 0 && current_line_width + need > wrap_width {
                                flush(&mut lines, &mut spans, &mut current_line_width);
                            }
                            if current_line_width > 0 {
                                spans.push(Span::raw(" "));
                                current_line_width += 1;
                            }
                            spans.push(Span::styled(word.to_string(), style));
                            current_line_width += wlen;
                        }
                    }
                }
            }
            MdEvent::Code(t) => {
                let code = t.into_string();
                let clen = code.chars().count();
                let need = if current_line_width > 0 { 1 + clen } else { clen };
                if current_line_width > 0 && current_line_width + need > wrap_width {
                    flush(&mut lines, &mut spans, &mut current_line_width);
                }
                if current_line_width > 0 {
                    spans.push(Span::raw(" "));
                    current_line_width += 1;
                }
                spans.push(Span::styled(code, Style::default().fg(theme().inline_code)));
                current_line_width += clen;
            }
            MdEvent::SoftBreak => {
                // A SoftBreak within a paragraph is a word separator.
                // Only add a space if there's accumulated content and it fits.
                if current_line_width > 0 && current_line_width < wrap_width {
                    spans.push(Span::raw(" "));
                    current_line_width += 1;
                } else if current_line_width > 0 {
                    flush(&mut lines, &mut spans, &mut current_line_width);
                }
            }
            MdEvent::End(TagEnd::Paragraph) => {
                flush(&mut lines, &mut spans, &mut current_line_width);
                lines.push(Line::from(""));
            }
            MdEvent::Rule => {
                flush(&mut lines, &mut spans, &mut current_line_width);
                lines.push(Line::from(Span::styled(
                    "─".repeat(wrap_width.min(60)),
                    Style::default().fg(theme().text_secondary),
                )));
                lines.push(Line::from(""));
            }
            _ => {}
        }
    }
    flush(&mut lines, &mut spans, &mut current_line_width);

    // Remove trailing blank lines
    while lines
        .last()
        .is_some_and(|l: &Line| l.spans.iter().all(|s| s.content.trim().is_empty()))
    {
        lines.pop();
    }
    lines
}

/// Onboarding: renderiza a dica atual centralizada quando `app.chat` está vazio.
fn render_tips(app: &UiApp, width: usize) -> Vec<Line<'static>> {
    use ansi_to_tui::IntoText;

    let mut lines: Vec<Line<'static>> = Vec::new();
    let Some((tip, idx, total)) = app.current_tip() else {
        return lines;
    };

    // Largura útil do conteúdo da dica (corpo wrapped). Limita a 80 cols mesmo
    // em telas largas para preservar legibilidade.
    let content_width = width.saturating_sub(8).clamp(20, 80);
    // Padding lateral para centralizar o bloco na área disponível.
    let pad_left = width.saturating_sub(content_width) / 2;
    let pad = " ".repeat(pad_left);

    let title_style = Style::default()
        .fg(theme().primary_accent)
        .add_modifier(Modifier::BOLD);
    let body_style = Style::default().fg(theme().text_primary);
    let dim = Style::default().fg(theme().text_secondary);

    // Mascote centralizado acima das dicas. Cada sprite tem ~28 cols de largura;
    // calcula o pad para centralizar horizontalmente na largura do chat.
    if let Some(comp) = app.companion.as_ref() {
        let raw = runtime::buddy::sprite_for_id(comp.pokemon_id);
        if let Ok(sprite_text) = raw.into_text() {
            let sprite_width = sprite_text
                .lines
                .iter()
                .map(ratatui::prelude::Line::width)
                .max()
                .unwrap_or(0);
            let sprite_pad = " ".repeat(width.saturating_sub(sprite_width) / 2);
            lines.push(Line::from(Span::raw("")));
            for sprite_line in sprite_text.lines {
                let mut spans: Vec<Span<'static>> = vec![Span::raw(sprite_pad.clone())];
                spans.extend(sprite_line.spans);
                lines.push(Line::from(spans));
            }
            // Linha-resumo (Nome · Mascote #ID) centralizada abaixo do sprite.
            let summary = comp.summary_line();
            let summary_w = summary.chars().count();
            let summary_pad = " ".repeat(width.saturating_sub(summary_w) / 2);
            lines.push(Line::from(vec![
                Span::raw(summary_pad),
                Span::styled(
                    summary,
                    Style::default()
                        .fg(theme().easter_egg.warm)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }
    }

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

fn thinking_footer_chat_line(app: &UiApp, wrap_width: usize) -> Option<Line<'static>> {
    if !app.thinking {
        return None;
    }
    let caption_budget = wrap_width.saturating_sub(20).clamp(8, 512);
    let caption_raw =
        crate::thinking_footer::thinking_footer_caption(app.ultrathink_active, app.caption_idx);
    let caption = crate::thinking_footer::truncate_graphemes(&caption_raw, caption_budget).into_owned();
    let frame_disp = app.think_eyes_spinner.frame_str();
    let dots = app.dots_spinner.frame_str();
    let line_text = if app.ultrathink_active {
        format!("  {frame_disp} ⚡ {caption}{dots}")
    } else {
        format!("  {frame_disp} {caption}{dots}")
    };
    let color = if app.ultrathink_active {
        ratatui::style::Color::Rgb(255, 200, 50)
    } else {
        theme().thinking
    };
    Some(Line::from(Span::styled(
        line_text,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )))
}

fn lines_for_user_message(msg: &str, wrap_width: usize) -> Vec<Line<'static>> {
    let mut result = Vec::new();
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
    result
}

fn lines_for_assistant_text(text: &str, wrap_width: usize) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    let md_lines = markdown_to_tui_lines(text, wrap_width.saturating_sub(2));
    for line in md_lines {
        let mut indented_spans = vec![Span::raw("  ")];
        indented_spans.extend(line.spans);
        result.push(Line::from(indented_spans));
    }
    result.push(Line::from(""));
    result
}

fn lines_for_tool_batch(app: &UiApp, items: &[ToolBatchItem], closed: bool) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    result.push(Line::from(vec![
        Span::styled("  \u{2699} ".to_string(), Style::default().fg(theme().info)),
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
                let frame = app.tool_spinner.frame_str();
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
    if closed {
        result.push(Line::from(""));
    }
    result
}

fn lines_for_thinking_block(app: &UiApp, text: &str, finished: bool) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    let (icon, label) = if finished {
        ("\u{1f4ad}", format!("Pensamento ({} chars)", text.len()))
    } else {
        let frame = app.tool_spinner.frame_str();
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
                .add_modifier(if finished {
                    Modifier::DIM
                } else {
                    Modifier::BOLD
                }),
        ),
    ]));
    result.push(Line::from(""));
    result
}

/// Parseia um bloco ANSI 256-color (ex.: sprite Pokémon) para `Line`s ratatui.
/// Usa o crate `ansi-to-tui` que converte cada `\x1b[...m` em `Style` apropriado.
fn lines_for_ansi_block(ansi: &str) -> Vec<Line<'static>> {
    use ansi_to_tui::IntoText;
    match ansi.into_text() {
        Ok(text) => text.lines.into_iter().collect(),
        Err(_) => vec![Line::from(Span::raw(ansi.to_string()))],
    }
}

fn lines_for_system_note(note: &str, wrap_width: usize) -> Vec<Line<'static>> {
    let style = Style::default().fg(theme().warn);
    let indent = "  ";
    let inner_width = wrap_width.saturating_sub(indent.len());
    let mut result = Vec::new();
    for line in note.lines() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut col: usize = 0;
        let mut first = true;
        for word in line.split_whitespace() {
            let wlen = word.chars().count();
            let need = if col > 0 { 1 + wlen } else { wlen };
            if col > 0 && col + need > inner_width {
                result.push(Line::from(std::mem::take(&mut spans)));
                col = 0;
                first = true;
            }
            if first {
                spans.push(Span::styled(format!("{indent}{word}"), style));
                first = false;
            } else {
                spans.push(Span::styled(format!(" {word}"), style));
            }
            col += need;
        }
        if !spans.is_empty() || line.trim().is_empty() {
            result.push(Line::from(spans));
        }
    }
    result.push(Line::from(""));
    result
}

fn lines_for_swd_log(
    transactions: &[crate::swd::SwdTransaction],
    mode: crate::swd::SwdLevel,
) -> Vec<Line<'static>> {
    use crate::swd::SwdOutcome;
    let mut result = Vec::new();
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
    result
}

fn lines_for_correction_retry(attempt: u8, max_attempts: u8) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    result.push(Line::from(Span::styled(
        format!("  \u{21a9} SWD retry {attempt}/{max_attempts}"),
        Style::default()
            .fg(theme().warn)
            .add_modifier(Modifier::BOLD),
    )));
    result.push(Line::from(""));
    result
}

fn lines_for_swd_diff(path: &str, hunks: &[crate::diff::DiffHunk]) -> Vec<Line<'static>> {
    use crate::diff::DiffTag;
    let mut result = Vec::new();
    result.push(Line::from(Span::styled(
        format!("  --- {path}"),
        Style::default()
            .fg(theme().info)
            .add_modifier(Modifier::BOLD),
    )));
    if hunks.is_empty() {
        result.push(Line::from(Span::styled(
            "  (Nenhuma alteração detectada)",
            Style::default().fg(theme().text_secondary),
        )));
    } else {
        for hunk in hunks {
            let old_count = hunk
                .lines
                .iter()
                .filter(|l| matches!(l.tag, DiffTag::Keep | DiffTag::Remove))
                .count();
            let new_count = hunk
                .lines
                .iter()
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
                    DiffTag::Keep => (" ", Style::default().fg(theme().text_secondary)),
                    DiffTag::Remove => (
                        "-",
                        Style::default()
                            .fg(theme().error)
                            .add_modifier(Modifier::BOLD),
                    ),
                    DiffTag::Add => (
                        "+",
                        Style::default()
                            .fg(theme().success)
                            .add_modifier(Modifier::BOLD),
                    ),
                };
                let lineno = match line.tag {
                    DiffTag::Add => "     ".to_string(),
                    _ => line
                        .old_lineno
                        .map_or_else(|| "     ".to_string(), |n| format!("{n:>4} ")),
                };
                result.push(Line::from(vec![
                    Span::styled(format!("  {lineno}| {marker} "), style),
                    Span::styled(line.value.clone(), style),
                ]));
            }
        }
    }
    result.push(Line::from(""));
    result
}

fn lines_for_task_progress(
    app: &UiApp,
    label: &str,
    msg: &str,
    events: &[String],
    finished: bool,
    status: Option<runtime::TaskStatus>,
) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    let border_color = task_label_color(label);
    let (prefix, prefix_color) = if finished {
        match status {
            Some(runtime::TaskStatus::Failed) => ("\u{2717}", theme().error), // ✗
            Some(runtime::TaskStatus::Killed) => ("\u{2298}", theme().warn),  // ⊘
            _ => ("\u{2713}", theme().success),
        }
    } else {
        let frame = app.tool_spinner.frame_str();
        (frame, border_color)
    };

    let header_spinner = if finished {
        prefix.to_string()
    } else {
        let frames = spinner_for_msg(msg);
        frames[app.tool_spinner.frame() % frames.len()].to_string()
    };
    result.push(Line::from(vec![
        Span::styled("  ╭─ ", Style::default().fg(border_color)),
        Span::styled(
            format!("{header_spinner} "),
            Style::default()
                .fg(prefix_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            label.to_string(),
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ─", Style::default().fg(border_color)),
    ]));

    let inner_width = 88usize;
    if events.is_empty() {
        result.push(Line::from(vec![
            Span::styled("  │ ", Style::default().fg(border_color)),
            Span::styled(msg.to_string(), Style::default().fg(theme().text_secondary)),
        ]));
    } else {
        let mut visual_lines: Vec<(bool, String)> = Vec::new();
        for ev in events {
            let chunks = wrap_event_lines(ev, inner_width, 4);
            for (j, chunk) in chunks.into_iter().enumerate() {
                visual_lines.push((j == 0, chunk));
            }
        }
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
                Span::styled(chunk.clone(), Style::default().fg(theme().text_secondary)),
            ]));
        }
    }

    result.push(Line::from(Span::styled(
        "  ╰──────",
        Style::default().fg(border_color),
    )));

    if finished {
        result.push(Line::from(""));
    }
    result
}

fn lines_for_chat_entry(app: &UiApp, entry: &ChatEntry, wrap_width: usize) -> Vec<Line<'static>> {
    match entry {
        ChatEntry::UserMessage(msg) => lines_for_user_message(msg, wrap_width),
        ChatEntry::AssistantText(text) => lines_for_assistant_text(text, wrap_width),
        ChatEntry::ToolBatchEntry { items, closed } => {
            lines_for_tool_batch(app, items.as_slice(), *closed)
        }
        ChatEntry::ThinkingBlock { text, finished } => {
            lines_for_thinking_block(app, text, *finished)
        }
        ChatEntry::SystemNote(note) => lines_for_system_note(note.as_str(), wrap_width),
        ChatEntry::AnsiBlock(ansi) => lines_for_ansi_block(ansi.as_str()),
        ChatEntry::SwdLogEntry { transactions, mode } => {
            lines_for_swd_log(transactions.as_slice(), *mode)
        }
        ChatEntry::CorrectionRetryEntry {
            attempt,
            max_attempts,
        } => lines_for_correction_retry(*attempt, *max_attempts),
        ChatEntry::ToolDiff { path, hunks } => {
            lines_for_swd_diff(path.as_str(), hunks.as_slice())
        }
        ChatEntry::ToolOutputSummary { summary } => {
            let mut lines = vec![Line::from(Span::styled(
                "📄 Output: ".to_string(),
                Style::default()
                    .fg(theme().text_secondary)
                    .add_modifier(Modifier::BOLD),
            ))];
            for ln in summary.lines() {
                lines.push(Line::from(Span::styled(
                    format!("   {ln}"),
                    Style::default().fg(theme().text_secondary),
                )));
            }
            lines.push(Line::from(""));
            lines
        }
        ChatEntry::SwdDiffEntry { path, hunks } => {
            lines_for_swd_diff(path.as_str(), hunks.as_slice())
        }
        ChatEntry::TaskProgress {
            label,
            msg,
            events,
            finished,
            status,
            ..
        } => lines_for_task_progress(
            app,
            label.as_str(),
            msg.as_str(),
            events.as_slice(),
            *finished,
            *status,
        ),
    }
}

fn chat_to_lines(app: &UiApp, width: usize) -> Vec<Line<'static>> {
    let wrap_width = width.saturating_sub(4).max(20);
    if app.show_tips && app.chat.is_empty() {
        return render_tips(app, width);
    }
    let mut result = Vec::new();
    for entry in &app.chat {
        result.extend(lines_for_chat_entry(app, entry, wrap_width));
    }
    if let Some(line) = thinking_footer_chat_line(app, wrap_width) {
        result.push(line);
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
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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
    (
        format!("[{}{}]", "|".repeat(filled), " ".repeat(empty)),
        color,
    )
}

fn draw_status(frame: &mut ratatui::Frame, area: Rect, app: &UiApp) {
    let spinner = "·";
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
    } else {
        Style::default().fg(ratatui::style::Color::Rgb(255, 191, 0))
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
            .map_or(chars.len(), |i| pos + i);

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
    if app.read_mode {
        draw_read_mode_banner(frame, area);
        return;
    }
    draw_normal_input(frame, area, app);
}

fn draw_read_mode_banner(frame: &mut ratatui::Frame, area: Rect) {
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
            Style::default()
                .fg(theme().warn)
                .add_modifier(Modifier::BOLD),
        )),
        layout[0],
    );
    frame.render_widget(
        Paragraph::new("  Pressione qualquer tecla para retomar o modo TUI")
            .style(Style::default().fg(theme().text_secondary)),
        layout[1],
    );
}

fn draw_normal_input(frame: &mut ratatui::Frame, area: Rect, app: &UiApp) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().border_active));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let input_area = layout[0];

    // Hint line (widget dedicado para facilitar evolução futura do help).
    let shortcuts = vec![
        HelpBinding::new("/", "comandos"),
        HelpBinding::new("↑/↓", "histórico"),
        HelpBinding::new("F2", "modelo"),
        HelpBinding::new("F3", "perm"),
        HelpBinding::new("F4", "sessão"),
        HelpBinding::new("Ctrl+H", "header"),
        HelpBinding::new("Ctrl+R", "leitura"),
        HelpBinding::new("arrastar", "copiar"),
        HelpBinding::new("Ctrl+C", "sair"),
    ];
    let help =
        Help::default()
            .bindings(shortcuts)
            .show_all(false)
            .styles(HelpStyles::from_palette(
                &cheese_palette_from_theme(theme()),
            ));
    frame.render_widget(help, layout[1]);

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
    let first_visible = (cursor_row + 1).saturating_sub(max_visible);

    for (vis_i, row_i) in (first_visible..(first_visible + max_visible).min(rows.len())).enumerate()
    {
        let (row_start, row_end) = rows[row_i];
        let prefix = if row_i == 0 { "> " } else { "  " };
        let is_cursor_row = row_i == cursor_row;
        let row_area = Rect {
            y: input_area.y + u16::try_from(vis_i).unwrap_or(u16::MAX),
            height: 1,
            ..input_area
        };

        let mut spans = vec![Span::styled(
            prefix,
            Style::default().fg(theme().primary_accent),
        )];

        if is_cursor_row {
            let local = app.cursor_col.saturating_sub(row_start);
            let row_chars = &chars[row_start..row_end];
            let before: String = row_chars[..local.min(row_chars.len())].iter().collect();
            let cursor_char = row_chars
                .get(local)
                .map_or_else(|| " ".to_string(), std::string::ToString::to_string);
            let after: String = row_chars.get(local + 1..).unwrap_or(&[]).iter().collect();
            if !before.is_empty() {
                spans.push(Span::styled(
                    before,
                    Style::default().fg(theme().text_primary),
                ));
            }
            spans.push(Span::styled(
                cursor_char,
                Style::default()
                    .fg(theme().accent_on_primary_bg)
                    .bg(theme().primary_accent)
                    .add_modifier(Modifier::BOLD),
            ));
            if !after.is_empty() {
                spans.push(Span::styled(
                    after,
                    Style::default().fg(theme().text_primary),
                ));
            }
        } else {
            let text: String = chars[row_start..row_end].iter().collect();
            if !text.is_empty() {
                spans.push(Span::styled(
                    text,
                    Style::default().fg(theme().text_primary),
                ));
            }
        }

        frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
    }
}

// ── Overlays ──────────────────────────────────────────────────────────────────

/// Desenha um toast de notificação no canto inferior direito.
/// Auto-dismiss: `app.toast_deadline` é gerenciado externamente (via tick/timeout).
fn draw_toast(frame: &mut ratatui::Frame, area: Rect, message: &str) {
    let t = theme();
    // Largura do toast: mínimo do conteúdo + padding.
    let content_w = message.chars().count() as u16 + 4;
    let w = content_w.min(area.width.saturating_sub(4)).max(10);
    let h = 3_u16;

    // Posição: canto inferior direito, 2 células de margem.
    let x = area.right().saturating_sub(w + 2);
    let y = area.bottom().saturating_sub(h + 2);
    let rect = Rect::new(x, y, w, h);

    // Fundo semi-transparente + borda colorida.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.easter_egg.warm))
        .style(Style::default().bg(ratatui::style::Color::DarkGray));

    let para = Paragraph::new(Line::from(Span::styled(
        format!(" {} ", message),
        Style::default()
            .fg(ratatui::style::Color::White)
            .add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center);

    frame.render_widget(Clear, rect); // limpa fundo
    frame.render_widget(block, rect);
    let inner = Rect::new(rect.x + 1, rect.y + 1, rect.width.saturating_sub(2), rect.height.saturating_sub(2));
    frame.render_widget(para, inner);
}

#[allow(clippy::too_many_lines)]
fn draw_overlay(frame: &mut ratatui::Frame, area: Rect, overlay: &OverlayKind, app: &UiApp) {
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
            filter,
            selected,
            items,
        } => {
            let items = UiApp::filter_model_items(items, filter);
            draw_picker(
                frame,
                area,
                "Selecione o modelo",
                &items
                    .iter()
                    .map(std::string::String::as_str)
                    .collect::<Vec<_>>(),
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
                &items
                    .iter()
                    .map(std::string::String::as_str)
                    .collect::<Vec<_>>(),
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
                &items
                    .iter()
                    .map(std::string::String::as_str)
                    .collect::<Vec<_>>(),
                *selected,
                None,
                &format!("atual: {current}"),
            );
        }
        OverlayKind::SlashPalette {
            items,
            filter,
            category_selected,
            selected,
            focus,
        } => {
            let columns = build_palette_columns(items, filter);
            draw_slash_palette_grouped(
                frame,
                area,
                &columns,
                *category_selected,
                *selected,
                *focus,
                filter,
            );
        }
        OverlayKind::SessionPicker { items, selected } => {
            let labels: Vec<String> = items
                .iter()
                .map(|(id, title, count)| {
                    let name = title.as_deref().unwrap_or(id);
                    let short_id = id.strip_prefix("session-").unwrap_or(id);
                    if title.is_some() {
                        format!("{name:<24} ({count} msgs)")
                    } else {
                        format!("{short_id:<20} ({count} msgs)")
                    }
                })
                .collect();
            draw_picker(
                frame,
                area,
                "Sessões recentes",
                &labels
                    .iter()
                    .map(std::string::String::as_str)
                    .collect::<Vec<_>>(),
                *selected,
                None,
                "",
            );
        }
        OverlayKind::ScriptPicker { items, selected } => {
            let labels: Vec<String> = items
                .iter()
                .map(|(name, _)| format!("  {name}"))
                .collect();
            draw_picker(
                frame,
                area,
                "Scripts disponíveis",
                &labels.iter().map(String::as_str).collect::<Vec<_>>(),
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
            let labels: Vec<String> = filtered.iter().take(8).map(|p| format!("  {p}")).collect();
            draw_picker(
                frame,
                area,
                &title,
                &labels
                    .iter()
                    .map(std::string::String::as_str)
                    .collect::<Vec<_>>(),
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
    let height = (u16::try_from(items.len()).unwrap_or(u16::MAX) + 6).min(area.height - 4);
    let x = (area.width.saturating_sub(width)) / 2 + area.x;
    let y = (area.height.saturating_sub(height)) / 2 + area.y;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);
    let palette = cheese_palette_from_theme(theme());
    let fieldset = Fieldset::new()
        .title(title)
        .fill(FieldsetFill::Dash)
        .styles(FieldsetStyles::from_palette(&palette));
    let inner = fieldset.inner(popup);
    frame.render_widget(fieldset, popup);

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
    let hint_area = if filter.is_some() {
        layout[2]
    } else {
        layout[1]
    };

    let mut list_items: Vec<CheeseRenderRow> = items
        .iter()
        .map(|item| CheeseRenderRow {
            text: (*item).to_string(),
            kind: CheeseRowKind::Option,
        })
        .collect();
    if list_items.is_empty() {
        list_items.push(CheeseRenderRow {
            text: "sem resultados".to_string(),
            kind: CheeseRowKind::ComingSoon,
        });
    }
    let mut list_state = CheeseListState::new(list_items.len());
    list_state.select(
        selected.min(list_items.len().saturating_sub(1)),
        list_items.len(),
    );
    let list = CheeseList::new(&list_items)
        .palette(palette.clone())
        .show_paginator(false)
        .item_spacing(0)
        .selection_indicator("▶");
    frame.render_stateful_widget(list, list_area, &mut list_state);

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

/// Render da paleta Ctrl+K em duas colunas (categorias + comandos).
#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
fn draw_slash_palette_grouped(
    frame: &mut ratatui::Frame,
    area: Rect,
    columns: &[(SlashCategory, Vec<(String, String)>)],
    category_selected: usize,
    selected: usize,
    focus: SlashPaletteFocus,
    filter: &str,
) {
    let width = (area.width * 3 / 4).max(70).min(area.width - 4);
    // +6 para borda + filtro + hint; usa mesmo cálculo do draw_picker.
    let height = (u16::try_from(columns.len()).unwrap_or(u16::MAX) + 8).min(area.height - 4);
    let x = (area.width.saturating_sub(width)) / 2 + area.x;
    let y = (area.height.saturating_sub(height)) / 2 + area.y;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);
    let palette = cheese_palette_from_theme(theme());
    let fieldset = Fieldset::new()
        .title("Slash Commands (Ctrl+K)")
        .fill(FieldsetFill::Dash)
        .styles(FieldsetStyles::from_palette(&palette));
    let inner = fieldset.inner(popup);
    frame.render_widget(fieldset, popup);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let content_area = layout[0];
    let filter_area = layout[1];
    let hint_area = layout[2];

    let columns_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(content_area);
    let left_area = columns_layout[0];
    let right_area = columns_layout[1];

    let left_title = if matches!(focus, SlashPaletteFocus::Categories) {
        "Categorias ●"
    } else {
        "Categorias"
    };
    let right_title = if matches!(focus, SlashPaletteFocus::Commands) {
        "Comandos ●"
    } else {
        "Comandos"
    };
    let left_fieldset = Fieldset::new()
        .title(left_title)
        .fill(FieldsetFill::Dash)
        .styles(FieldsetStyles::from_palette(&palette));
    let right_fieldset = Fieldset::new()
        .title(right_title)
        .fill(FieldsetFill::Dash)
        .styles(FieldsetStyles::from_palette(&palette));
    let left_inner = left_fieldset.inner(left_area);
    let right_inner = right_fieldset.inner(right_area);
    frame.render_widget(left_fieldset, left_area);
    frame.render_widget(right_fieldset, right_area);

    let mut category_items: Vec<CheeseRenderRow> = columns
        .iter()
        .map(|(cat, cmds)| CheeseRenderRow {
            text: format!("{} ({})", category_label_pt(*cat), cmds.len()),
            kind: CheeseRowKind::Option,
        })
        .collect();
    if category_items.is_empty() {
        category_items.push(CheeseRenderRow {
            text: "sem categorias".to_string(),
            kind: CheeseRowKind::ComingSoon,
        });
    }
    let mut category_state = CheeseListState::new(category_items.len());
    category_state.select(
        category_selected.min(category_items.len().saturating_sub(1)),
        category_items.len(),
    );
    let category_list = CheeseList::new(&category_items)
        .palette(palette.clone())
        .show_paginator(false)
        .item_spacing(0)
        .selection_indicator("▶");
    frame.render_stateful_widget(category_list, left_inner, &mut category_state);

    let mut command_items: Vec<CheeseRenderRow> = if let Some((_, cmds)) =
        columns.get(category_selected.min(columns.len().saturating_sub(1)))
    {
        cmds.iter()
            .map(|(cmd, desc)| {
                let coming_soon = is_command_coming_soon(cmd);
                let suffix = if coming_soon { "  (em breve)" } else { "" };
                let body = format!("/{cmd:<12} {desc}{suffix}");
                CheeseRenderRow {
                    text: body,
                    kind: if coming_soon {
                        CheeseRowKind::ComingSoon
                    } else {
                        CheeseRowKind::Option
                    },
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    if command_items.is_empty() {
        command_items.push(CheeseRenderRow {
            text: "sem comandos para este filtro".to_string(),
            kind: CheeseRowKind::ComingSoon,
        });
    }
    let mut command_state = CheeseListState::new(command_items.len());
    command_state.select(
        selected.min(command_items.len().saturating_sub(1)),
        command_items.len(),
    );
    let list = CheeseList::new(&command_items)
        .palette(palette.clone())
        .show_paginator(false)
        .item_spacing(0)
        .selection_indicator("▶");
    frame.render_stateful_widget(list, right_inner, &mut command_state);

    frame.render_widget(
        Paragraph::new(format!("  filtro: {filter}_"))
            .style(Style::default().fg(theme().text_secondary)),
        filter_area,
    );
    frame.render_widget(
        Paragraph::new("  ←/→ foco colunas · ↑/↓ navegar · Enter executar · Esc cancelar")
            .style(Style::default().fg(theme().text_secondary)),
        hint_area,
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheeseRowKind {
    Option,
    ComingSoon,
}

#[derive(Debug, Clone)]
struct CheeseRenderRow {
    text: String,
    kind: CheeseRowKind,
}

impl CheeseListItem for CheeseRenderRow {
    fn height(&self) -> u16 {
        1
    }

    fn render(&self, area: Rect, buf: &mut Buffer, ctx: &CheeseListItemContext) {
        if area.width == 0 {
            return;
        }
        let (prefix, style) = match self.kind {
            CheeseRowKind::ComingSoon if !ctx.selected => {
                ("  ", Style::default().fg(ctx.palette.muted))
            }
            _ if ctx.selected => (
                "  ",
                Style::default()
                    .fg(ctx.palette.on_highlight)
                    .bg(ctx.palette.highlight),
            ),
            _ => ("  ", Style::default().fg(ctx.palette.foreground)),
        };
        let width = area.width as usize;
        let mut text = format!("{prefix}{}", self.text);
        if text.chars().count() > width {
            text = text
                .chars()
                .take(width.saturating_sub(1))
                .collect::<String>()
                + "…";
        }
        buf.set_string(area.x, area.y, text, style);
    }
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
                format!(
                    "  {:<11} ",
                    rust_i18n::t!("tui.tool_approval.required_label")
                ),
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
            Span::styled(
                "  [ Y ] ",
                Style::default()
                    .fg(theme().success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}   ", rust_i18n::t!("tui.tool_approval.yes_once")),
                Style::default().fg(theme().success),
            ),
            Span::styled(
                "[ A ] ",
                Style::default()
                    .fg(theme().info)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}   ", rust_i18n::t!("tui.tool_approval.always")),
                Style::default().fg(theme().info),
            ),
            Span::styled(
                "[ N ] ",
                Style::default()
                    .fg(theme().error)
                    .add_modifier(Modifier::BOLD),
            ),
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
                Style::default()
                    .fg(theme().success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}    ", rust_i18n::t!("tui.swd_confirm.accept")),
                Style::default().fg(theme().text_primary),
            ),
            Span::styled(
                "[R] ",
                Style::default()
                    .fg(theme().error)
                    .add_modifier(Modifier::BOLD),
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
            Style::default()
                .fg(theme().text_primary)
                .add_modifier(Modifier::BOLD),
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

fn draw_deepresearch_key_input(frame: &mut ratatui::Frame, area: Rect, input: &str, cursor: usize) {
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
            Style::default()
                .fg(theme().text_primary)
                .add_modifier(Modifier::BOLD),
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
            format!("  {n_chars} caracteres"),
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
            _ => {
                if step == 1 {
                    "Anthropic"
                } else {
                    "OpenAI"
                }
            }
        };
        let field_label = format!(
            "  {}",
            rust_i18n::t!(
                "tui.setup.field_label",
                provider = provider_name.to_string()
            )
        );
        let masked: String = "\u{2022}".repeat(input.chars().count());
        let display = format!("  > {masked}");
        vec![
            Line::from(""),
            Line::from(Span::styled(
                field_label,
                Style::default().fg(theme().text_primary),
            )),
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

#[allow(clippy::too_many_lines)]
fn draw_first_run_wizard(
    frame: &mut ratatui::Frame,
    area: Rect,
    step: &WizardStep,
    state: &WizardState,
) {
    let width = (area.width * 2 / 3)
        .max(60)
        .min(area.width.saturating_sub(4));
    let height = 18u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2 + area.x;
    let y = (area.height.saturating_sub(height)) / 2 + area.y;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let (step_label, total_steps) = match step {
        WizardStep::Welcome => ("1", "6"),
        WizardStep::Provider { .. } => ("2", "6"),
        WizardStep::Model { .. } => ("3", "6"),
        WizardStep::Permissions { .. } => ("4", "6"),
        WizardStep::Defaults { .. } => ("5", "6"),
        WizardStep::Done => ("6", "6"),
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
                Style::default()
                    .fg(theme().text_primary)
                    .add_modifier(Modifier::BOLD),
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
                format!(
                    "   • {}",
                    rust_i18n::t!("tui.wizard.welcome.bullet_permissions")
                ),
                Style::default().fg(theme().text_primary),
            )),
            Line::from(Span::styled(
                format!(
                    "   • {}",
                    rust_i18n::t!("tui.wizard.welcome.bullet_defaults")
                ),
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

        WizardStep::Provider { selected } => {
            let mut lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  Escolha o provedor/canal:",
                    Style::default()
                        .fg(theme().text_primary)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            for (i, provider) in WIZARD_PROVIDER_OPTIONS.iter().enumerate() {
                let label = provider_label(*provider);
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

        WizardStep::Model { provider, selected } => {
            let models = models_for_provider(*provider);
            let mut lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!(
                        "  {} ({})",
                        rust_i18n::t!("tui.wizard.model.title"),
                        provider_label(*provider)
                    ),
                    Style::default()
                        .fg(theme().text_primary)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            for (i, model) in models.iter().enumerate() {
                if i == *selected {
                    lines.push(Line::from(Span::styled(
                        format!("  ▶ {model}"),
                        Style::default()
                            .fg(theme().accent_on_primary_bg)
                            .bg(theme().primary_accent)
                            .add_modifier(Modifier::BOLD),
                    )));
                } else {
                    lines.push(Line::from(Span::styled(
                        format!("    {model}"),
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
                    Style::default()
                        .fg(theme().text_primary)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            let read_only = rust_i18n::t!("tui.wizard.permissions.read_only_desc").to_string();
            let workspace_write =
                rust_i18n::t!("tui.wizard.permissions.workspace_write_desc").to_string();
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
                    Style::default()
                        .fg(theme().text_primary)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            for (i, (label, enabled)) in toggles.iter().enumerate() {
                let check = if *enabled { "[x]" } else { "[ ]" };
                let check_color = if *enabled {
                    theme().success
                } else {
                    theme().text_secondary
                };
                let is_focused = i == *focused;
                let prefix = if is_focused { "  ▶ " } else { "    " };
                if is_focused {
                    lines.push(Line::from(vec![
                        Span::styled(prefix, Style::default().fg(theme().primary_accent)),
                        Span::styled(
                            check,
                            Style::default()
                                .fg(check_color)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("  {label}"),
                            Style::default()
                                .fg(theme().text_primary)
                                .add_modifier(Modifier::BOLD),
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
            let bool_str = |v: bool| -> String {
                if v {
                    on.clone()
                } else {
                    off.clone()
                }
            };
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.wizard.done.title")),
                    Style::default()
                        .fg(theme().success)
                        .add_modifier(Modifier::BOLD),
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
                        format!(
                            "  {:<13}",
                            rust_i18n::t!("tui.wizard.done.label_permissions")
                        ),
                        Style::default().fg(theme().text_secondary),
                    ),
                    Span::styled(
                        state.permission_mode.clone(),
                        Style::default().fg(theme().warn),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!(
                            "  {:<13}",
                            rust_i18n::t!("tui.wizard.done.label_auto_update")
                        ),
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
        }
    };

    frame.render_widget(Paragraph::new(lines), inner);
}

#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
fn draw_auth_picker(frame: &mut ratatui::Frame, area: Rect, step: &AuthStep) {
    let width = (area.width * 2 / 3)
        .max(60)
        .min(area.width.saturating_sub(4));
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
        AuthStep::ProviderList { selected } => {
            let groups = provider_auth_groups();
            let mut lines: Vec<Line> = Vec::new();
            lines.push(Line::from(Span::styled(
                "  Select provider:",
                Style::default()
                    .fg(theme().text_primary)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            for (i, group) in groups.iter().enumerate() {
                let sel = i == *selected;
                let connected = is_provider_connected(group);
                let status = if connected { " \u{2713}" } else { "" };
                let name = format!("{}{}", group.name, status);
                if sel {
                    lines.push(Line::from(Span::styled(
                        format!("  \u{25b6} {name}"),
                        Style::default()
                            .fg(theme().accent_on_primary_bg)
                            .bg(theme().primary_accent)
                            .add_modifier(Modifier::BOLD),
                    )));
                } else {
                    let fg = if connected {
                        theme().success
                    } else {
                        theme().text_primary
                    };
                    lines.push(Line::from(Span::styled(
                        format!("    {name}"),
                        Style::default().fg(fg),
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

        AuthStep::MethodList {
            selected,
            provider,
            claude_code_detected,
            codex_detected,
        } => {
            let methods =
                auth_methods_visible_for_provider(provider, *claude_code_detected, *codex_detected);
            let mut lines: Vec<Line> = Vec::new();

            lines.push(Line::from(Span::styled(
                format!("  {}  \u{2190}", provider.name),
                Style::default()
                    .fg(theme().primary_accent)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));

            if *claude_code_detected && provider.id == "anthropic" {
                lines.push(Line::from(Span::styled(
                    format!("  {}", rust_i18n::t!("tui.auth.claude_code_detected")),
                    Style::default()
                        .fg(theme().success)
                        .add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
            }
            if *codex_detected && provider.id == "openai" {
                lines.push(Line::from(Span::styled(
                    "  Codex auth.json detectado".to_string(),
                    Style::default()
                        .fg(theme().success)
                        .add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
            }

            for (i, (_method, label)) in methods.iter().enumerate() {
                let sel = i == *selected;
                if sel {
                    lines.push(Line::from(Span::styled(
                        format!("  {:>2}. {}", i + 1, label),
                        Style::default()
                            .fg(theme().accent_on_primary_bg)
                            .bg(theme().primary_accent)
                            .add_modifier(Modifier::BOLD),
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
            let cur: String = input
                .chars()
                .nth(*cursor)
                .map_or_else(|| " ".to_string(), |c| c.to_string());
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
                    Span::styled(
                        cur,
                        Style::default()
                            .fg(theme().accent_on_primary_bg)
                            .bg(theme().primary_accent),
                    ),
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

        AuthStep::PasteSecret {
            method,
            input,
            masked,
            ..
        } => {
            let display = if *masked {
                "\u{2022}".repeat(input.chars().count())
            } else {
                input.clone()
            };
            let label = match method {
                AuthMethodChoice::PasteApiKey => {
                    rust_i18n::t!("tui.auth.paste.api_key_label").to_string()
                }
                AuthMethodChoice::PasteOpenAiKey => {
                    rust_i18n::t!("tui.auth.paste.openai_key_label").to_string()
                }
                _ => rust_i18n::t!("tui.auth.paste.token_label").to_string(),
            };
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {label}"),
                    Style::default().fg(theme().text_primary),
                )),
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

        AuthStep::BrowserFlow {
            url,
            port,
            started_at,
            ..
        } => {
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
                    Style::default()
                        .fg(theme().thinking)
                        .add_modifier(Modifier::BOLD),
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
                    Style::default()
                        .fg(theme().text_primary)
                        .add_modifier(Modifier::BOLD),
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

        AuthStep::Done { label, model: _ } => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  \u{2713} {label}"),
                    Style::default()
                        .fg(theme().success)
                        .add_modifier(Modifier::BOLD),
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
                    Style::default()
                        .fg(theme().error)
                        .add_modifier(Modifier::BOLD),
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

        AuthStep::ConnectedModels { provider, selected } => {
            let models = provider_models(provider);
            let mut lines: Vec<Line> = Vec::new();
            lines.push(Line::from(Span::styled(
                format!("  {}  \u{2713} connected", provider.name),
                Style::default()
                    .fg(theme().success)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                "  Available models:",
                Style::default().fg(theme().text_secondary),
            )));
            lines.push(Line::from(""));
            for (i, model) in models.iter().enumerate() {
                let sel = i == *selected;
                if sel {
                    lines.push(Line::from(Span::styled(
                        format!("  \u{25b6} {model}"),
                        Style::default()
                            .fg(theme().accent_on_primary_bg)
                            .bg(theme().primary_accent)
                            .add_modifier(Modifier::BOLD),
                    )));
                } else {
                    lines.push(Line::from(Span::styled(
                        format!("    {model}"),
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
            let k = l
                .trim_start_matches("export ")
                .split('=')
                .next()
                .unwrap_or("");
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

// ─── Small helpers ────────────────────────────────────────────────────────────

fn whoami_user() -> String {
    env::var("USER")
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_else(|_| "User".to_string())
}

/// Reconstrói um `Companion` síncrono (sem chamar LLM) a partir do que está em
/// disco — usado pelo header/painel de stats antes do hatch de background
/// terminar. Se não houver registro, usa apenas `roll_bones`.
fn load_initial_companion() -> Option<runtime::buddy::Companion> {
    let user_id = whoami_user();
    let stored = runtime::buddy::load_stored_companion();
    let bones = match stored.as_ref().and_then(|s| s.pokemon_id) {
        Some(id) => runtime::buddy::roll_bones_for(&user_id, id),
        None => runtime::buddy::roll_bones(&user_id),
    };
    let (soul, hatched_at) = match stored {
        Some(s) => (s.soul, s.hatched_at),
        None => (
            runtime::buddy::CompanionSoul {
                name: runtime::buddy::pokemon_name(bones.pokemon_id).to_string(),
                personality: String::new(),
            },
            0,
        ),
    };
    Some(runtime::buddy::Companion::from_parts(bones, soul, hatched_at))
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
        app.apply_tui_msg(TuiMsg::Usage {
            input_tokens: 100,
            output_tokens: 50,
        });
        app.apply_tui_msg(TuiMsg::Usage {
            input_tokens: 200,
            output_tokens: 75,
        });
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
                assert!(
                    *closed,
                    "first batch closed by the assistant text that follows"
                );
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
        let app = make_app();
        let all = app.active_provider_model_items();
        assert!(!all.is_empty());
        let gpt = UiApp::filter_model_items(&all, "gpt");
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
    fn open_auth_picker_seeds_provider_list() {
        let mut app = make_app();
        app.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::ProviderList { selected: 0 },
        });
        assert!(matches!(
            app.overlay,
            Some(OverlayKind::AuthPicker {
                step: AuthStep::ProviderList { selected: 0 }
            })
        ));
    }

    #[test]
    fn auth_picker_method_list_navigation_clamps() {
        let mut app = make_app();
        let provider = provider_auth_groups()
            .into_iter()
            .find(|g| g.id == "anthropic")
            .unwrap();
        let methods = auth_methods_visible_for_provider(&provider, false, false);
        let count = methods.len();

        app.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::MethodList {
                provider: provider.clone(),
                selected: 0,
                claude_code_detected: false,
                codex_detected: false,
            },
        });

        // Navigate up at 0 — should stay at 0.
        let key_up = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        );
        handle_overlay_key(&mut app, key_up);
        if let Some(OverlayKind::AuthPicker {
            step: AuthStep::MethodList { selected, .. },
        }) = &app.overlay
        {
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
        if let Some(OverlayKind::AuthPicker {
            step: AuthStep::MethodList { selected, .. },
        }) = &app.overlay
        {
            assert_eq!(*selected, count - 1, "should clamp at last item");
        } else {
            panic!("overlay should still be MethodList");
        }
    }

    #[test]
    fn auth_picker_esc_closes_overlay_from_provider_list() {
        let mut app = make_app();
        app.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::ProviderList { selected: 0 },
        });
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        );
        handle_overlay_key(&mut app, key);
        assert!(
            app.overlay.is_none(),
            "Esc should close the overlay from ProviderList"
        );
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
            step: AuthStep::Done {
                label: "test-label".to_string(),
                model: None,
            },
        });
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        let action = handle_overlay_key(&mut app, key);
        assert!(
            matches!(action, TuiAction::AuthComplete { label, .. } if label == "test-label"),
            "Done+Enter should return AuthComplete"
        );
        assert!(app.overlay.is_none(), "overlay should be closed after Done");
    }

    #[test]
    fn auth_picker_failed_step_goes_back_to_provider_list_on_enter() {
        let mut app = make_app();
        app.overlay = Some(OverlayKind::AuthPicker {
            step: AuthStep::Failed {
                error: "some error".to_string(),
            },
        });
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        handle_overlay_key(&mut app, key);
        assert!(
            matches!(
                app.overlay,
                Some(OverlayKind::AuthPicker {
                    step: AuthStep::ProviderList { .. }
                })
            ),
            "Failed+Enter should go back to ProviderList"
        );
    }

    #[test]
    fn auth_methods_visible_includes_import_when_detected() {
        let anthropic = provider_auth_groups()
            .into_iter()
            .find(|g| g.id == "anthropic")
            .unwrap();
        let openai = provider_auth_groups()
            .into_iter()
            .find(|g| g.id == "openai")
            .unwrap();

        let without = auth_methods_visible_for_provider(&anthropic, false, false);
        let with_cc = auth_methods_visible_for_provider(&anthropic, true, false);
        assert_eq!(with_cc.len(), without.len() + 1);
        assert_eq!(with_cc[0].0, AuthMethodChoice::ImportClaudeCode);

        let without_oai = auth_methods_visible_for_provider(&openai, false, false);
        let with_codex = auth_methods_visible_for_provider(&openai, false, true);
        assert_eq!(with_codex.len(), without_oai.len() + 1);
        assert_eq!(with_codex[0].0, AuthMethodChoice::ImportCodex);
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

        if let Some(OverlayKind::AuthPicker {
            step: AuthStep::EmailInput { input, cursor, .. },
        }) = &app.overlay
        {
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
            "match_b.rs".to_string(),           // basename match
        ];
        let filtered = filter_mention_items(&items, "match");
        assert_eq!(filtered.len(), 2);
        assert_eq!(
            filtered[0], "match_b.rs",
            "basename match should come first"
        );
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
            "should find foo.rs via walk; got: {paths:?}"
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
            Some(OverlayKind::FirstRunWizard {
                step: WizardStep::Welcome,
                ..
            }) => {}
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

        // Welcome -> Provider
        wizard_enter(&mut app);
        assert!(
            matches!(
                app.overlay,
                Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Provider { .. },
                    ..
                })
            ),
            "expected Provider step"
        );

        // Provider -> Model
        wizard_enter(&mut app);
        assert!(
            matches!(
                app.overlay,
                Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Model { .. },
                    ..
                })
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
                Some(OverlayKind::FirstRunWizard {
                    step: WizardStep::Done,
                    ..
                })
            ),
            "expected Done step"
        );
    }

    #[test]
    fn setup_wizard_model_selection_is_captured() {
        let mut app = make_app();
        app.open_first_run_wizard();
        wizard_enter(&mut app); // -> Provider
        wizard_enter(&mut app); // Provider(Anthropic) -> Model

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
        wizard_enter(&mut app); // Welcome -> Provider
        wizard_enter(&mut app); // Provider -> Model
        wizard_enter(&mut app); // Model -> Permissions
        wizard_enter(&mut app); // Permissions -> Defaults

        // Initially features.auto_update = true; Space should toggle it off
        handle_overlay_key(&mut app, make_key(KeyCode::Char(' ')));
        match &app.overlay {
            Some(OverlayKind::FirstRunWizard {
                step: WizardStep::Defaults { .. },
                state,
            }) => {
                assert!(
                    !state.features.auto_update,
                    "auto_update should be toggled off"
                );
            }
            other => panic!("expected Defaults, got: {other:?}"),
        }
    }

    #[test]
    fn setup_wizard_esc_from_welcome_closes_overlay() {
        let mut app = make_app();
        app.open_first_run_wizard();
        handle_overlay_key(&mut app, make_key(KeyCode::Esc));
        assert!(
            app.overlay.is_none(),
            "overlay should be closed after Esc on Welcome"
        );
    }

    #[test]
    fn setup_wizard_done_persists_global_config() {
        use std::sync::Mutex;
        static HOME_LOCK: Mutex<()> = Mutex::new(());
        struct HomeRestore(Option<std::ffi::OsString>);
        impl Drop for HomeRestore {
            fn drop(&mut self) {
                match &self.0 {
                    Some(p) => std::env::set_var("HOME", p),
                    None => std::env::remove_var("HOME"),
                }
            }
        }

        let td = tempfile::TempDir::new().unwrap();
        let _lock = HOME_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _restore = HomeRestore(std::env::var_os("HOME"));
        std::env::set_var("HOME", td.path());

        let mut app = make_app();
        app.open_first_run_wizard();
        wizard_enter(&mut app); // Welcome -> Provider
        wizard_enter(&mut app); // Provider -> Model
        wizard_enter(&mut app); // Model -> Permissions
        wizard_enter(&mut app); // Permissions -> Defaults
        wizard_enter(&mut app); // Defaults -> Done
        wizard_enter(&mut app); // Done -> close + persist

        assert!(
            app.overlay.is_none(),
            "overlay should be closed after Done+Enter"
        );
        let cfg = runtime::load_global_config().expect("config should be loadable");
        assert!(
            cfg.setup_complete,
            "setup_complete should be true after wizard"
        );
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
        // 6 comandos REPL-local: swd, auth, theme, uninstall, logout, exit.
        assert_eq!(items.len(), visible_spec_count + 6);

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
    fn clear_paste_state_resets_counter_and_map() {
        let mut app = make_app();
        app.paste_counter = 5;
        app.pasted_contents.insert(1, "hello".to_string());
        app.pasted_contents.insert(2, "world".to_string());

        app.clear_paste_state();

        assert_eq!(app.paste_counter, 0);
        assert!(app.pasted_contents.is_empty());
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
