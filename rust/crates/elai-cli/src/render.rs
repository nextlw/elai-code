use std::fmt::Write as FmtWrite;
use std::io::{self, Write};

use crossterm::cursor::{MoveToColumn, RestorePosition, SavePosition};
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor, Stylize};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{execute, queue};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

const DEFAULT_TEXT_SECONDARY_INTENSITY: u8 = 244;
const SECONDARY_INTENSITY_MIN: u8 = 232;
const SECONDARY_INTENSITY_MAX: u8 = 255;

// ─── Paleta única (fonte de verdade) ────────────────────────────────────────
//
// MANUTENÇÃO: cada campo de `ColorTheme` documenta os sites que o consomem. Se
// você alterar a cor, atualize o rustdoc. Se introduzir um literal `Color::Xxx`
// novo em qualquer lugar do crate, isso é um bug — registre-o como token aqui
// antes de fazer merge. A TUI ratatui consome via `ColorTheme::for_tui()`
// (retorna `RatatuiTheme`); ver `crossterm_to_ratatui` para a tabela de
// equivalência ANSI entre as duas crates.

/// Paleta única do Elai CLI/TUI, definida em `crossterm::style::Color`.
///
/// **Brilho ANSI**: os defaults usam variantes `Dark*` (ANSI 0–7) porque a TUI
/// ratatui histórica do projeto usa as cores escuras. Como `ratatui::Color::Cyan`
/// já é o ciano escuro (ANSI 6), `crossterm::Color::DarkCyan` aqui mantém paridade
/// visual quando o tema é convertido para a camada da TUI.
///
/// **Para a TUI**: use [`ColorTheme::for_tui`] para obter um [`RatatuiTheme`]
/// com os mesmos tokens já convertidos para `ratatui::style::Color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorTheme {
    /// Acento primário (laranja). **Default**: `AnsiValue(215)`. **Sites**:
    /// prompt `>`, bordas ativas de overlays, fundo de items selecionados.
    pub primary_accent: Color,

    /// Foreground sobre fundo `primary_accent` (alto contraste). **Default**:
    /// `Black`. **Sites**: texto de items selecionados em pickers.
    pub accent_on_primary_bg: Color,

    /// Texto primário do conteúdo. **Default**: `White`. **Sites**: corpo de
    /// mensagens, labels principais, headings nível 2 markdown.
    pub text_primary: Color,

    /// Texto secundário/meta (hints, paths, blockquote, estados "em breve").
    /// **Default**: `AnsiValue(244)`. **Sites**: labels auxiliares, hints de
    /// teclado, blockquote markdown, headings nível 4+, estados desabilitados.
    pub text_secondary: Color,

    /// Borda de painel/overlay ativo. **Default**: `AnsiValue(215)` (=
    /// `primary_accent`). **Sites**: `Block::default().border_style` em overlays.
    pub border_active: Color,

    /// Borda de painel/overlay inativo. **Default**: `AnsiValue(239)`.
    /// **Sites**: bordas neutras de subpainéis na TUI.
    pub border_inactive: Color,

    /// Borda de tabelas markdown. **Default**: `DarkCyan`. **Sites**:
    /// `render_table`, `render_table_row`.
    pub border_table: Color,

    /// Borda de fenced code blocks markdown. **Default**: `Grey`. **Sites**:
    /// `start_code_block`, `finish_code_block`.
    pub border_code_block: Color,

    /// Cor "informativa": tool calls, headings nível 1. **Default**: `DarkCyan`.
    /// **Sites**: `⚙ tool:` na TUI, `# Heading` no markdown CLI, header de hunk diff.
    pub info: Color,

    /// Sucesso (✓ tasks, OK em tool results, spinner OK). **Default**:
    /// `DarkGreen`. **Sites**: `Spinner::finish`, `TaskStatus::Completed`, SWD `Verified`.
    pub success: Color,

    /// Aviso (system notes, retry, drift). **Default**: `DarkYellow`. **Sites**:
    /// `SystemNote`, `CorrectionRetryEntry`, SWD `Drift/Noop`, modo de permissão.
    pub warn: Color,

    /// Erro (✗ tasks, falhas, spinner com falha). **Default**: `DarkRed`.
    /// **Sites**: `Spinner::fail`, `TaskStatus::Failed`, SWD `Failed/RolledBack`.
    pub error: Color,

    /// "Pensando" / spinner ativo. **Default**: `DarkBlue`. **Sites**:
    /// `Spinner::tick`, indicador "Thinking…", spinner de TaskProgress em execução.
    pub thinking: Color,

    /// Hyperlinks markdown e headings nível 3. **Default**: `DarkBlue`. **Sites**:
    /// `Tag::Link`, `Tag::Image`, heading nível 3.
    pub link: Color,

    /// `inline code` markdown (texto monoespaçado curto). **Default**:
    /// `AnsiValue(156)` (verde claro). **Sites**: `Event::Code`, code spans na TUI.
    pub inline_code: Color,

    /// Linhas de contexto em hunks de diff (`@@ … @@`) e ênfase italic.
    /// **Default**: `DarkMagenta`. **Sites**: `SwdDiffEntry`, italics markdown.
    pub diff_context: Color,

    /// Sub-tema isolado para a animação "dream" (easter egg). Mantido em
    /// namespace separado porque são cores muito específicas da animação,
    /// não fazem parte do design system geral.
    pub easter_egg: EasterEggTheme,
}

/// Sub-tema do easter egg "dream" (animação).
///
/// Cores são RGB exatos (não passam por ANSI). Wrapper para que o consumo
/// continue passando pelo tema único — nunca usar literais `Color::Rgb(…)`
/// fora deste sub-tema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EasterEggTheme {
    /// Corpo claro (areia). **Default**: `Rgb { r: 242, g: 222, b: 206 }`.
    pub body: Color,
    /// Tom quente (laranja queimado), bordas e olhos. **Default**: `Rgb { r: 201, g: 123, b: 74 }`.
    pub warm: Color,
    /// Tom escuro (terra), pupilas/detalhes. **Default**: `Rgb { r: 110, g: 65, b: 28 }`.
    pub dark: Color,
}

impl Default for EasterEggTheme {
    fn default() -> Self {
        Self {
            body: Color::Rgb { r: 242, g: 222, b: 206 },
            warm: Color::Rgb { r: 201, g: 123, b: 74 },
            dark: Color::Rgb { r: 110, g: 65, b: 28 },
        }
    }
}

impl Default for ColorTheme {
    fn default() -> Self {
        Self {
            primary_accent: Color::AnsiValue(215),
            accent_on_primary_bg: Color::Black,
            text_primary: Color::White,
            text_secondary: Color::AnsiValue(DEFAULT_TEXT_SECONDARY_INTENSITY),
            border_active: Color::AnsiValue(215),
            border_inactive: Color::AnsiValue(239),
            border_table: Color::DarkCyan,
            border_code_block: Color::Grey,
            info: Color::DarkCyan,
            success: Color::DarkGreen,
            warn: Color::DarkYellow,
            error: Color::DarkRed,
            thinking: Color::DarkBlue,
            link: Color::DarkBlue,
            inline_code: Color::AnsiValue(156),
            diff_context: Color::DarkMagenta,
            easter_egg: EasterEggTheme::default(),
        }
    }
}

impl ColorTheme {
    #[must_use]
    pub fn resolved() -> Self {
        let mut theme = Self::default();
        let config = runtime::load_global_config().ok();
        let overrides = config.as_ref().map(|cfg| &cfg.theme);

        theme.primary_accent = resolve_color(
            theme.primary_accent,
            "ELAI_THEME_PRIMARY_ACCENT",
            overrides.and_then(|o| o.primary_accent.as_deref()),
        );
        theme.accent_on_primary_bg = resolve_color(
            theme.accent_on_primary_bg,
            "ELAI_THEME_ACCENT_ON_PRIMARY_BG",
            overrides.and_then(|o| o.accent_on_primary_bg.as_deref()),
        );
        theme.text_primary = resolve_color(
            theme.text_primary,
            "ELAI_THEME_TEXT_PRIMARY",
            overrides.and_then(|o| o.text_primary.as_deref()),
        );
        theme.text_secondary = resolve_text_secondary(
            theme.text_secondary,
            "ELAI_THEME_TEXT_SECONDARY",
            overrides.and_then(|o| o.text_secondary.as_deref()),
            "ELAI_TEXT_SECONDARY_INTENSITY",
            overrides.and_then(|o| o.text_secondary_intensity),
        );
        theme.border_active = resolve_color(
            theme.border_active,
            "ELAI_THEME_BORDER_ACTIVE",
            overrides.and_then(|o| o.border_active.as_deref()),
        );
        theme.border_inactive = resolve_color(
            theme.border_inactive,
            "ELAI_THEME_BORDER_INACTIVE",
            overrides.and_then(|o| o.border_inactive.as_deref()),
        );
        theme.border_table = resolve_color(
            theme.border_table,
            "ELAI_THEME_BORDER_TABLE",
            overrides.and_then(|o| o.border_table.as_deref()),
        );
        theme.border_code_block = resolve_color(
            theme.border_code_block,
            "ELAI_THEME_BORDER_CODE_BLOCK",
            overrides.and_then(|o| o.border_code_block.as_deref()),
        );
        theme.info = resolve_color(
            theme.info,
            "ELAI_THEME_INFO",
            overrides.and_then(|o| o.info.as_deref()),
        );
        theme.success = resolve_color(
            theme.success,
            "ELAI_THEME_SUCCESS",
            overrides.and_then(|o| o.success.as_deref()),
        );
        theme.warn = resolve_color(
            theme.warn,
            "ELAI_THEME_WARN",
            overrides.and_then(|o| o.warn.as_deref()),
        );
        theme.error = resolve_color(
            theme.error,
            "ELAI_THEME_ERROR",
            overrides.and_then(|o| o.error.as_deref()),
        );
        theme.thinking = resolve_color(
            theme.thinking,
            "ELAI_THEME_THINKING",
            overrides.and_then(|o| o.thinking.as_deref()),
        );
        theme.link = resolve_color(
            theme.link,
            "ELAI_THEME_LINK",
            overrides.and_then(|o| o.link.as_deref()),
        );
        theme.inline_code = resolve_color(
            theme.inline_code,
            "ELAI_THEME_INLINE_CODE",
            overrides.and_then(|o| o.inline_code.as_deref()),
        );
        theme.diff_context = resolve_color(
            theme.diff_context,
            "ELAI_THEME_DIFF_CONTEXT",
            overrides.and_then(|o| o.diff_context.as_deref()),
        );
        theme.easter_egg.body = resolve_color(
            theme.easter_egg.body,
            "ELAI_THEME_EASTER_EGG_BODY",
            overrides.and_then(|o| o.easter_egg_body.as_deref()),
        );
        theme.easter_egg.warm = resolve_color(
            theme.easter_egg.warm,
            "ELAI_THEME_EASTER_EGG_WARM",
            overrides.and_then(|o| o.easter_egg_warm.as_deref()),
        );
        theme.easter_egg.dark = resolve_color(
            theme.easter_egg.dark,
            "ELAI_THEME_EASTER_EGG_DARK",
            overrides.and_then(|o| o.easter_egg_dark.as_deref()),
        );

        theme
    }

    /// Converte este tema para a representação `ratatui::style::Color` consumida
    /// pela TUI. Wrapper read-only — a fonte de verdade continua sendo o
    /// `ColorTheme` em `crossterm::Color`. Ver [`crossterm_to_ratatui`] para
    /// a tabela de equivalência ANSI entre as duas crates.
    #[must_use]
    pub fn for_tui(&self) -> RatatuiTheme {
        RatatuiTheme {
            primary_accent: crossterm_to_ratatui(self.primary_accent),
            accent_on_primary_bg: crossterm_to_ratatui(self.accent_on_primary_bg),
            text_primary: crossterm_to_ratatui(self.text_primary),
            text_secondary: crossterm_to_ratatui(self.text_secondary),
            border_active: crossterm_to_ratatui(self.border_active),
            border_inactive: crossterm_to_ratatui(self.border_inactive),
            border_table: crossterm_to_ratatui(self.border_table),
            border_code_block: crossterm_to_ratatui(self.border_code_block),
            info: crossterm_to_ratatui(self.info),
            success: crossterm_to_ratatui(self.success),
            warn: crossterm_to_ratatui(self.warn),
            error: crossterm_to_ratatui(self.error),
            thinking: crossterm_to_ratatui(self.thinking),
            link: crossterm_to_ratatui(self.link),
            inline_code: crossterm_to_ratatui(self.inline_code),
            diff_context: crossterm_to_ratatui(self.diff_context),
            easter_egg: RatatuiEasterEggTheme {
                body: crossterm_to_ratatui(self.easter_egg.body),
                warm: crossterm_to_ratatui(self.easter_egg.warm),
                dark: crossterm_to_ratatui(self.easter_egg.dark),
            },
        }
    }
}

/// View do [`ColorTheme`] em `ratatui::style::Color`, gerada por [`ColorTheme::for_tui`].
///
/// Existe porque `ratatui` e `crossterm` usam enums diferentes para representar
/// cores ANSI e divergem na convenção de brilho (ver [`crossterm_to_ratatui`]).
/// Esta struct é apenas uma projeção do tema único — não tenha um `Default` próprio
/// nem mexa nesses valores diretamente; sempre derive de `ColorTheme`.
///
/// **Espelhamento**: mantém todos os campos de [`ColorTheme`] mesmo quando a TUI
/// ratatui não usa alguns hoje (`border_table`, `border_code_block` são exclusivos
/// do renderer markdown CLI). Mantemos espelho 1-para-1 para que adicionar um
/// novo widget na TUI nunca exija expandir tipos — o token já existe.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct RatatuiTheme {
    pub primary_accent: ratatui::style::Color,
    pub accent_on_primary_bg: ratatui::style::Color,
    pub text_primary: ratatui::style::Color,
    pub text_secondary: ratatui::style::Color,
    pub border_active: ratatui::style::Color,
    pub border_inactive: ratatui::style::Color,
    pub border_table: ratatui::style::Color,
    pub border_code_block: ratatui::style::Color,
    pub info: ratatui::style::Color,
    pub success: ratatui::style::Color,
    pub warn: ratatui::style::Color,
    pub error: ratatui::style::Color,
    pub thinking: ratatui::style::Color,
    pub link: ratatui::style::Color,
    pub inline_code: ratatui::style::Color,
    pub diff_context: ratatui::style::Color,
    pub easter_egg: RatatuiEasterEggTheme,
}

#[derive(Debug, Clone, Copy)]
pub struct RatatuiEasterEggTheme {
    pub body: ratatui::style::Color,
    pub warm: ratatui::style::Color,
    pub dark: ratatui::style::Color,
}

fn resolve_color(default: Color, env_key: &str, config_value: Option<&str>) -> Color {
    if let Some(raw) = std::env::var_os(env_key).and_then(|raw| raw.into_string().ok()) {
        return parse_color(&raw).unwrap_or(default);
    }
    if let Some(raw) = config_value {
        return parse_color(raw).unwrap_or(default);
    }
    default
}

fn resolve_text_secondary(
    default: Color,
    env_color_key: &str,
    config_color_value: Option<&str>,
    env_intensity_key: &str,
    config_intensity_value: Option<u8>,
) -> Color {
    if let Some(raw) = std::env::var_os(env_color_key).and_then(|raw| raw.into_string().ok()) {
        return parse_color(&raw).unwrap_or(default);
    }
    if let Some(raw) = std::env::var_os(env_intensity_key).and_then(|raw| raw.into_string().ok()) {
        return parse_text_secondary_intensity(&raw).unwrap_or(default);
    }
    if let Some(raw) = config_color_value {
        return parse_color(raw).unwrap_or(default);
    }
    if let Some(intensity) = config_intensity_value {
        return validated_text_secondary_intensity(intensity).unwrap_or(default);
    }
    default
}

fn parse_text_secondary_intensity(raw: &str) -> Option<Color> {
    raw.parse::<u8>()
        .ok()
        .and_then(validated_text_secondary_intensity)
}

fn validated_text_secondary_intensity(value: u8) -> Option<Color> {
    if (SECONDARY_INTENSITY_MIN..=SECONDARY_INTENSITY_MAX).contains(&value) {
        Some(Color::AnsiValue(value))
    } else {
        None
    }
}

#[must_use]
pub fn parse_color(raw: &str) -> Option<Color> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(hex) = trimmed.strip_prefix('#') {
        if hex.len() == 6 {
            let parsed = u32::from_str_radix(hex, 16).ok()?;
            let r = ((parsed >> 16) & 0xff) as u8;
            let g = ((parsed >> 8) & 0xff) as u8;
            let b = (parsed & 0xff) as u8;
            return Some(Color::Rgb { r, g, b });
        }
        return None;
    }

    if let Ok(index) = trimmed.parse::<u8>() {
        return Some(Color::AnsiValue(index));
    }

    let normalized = trimmed.to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "reset" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "dark_red" => Some(Color::DarkRed),
        "dark_green" => Some(Color::DarkGreen),
        "dark_yellow" => Some(Color::DarkYellow),
        "dark_blue" => Some(Color::DarkBlue),
        "dark_magenta" => Some(Color::DarkMagenta),
        "dark_cyan" => Some(Color::DarkCyan),
        "grey" | "gray" => Some(Color::Grey),
        "dark_grey" | "dark_gray" => Some(Color::DarkGrey),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        _ => None,
    }
}

/// Converte `crossterm::style::Color` em `ratatui::style::Color` mantendo o
/// índice ANSI (e portanto o brilho real no terminal).
///
/// **Atenção à divergência de nomes** entre as duas crates:
///
/// | ANSI | crossterm    | ratatui     |
/// |-----:|:-------------|:------------|
/// |    1 | `DarkRed`    | `Red`       |
/// |    2 | `DarkGreen`  | `Green`     |
/// |    3 | `DarkYellow` | `Yellow`    |
/// |    4 | `DarkBlue`   | `Blue`      |
/// |    5 | `DarkMagenta`| `Magenta`   |
/// |    6 | `DarkCyan`   | `Cyan`      |
/// |    7 | `Grey`       | `Gray`      |
/// |    8 | `DarkGrey`   | `DarkGray`  |
/// |    9 | `Red`        | `LightRed`  |
/// |   10 | `Green`      | `LightGreen`|
/// |   11 | `Yellow`     | `LightYellow`|
/// |   12 | `Blue`       | `LightBlue` |
/// |   13 | `Magenta`    | `LightMagenta`|
/// |   14 | `Cyan`       | `LightCyan` |
/// |   15 | `White`      | `White`     |
///
/// `crossterm` segue convenção britânica e `Red` sem prefixo é o **brilhante**
/// (ANSI 9). `ratatui` segue convenção americana e `Red` sem prefixo é o
/// **escuro** (ANSI 1). O mapeamento abaixo respeita o índice, não o nome.
#[must_use]
pub fn crossterm_to_ratatui(color: Color) -> ratatui::style::Color {
    use ratatui::style::Color as Rt;
    match color {
        Color::Reset => Rt::Reset,
        Color::Black => Rt::Black,
        Color::DarkRed => Rt::Red,
        Color::DarkGreen => Rt::Green,
        Color::DarkYellow => Rt::Yellow,
        Color::DarkBlue => Rt::Blue,
        Color::DarkMagenta => Rt::Magenta,
        Color::DarkCyan => Rt::Cyan,
        Color::Grey => Rt::Gray,
        Color::DarkGrey => Rt::Gray,
        Color::Red => Rt::LightRed,
        Color::Green => Rt::LightGreen,
        Color::Yellow => Rt::LightYellow,
        Color::Blue => Rt::LightBlue,
        Color::Magenta => Rt::LightMagenta,
        Color::Cyan => Rt::LightCyan,
        Color::White => Rt::White,
        Color::AnsiValue(n) => Rt::Indexed(n),
        Color::Rgb { r, g, b } => Rt::Rgb(r, g, b),
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Spinner {
    frame_index: usize,
}

impl Spinner {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tick(
        &mut self,
        label: &str,
        theme: &ColorTheme,
        out: &mut impl Write,
    ) -> io::Result<()> {
        let frame = Self::FRAMES[self.frame_index % Self::FRAMES.len()];
        self.frame_index += 1;
        queue!(
            out,
            SavePosition,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(theme.thinking),
            Print(format!("{frame} {label}")),
            ResetColor,
            RestorePosition
        )?;
        out.flush()
    }

    pub fn finish(
        &mut self,
        label: &str,
        theme: &ColorTheme,
        out: &mut impl Write,
    ) -> io::Result<()> {
        self.frame_index = 0;
        execute!(
            out,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(theme.success),
            Print(format!("✔ {label}\n")),
            ResetColor
        )?;
        out.flush()
    }

    pub fn fail(
        &mut self,
        label: &str,
        theme: &ColorTheme,
        out: &mut impl Write,
    ) -> io::Result<()> {
        self.frame_index = 0;
        execute!(
            out,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(theme.error),
            Print(format!("✘ {label}\n")),
            ResetColor
        )?;
        out.flush()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ListKind {
    Unordered,
    Ordered { next_index: u64 },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct TableState {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    in_head: bool,
}

impl TableState {
    fn push_cell(&mut self) {
        let cell = self.current_cell.trim().to_string();
        self.current_row.push(cell);
        self.current_cell.clear();
    }

    fn finish_row(&mut self) {
        if self.current_row.is_empty() {
            return;
        }
        let row = std::mem::take(&mut self.current_row);
        if self.in_head {
            self.headers = row;
        } else {
            self.rows.push(row);
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct RenderState {
    emphasis: usize,
    strong: usize,
    heading_level: Option<u8>,
    quote: usize,
    list_stack: Vec<ListKind>,
    link_stack: Vec<LinkState>,
    table: Option<TableState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LinkState {
    destination: String,
    text: String,
}

impl RenderState {
    fn style_text(&self, text: &str, theme: &ColorTheme) -> String {
        let mut style = text.stylize();

        if matches!(self.heading_level, Some(1 | 2)) || self.strong > 0 {
            style = style.bold();
        }
        if self.emphasis > 0 {
            style = style.italic();
        }

        if let Some(level) = self.heading_level {
            style = match level {
                1 => style.with(theme.info),
                2 => style.with(theme.text_primary),
                3 => style.with(theme.link),
                _ => style.with(theme.text_secondary),
            };
        } else if self.strong > 0 {
            style = style.with(theme.warn);
        } else if self.emphasis > 0 {
            style = style.with(theme.diff_context);
        }

        if self.quote > 0 {
            style = style.with(theme.text_secondary);
        }

        format!("{style}")
    }

    fn append_raw(&mut self, output: &mut String, text: &str) {
        if let Some(link) = self.link_stack.last_mut() {
            link.text.push_str(text);
        } else if let Some(table) = self.table.as_mut() {
            table.current_cell.push_str(text);
        } else {
            output.push_str(text);
        }
    }

    fn append_styled(&mut self, output: &mut String, text: &str, theme: &ColorTheme) {
        let styled = self.style_text(text, theme);
        self.append_raw(output, &styled);
    }
}

#[derive(Debug)]
pub struct TerminalRenderer {
    syntax_set: SyntaxSet,
    syntax_theme: Theme,
    color_theme: ColorTheme,
}

impl Default for TerminalRenderer {
    fn default() -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let syntax_theme = ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .unwrap_or_default();
        Self {
            syntax_set,
            syntax_theme,
            color_theme: ColorTheme::resolved(),
        }
    }
}

impl TerminalRenderer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn color_theme(&self) -> &ColorTheme {
        &self.color_theme
    }

    #[must_use]
    pub fn render_markdown(&self, markdown: &str) -> String {
        let mut output = String::new();
        let mut state = RenderState::default();
        let mut code_language = String::new();
        let mut code_buffer = String::new();
        let mut in_code_block = false;

        for event in Parser::new_ext(markdown, Options::all()) {
            self.render_event(
                event,
                &mut state,
                &mut output,
                &mut code_buffer,
                &mut code_language,
                &mut in_code_block,
            );
        }

        output.trim_end().to_string()
    }

    #[must_use]
    pub fn markdown_to_ansi(&self, markdown: &str) -> String {
        self.render_markdown(markdown)
    }

    #[allow(clippy::too_many_lines)]
    fn render_event(
        &self,
        event: Event<'_>,
        state: &mut RenderState,
        output: &mut String,
        code_buffer: &mut String,
        code_language: &mut String,
        in_code_block: &mut bool,
    ) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                self.start_heading(state, level as u8, output);
            }
            Event::End(TagEnd::Paragraph) => output.push_str("\n\n"),
            Event::Start(Tag::BlockQuote(..)) => self.start_quote(state, output),
            Event::End(TagEnd::BlockQuote(..)) => {
                state.quote = state.quote.saturating_sub(1);
                output.push('\n');
            }
            Event::End(TagEnd::Heading(..)) => {
                state.heading_level = None;
                output.push_str("\n\n");
            }
            Event::End(TagEnd::Item) | Event::SoftBreak | Event::HardBreak => {
                state.append_raw(output, "\n");
            }
            Event::Start(Tag::List(first_item)) => {
                let kind = match first_item {
                    Some(index) => ListKind::Ordered { next_index: index },
                    None => ListKind::Unordered,
                };
                state.list_stack.push(kind);
            }
            Event::End(TagEnd::List(..)) => {
                state.list_stack.pop();
                output.push('\n');
            }
            Event::Start(Tag::Item) => Self::start_item(state, output),
            Event::Start(Tag::CodeBlock(kind)) => {
                *in_code_block = true;
                *code_language = match kind {
                    CodeBlockKind::Indented => String::from("text"),
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                };
                code_buffer.clear();
                self.start_code_block(code_language, output);
            }
            Event::End(TagEnd::CodeBlock) => {
                self.finish_code_block(code_buffer, code_language, output);
                *in_code_block = false;
                code_language.clear();
                code_buffer.clear();
            }
            Event::Start(Tag::Emphasis) => state.emphasis += 1,
            Event::End(TagEnd::Emphasis) => state.emphasis = state.emphasis.saturating_sub(1),
            Event::Start(Tag::Strong) => state.strong += 1,
            Event::End(TagEnd::Strong) => state.strong = state.strong.saturating_sub(1),
            Event::Code(code) => {
                let rendered =
                    format!("{}", format!("`{code}`").with(self.color_theme.inline_code));
                state.append_raw(output, &rendered);
            }
            Event::Rule => output.push_str("---\n"),
            Event::Text(text) => {
                self.push_text(text.as_ref(), state, output, code_buffer, *in_code_block);
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                state.append_raw(output, &html);
            }
            Event::FootnoteReference(reference) => {
                state.append_raw(output, &format!("[{reference}]"));
            }
            Event::TaskListMarker(done) => {
                state.append_raw(output, if done { "[x] " } else { "[ ] " });
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                state.append_raw(output, &math);
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                state.link_stack.push(LinkState {
                    destination: dest_url.to_string(),
                    text: String::new(),
                });
            }
            Event::End(TagEnd::Link) => {
                if let Some(link) = state.link_stack.pop() {
                    let label = if link.text.is_empty() {
                        link.destination.clone()
                    } else {
                        link.text
                    };
                    let rendered = format!(
                        "{}",
                        format!("[{label}]({})", link.destination)
                            .underlined()
                            .with(self.color_theme.link)
                    );
                    state.append_raw(output, &rendered);
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let rendered = format!(
                    "{}",
                    format!("[image:{dest_url}]").with(self.color_theme.link)
                );
                state.append_raw(output, &rendered);
            }
            Event::Start(Tag::Table(..)) => state.table = Some(TableState::default()),
            Event::End(TagEnd::Table) => {
                if let Some(table) = state.table.take() {
                    output.push_str(&self.render_table(&table));
                    output.push_str("\n\n");
                }
            }
            Event::Start(Tag::TableHead) => {
                if let Some(table) = state.table.as_mut() {
                    table.in_head = true;
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(table) = state.table.as_mut() {
                    table.finish_row();
                    table.in_head = false;
                }
            }
            Event::Start(Tag::TableRow) => {
                if let Some(table) = state.table.as_mut() {
                    table.current_row.clear();
                    table.current_cell.clear();
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(table) = state.table.as_mut() {
                    table.finish_row();
                }
            }
            Event::Start(Tag::TableCell) => {
                if let Some(table) = state.table.as_mut() {
                    table.current_cell.clear();
                }
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(table) = state.table.as_mut() {
                    table.push_cell();
                }
            }
            Event::Start(Tag::Paragraph | Tag::MetadataBlock(..) | _)
            | Event::End(TagEnd::Image | TagEnd::MetadataBlock(..) | _) => {}
        }
    }

    #[allow(clippy::unused_self)]
    fn start_heading(&self, state: &mut RenderState, level: u8, output: &mut String) {
        state.heading_level = Some(level);
        if !output.is_empty() {
            output.push('\n');
        }
    }

    fn start_quote(&self, state: &mut RenderState, output: &mut String) {
        state.quote += 1;
        let _ = write!(output, "{}", "│ ".with(self.color_theme.text_secondary));
    }

    fn start_item(state: &mut RenderState, output: &mut String) {
        let depth = state.list_stack.len().saturating_sub(1);
        output.push_str(&"  ".repeat(depth));

        let marker = match state.list_stack.last_mut() {
            Some(ListKind::Ordered { next_index }) => {
                let value = *next_index;
                *next_index += 1;
                format!("{value}. ")
            }
            _ => "• ".to_string(),
        };
        output.push_str(&marker);
    }

    fn start_code_block(&self, code_language: &str, output: &mut String) {
        let label = if code_language.is_empty() {
            "code".to_string()
        } else {
            code_language.to_string()
        };
        let _ = writeln!(
            output,
            "{}",
            format!("╭─ {label}")
                .bold()
                .with(self.color_theme.border_code_block)
        );
    }

    fn finish_code_block(&self, code_buffer: &str, code_language: &str, output: &mut String) {
        output.push_str(&self.highlight_code(code_buffer, code_language));
        let _ = write!(
            output,
            "{}",
            "╰─".bold().with(self.color_theme.border_code_block)
        );
        output.push_str("\n\n");
    }

    fn push_text(
        &self,
        text: &str,
        state: &mut RenderState,
        output: &mut String,
        code_buffer: &mut String,
        in_code_block: bool,
    ) {
        if in_code_block {
            code_buffer.push_str(text);
        } else {
            state.append_styled(output, text, &self.color_theme);
        }
    }

    fn render_table(&self, table: &TableState) -> String {
        let mut rows = Vec::new();
        if !table.headers.is_empty() {
            rows.push(table.headers.clone());
        }
        rows.extend(table.rows.iter().cloned());

        if rows.is_empty() {
            return String::new();
        }

        let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
        let widths = (0..column_count)
            .map(|column| {
                rows.iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| visible_width(cell))
                    .max()
                    .unwrap_or(0)
            })
            .collect::<Vec<_>>();

        let border = format!("{}", "│".with(self.color_theme.border_table));
        let separator = widths
            .iter()
            .map(|width| "─".repeat(*width + 2))
            .collect::<Vec<_>>()
            .join(&format!("{}", "┼".with(self.color_theme.border_table)));
        let separator = format!("{border}{separator}{border}");

        let mut output = String::new();
        if !table.headers.is_empty() {
            output.push_str(&self.render_table_row(&table.headers, &widths, true));
            output.push('\n');
            output.push_str(&separator);
            if !table.rows.is_empty() {
                output.push('\n');
            }
        }

        for (index, row) in table.rows.iter().enumerate() {
            output.push_str(&self.render_table_row(row, &widths, false));
            if index + 1 < table.rows.len() {
                output.push('\n');
            }
        }

        output
    }

    fn render_table_row(&self, row: &[String], widths: &[usize], is_header: bool) -> String {
        let border = format!("{}", "│".with(self.color_theme.border_table));
        let mut line = String::new();
        line.push_str(&border);

        for (index, width) in widths.iter().enumerate() {
            let cell = row.get(index).map_or("", String::as_str);
            line.push(' ');
            if is_header {
                let _ = write!(line, "{}", cell.bold().with(self.color_theme.info));
            } else {
                line.push_str(cell);
            }
            let padding = width.saturating_sub(visible_width(cell));
            line.push_str(&" ".repeat(padding + 1));
            line.push_str(&border);
        }

        line
    }

    #[must_use]
    pub fn highlight_code(&self, code: &str, language: &str) -> String {
        let syntax = self
            .syntax_set
            .find_syntax_by_token(language)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());
        let mut syntax_highlighter = HighlightLines::new(syntax, &self.syntax_theme);
        let mut colored_output = String::new();

        for line in LinesWithEndings::from(code) {
            match syntax_highlighter.highlight_line(line, &self.syntax_set) {
                Ok(ranges) => {
                    let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
                    colored_output.push_str(&apply_code_block_background(&escaped));
                }
                Err(_) => colored_output.push_str(&apply_code_block_background(line)),
            }
        }

        colored_output
    }

    pub fn stream_markdown(&self, markdown: &str, out: &mut impl Write) -> io::Result<()> {
        let rendered_markdown = self.markdown_to_ansi(markdown);
        write!(out, "{rendered_markdown}")?;
        if !rendered_markdown.ends_with('\n') {
            writeln!(out)?;
        }
        out.flush()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MarkdownStreamState {
    pending: String,
}

impl MarkdownStreamState {
    #[must_use]
    pub fn push(&mut self, renderer: &TerminalRenderer, delta: &str) -> Option<String> {
        self.pending.push_str(delta);
        let split = find_stream_safe_boundary(&self.pending)?;
        let ready = self.pending[..split].to_string();
        self.pending.drain(..split);
        Some(renderer.markdown_to_ansi(&ready))
    }

    #[must_use]
    pub fn flush(&mut self, renderer: &TerminalRenderer) -> Option<String> {
        if self.pending.trim().is_empty() {
            self.pending.clear();
            None
        } else {
            let pending = std::mem::take(&mut self.pending);
            Some(renderer.markdown_to_ansi(&pending))
        }
    }
}

fn apply_code_block_background(line: &str) -> String {
    let trimmed = line.trim_end_matches('\n');
    let trailing_newline = if trimmed.len() == line.len() {
        ""
    } else {
        "\n"
    };
    let with_background = trimmed.replace("\u{1b}[0m", "\u{1b}[0;48;5;236m");
    format!("\u{1b}[48;5;236m{with_background}\u{1b}[0m{trailing_newline}")
}

fn find_stream_safe_boundary(markdown: &str) -> Option<usize> {
    let mut in_fence = false;
    let mut last_boundary = None;

    for (offset, line) in markdown.split_inclusive('\n').scan(0usize, |cursor, line| {
        let start = *cursor;
        *cursor += line.len();
        Some((start, line))
    }) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            if !in_fence {
                last_boundary = Some(offset + line.len());
            }
            continue;
        }

        if in_fence {
            continue;
        }

        if trimmed.is_empty() {
            last_boundary = Some(offset + line.len());
        }
    }

    last_boundary
}

fn visible_width(input: &str) -> usize {
    strip_ansi(input).chars().count()
}

fn strip_ansi(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            output.push(ch);
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::{parse_color, strip_ansi, ColorTheme, MarkdownStreamState, Spinner, TerminalRenderer};
    use crossterm::style::Color;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvRestore {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.prev {
                Some(p) => std::env::set_var(self.key, p),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn with_home(td: &TempDir, f: impl FnOnce()) {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _restore = EnvRestore {
            key: "HOME",
            prev: std::env::var_os("HOME"),
        };
        std::env::set_var("HOME", td.path());
        f();
    }

    #[test]
    fn renders_markdown_with_styling_and_lists() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer
            .render_markdown("# Heading\n\nThis is **bold** and *italic*.\n\n- item\n\n`code`");

        assert!(markdown_output.contains("Heading"));
        assert!(markdown_output.contains("• item"));
        assert!(markdown_output.contains("code"));
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn renders_links_as_colored_markdown_labels() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.render_markdown("See [Elai](https://example.com/docs) now.");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("[Elai](https://example.com/docs)"));
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn highlights_fenced_code_blocks() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.markdown_to_ansi("```rust\nfn hi() { println!(\"hi\"); }\n```");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("╭─ rust"));
        assert!(plain_text.contains("fn hi"));
        assert!(markdown_output.contains('\u{1b}'));
        assert!(markdown_output.contains("[48;5;236m"));
    }

    #[test]
    fn renders_ordered_and_nested_lists() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.render_markdown("1. first\n2. second\n   - nested\n   - child");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("1. first"));
        assert!(plain_text.contains("2. second"));
        assert!(plain_text.contains("  • nested"));
        assert!(plain_text.contains("  • child"));
    }

    #[test]
    fn renders_tables_with_alignment() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer
            .render_markdown("| Name | Value |\n| ---- | ----- |\n| alpha | 1 |\n| beta | 22 |");
        let plain_text = strip_ansi(&markdown_output);
        let lines = plain_text.lines().collect::<Vec<_>>();

        assert_eq!(lines[0], "│ Name  │ Value │");
        assert_eq!(lines[1], "│───────┼───────│");
        assert_eq!(lines[2], "│ alpha │ 1     │");
        assert_eq!(lines[3], "│ beta  │ 22    │");
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn streaming_state_waits_for_complete_blocks() {
        let renderer = TerminalRenderer::new();
        let mut state = MarkdownStreamState::default();

        assert_eq!(state.push(&renderer, "# Heading"), None);
        let flushed = state
            .push(&renderer, "\n\nParagraph\n\n")
            .expect("completed block");
        let plain_text = strip_ansi(&flushed);
        assert!(plain_text.contains("Heading"));
        assert!(plain_text.contains("Paragraph"));

        assert_eq!(state.push(&renderer, "```rust\nfn main() {}\n"), None);
        let code = state
            .push(&renderer, "```\n")
            .expect("closed code fence flushes");
        assert!(strip_ansi(&code).contains("fn main()"));
    }

    #[test]
    fn spinner_advances_frames() {
        let terminal_renderer = TerminalRenderer::new();
        let mut spinner = Spinner::new();
        let mut out = Vec::new();
        spinner
            .tick("Working", terminal_renderer.color_theme(), &mut out)
            .expect("tick succeeds");
        spinner
            .tick("Working", terminal_renderer.color_theme(), &mut out)
            .expect("tick succeeds");

        let output = String::from_utf8_lossy(&out);
        assert!(output.contains("Working"));
    }

    #[test]
    fn parse_color_accepts_named_index_and_hex() {
        assert_eq!(parse_color("dark_blue"), Some(Color::DarkBlue));
        assert_eq!(parse_color("240"), Some(Color::AnsiValue(240)));
        assert_eq!(
            parse_color("#A8A8A8"),
            Some(Color::Rgb {
                r: 0xA8,
                g: 0xA8,
                b: 0xA8,
            })
        );
    }

    #[test]
    fn resolved_theme_prefers_env_over_config() {
        let td = TempDir::new().unwrap();
        with_home(&td, || {
            let mut cfg = runtime::GlobalConfig::default();
            cfg.theme.info = Some("dark_red".to_string());
            cfg.theme.text_secondary_intensity = Some(240);
            runtime::save_global_config(&cfg).unwrap();

            let _restore_info = EnvRestore {
                key: "ELAI_THEME_INFO",
                prev: std::env::var_os("ELAI_THEME_INFO"),
            };
            let _restore_gray = EnvRestore {
                key: "ELAI_TEXT_SECONDARY_INTENSITY",
                prev: std::env::var_os("ELAI_TEXT_SECONDARY_INTENSITY"),
            };
            std::env::set_var("ELAI_THEME_INFO", "dark_green");
            std::env::set_var("ELAI_TEXT_SECONDARY_INTENSITY", "244");

            let resolved = ColorTheme::resolved();
            assert_eq!(resolved.info, Color::DarkGreen);
            assert_eq!(resolved.text_secondary, Color::AnsiValue(244));
        });
    }

    #[test]
    fn invalid_gray_intensity_falls_back_to_default_244() {
        let td = TempDir::new().unwrap();
        with_home(&td, || {
            let mut cfg = runtime::GlobalConfig::default();
            cfg.theme.text_secondary_intensity = Some(200);
            runtime::save_global_config(&cfg).unwrap();

            let _restore_gray = EnvRestore {
                key: "ELAI_TEXT_SECONDARY_INTENSITY",
                prev: std::env::var_os("ELAI_TEXT_SECONDARY_INTENSITY"),
            };
            std::env::set_var("ELAI_TEXT_SECONDARY_INTENSITY", "200");

            let resolved = ColorTheme::resolved();
            assert_eq!(resolved.text_secondary, Color::AnsiValue(244));
        });
    }
}
