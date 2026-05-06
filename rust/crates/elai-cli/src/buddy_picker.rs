//! Pokémon mascot selection screen — grid layout with scroll.
//!
//! Uses `ratatui` for the grid (multiple sprite cards visible at once) and
//! `ansi-to-tui` to parse the embedded ANSI 256-color sprites into colored
//! `Line`/`Span`s that ratatui can paint.

use std::io;

use ansi_to_tui::IntoText;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph},
    Terminal,
};
use runtime::buddy::{
    pokemon_name, roll_bones_for, sprite_for_id, PokemonId, Rarity, StatName, POKEMON_COUNT,
};

const CARD_WIDTH: u16 = 30;
const CARD_HEIGHT: u16 = 16;
const STATS_PANEL_WIDTH: u16 = 28;

/// Runs the interactive picker. Returns the chosen Pokédex id, or `None` if the
/// user cancelled with `Esc` / `q`.
pub fn run_buddy_picker(initial: Option<PokemonId>) -> io::Result<Option<PokemonId>> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Pré-parseia todos os sprites uma única vez (151 sprites, ~150 KB total).
    let cards: Vec<Text<'static>> = (1..=POKEMON_COUNT)
        .map(|id| {
            sprite_for_id(id)
                .into_text()
                .unwrap_or_else(|_| Text::from(format!("#{id:03}")))
        })
        .collect();

    let mut current: PokemonId = initial.unwrap_or(1).clamp(1, POKEMON_COUNT);
    let mut scroll_row: usize = 0;
    // Bones (stats/rarity/shiny) são recalculados por mascote em foco —
    // determinístico para `(user_id, pokemon_id)`, então cada um tem valores
    // próprios mas estáveis entre runs.
    let user_id = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_string());

    let result = loop {
        let mut cols_per_row: usize = 1;
        let mut visible_rows: usize = 1;

        terminal.draw(|f| {
            let size = f.area();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(2), // header
                    Constraint::Min(CARD_HEIGHT),
                    Constraint::Length(1), // footer
                ])
                .split(size);

            // Header
            let header = Line::from(vec![
                Span::styled(" Escolha seu mascote ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw("   "),
                Span::styled(
                    format!("#{:03} {}  ({}/{})", current, pokemon_name(current), current, POKEMON_COUNT),
                    Style::default().fg(Color::White),
                ),
            ]);
            f.render_widget(Paragraph::new(header), chunks[0]);

            // Reserva painel lateral de stats à direita do grid (se largura permitir).
            let body_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Min(CARD_WIDTH),
                    Constraint::Length(if chunks[1].width > CARD_WIDTH + STATS_PANEL_WIDTH {
                        STATS_PANEL_WIDTH
                    } else {
                        0
                    }),
                ])
                .split(chunks[1]);
            let grid_area = body_cols[0];
            let stats_area = body_cols[1];

            if stats_area.width > 0 {
                let focused = roll_bones_for(&user_id, current);
                draw_stats_panel(
                    f,
                    stats_area,
                    current,
                    &focused.stats,
                    focused.rarity,
                    focused.shiny,
                );
            }
            cols_per_row = ((grid_area.width / CARD_WIDTH).max(1)) as usize;
            visible_rows = ((grid_area.height / CARD_HEIGHT).max(1)) as usize;
            let total_rows = (POKEMON_COUNT as usize).div_ceil(cols_per_row);

            // Ajusta scroll para manter o selecionado visível.
            let cur_idx = (current - 1) as usize;
            let cur_row = cur_idx / cols_per_row;
            if cur_row < scroll_row {
                scroll_row = cur_row;
            } else if cur_row >= scroll_row + visible_rows {
                scroll_row = cur_row + 1 - visible_rows;
            }
            scroll_row = scroll_row.min(total_rows.saturating_sub(visible_rows));

            for row in 0..visible_rows {
                let row_idx = scroll_row + row;
                if row_idx >= total_rows {
                    break;
                }
                let row_y = grid_area.y + (row as u16) * CARD_HEIGHT;
                if row_y + CARD_HEIGHT > grid_area.y + grid_area.height {
                    break;
                }
                for col in 0..cols_per_row {
                    let card_idx = row_idx * cols_per_row + col;
                    if card_idx >= POKEMON_COUNT as usize {
                        break;
                    }
                    let card_x = grid_area.x + (col as u16) * CARD_WIDTH;
                    if card_x + CARD_WIDTH > grid_area.x + grid_area.width {
                        break;
                    }
                    let card_area = Rect::new(card_x, row_y, CARD_WIDTH, CARD_HEIGHT);
                    let pokemon_id = (card_idx + 1) as PokemonId;
                    let is_selected = pokemon_id == current;

                    let border_style = if is_selected {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let title_style = if is_selected {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    let block = Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(border_style)
                        .title(Span::styled(
                            format!(" #{:03} {} ", pokemon_id, pokemon_name(pokemon_id)),
                            title_style,
                        ));
                    let inner = block.inner(card_area);
                    f.render_widget(block, card_area);
                    let sprite = cards[card_idx].clone();
                    f.render_widget(Paragraph::new(sprite).alignment(Alignment::Center), inner);
                }
            }

            // Footer
            let footer = Line::from(vec![Span::styled(
                "  ←/→ ↑/↓ navegar    Enter confirmar    PgUp/PgDn ±página    Home/End  início/fim    Esc cancelar",
                Style::default().fg(Color::DarkGray),
            )]);
            f.render_widget(Paragraph::new(footer), chunks[2]);
        })?;

        // Input
        if let Event::Key(KeyEvent {
            code,
            modifiers,
            kind,
            ..
        }) = event::read()?
        {
            if kind != KeyEventKind::Press {
                continue;
            }
            let cols = cols_per_row.max(1) as i32;
            let page = (visible_rows.max(1) as i32) * cols;
            match (code, modifiers) {
                (KeyCode::Esc | KeyCode::Char('q'), _) => break Ok(None),
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => break Ok(None),
                (KeyCode::Enter, _) => break Ok(Some(current)),
                (KeyCode::Right | KeyCode::Char('l'), _) => current = step(current, 1),
                (KeyCode::Left | KeyCode::Char('h'), _) => current = step(current, -1),
                (KeyCode::Down | KeyCode::Char('j'), _) => current = step(current, cols),
                (KeyCode::Up | KeyCode::Char('k'), _) => current = step(current, -cols),
                (KeyCode::PageDown, _) => current = step(current, page),
                (KeyCode::PageUp, _) => current = step(current, -page),
                (KeyCode::Home, _) => current = 1,
                (KeyCode::End, _) => current = POKEMON_COUNT,
                _ => {}
            }
        }
    };

    let mut stdout = io::stdout();
    execute!(stdout, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    result
}

fn draw_stats_panel(
    f: &mut ratatui::Frame,
    area: Rect,
    pokemon_id: PokemonId,
    stats: &std::collections::HashMap<StatName, u8>,
    rarity: Rarity,
    shiny: bool,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " Stats ",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Cabeçalho com nome + rarity
    let shiny_mark = if shiny { "✨ " } else { "" };
    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{shiny_mark}#{pokemon_id:03} {}", pokemon_name(pokemon_id)),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{} {}", rarity.as_str(), rarity.stars()),
            Style::default().fg(rarity_color(rarity)),
        ),
    ]));
    lines.push(Line::from(Span::raw("")));

    // Barras de stats
    let bar_width = (inner.width.saturating_sub(11) as usize).clamp(4, 14);
    let stat_rows: [(&str, StatName, Color); 5] = [
        ("HP ", StatName::Patience, Color::LightGreen),
        ("ATK", StatName::Chaos, Color::LightRed),
        ("DEF", StatName::Wisdom, Color::LightBlue),
        ("DBG", StatName::Debugging, Color::LightYellow),
        ("SNK", StatName::Snark, Color::LightMagenta),
    ];
    for (label, stat, color) in stat_rows {
        let value = stats.get(&stat).copied().unwrap_or(0);
        lines.push(stat_bar(label, value, bar_width, color));
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

fn stat_bar(label: &str, value: u8, width: usize, color: Color) -> Line<'static> {
    let v = value.min(100) as usize;
    let filled = (v * width) / 100;
    let empty = width.saturating_sub(filled);
    Line::from(vec![
        Span::raw(" "),
        Span::styled(format!("{label:<4}"), Style::default().fg(Color::Gray)),
        Span::styled("█".repeat(filled), Style::default().fg(color)),
        Span::styled("░".repeat(empty), Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!(" {value:>3}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn rarity_color(r: Rarity) -> Color {
    match r {
        Rarity::Common => Color::Gray,
        Rarity::Uncommon => Color::Green,
        Rarity::Rare => Color::Cyan,
        Rarity::Epic => Color::Magenta,
        Rarity::Legendary => Color::Yellow,
    }
}

fn step(current: PokemonId, delta: i32) -> PokemonId {
    let total = i32::from(POKEMON_COUNT);
    let mut next = i32::from(current) + delta;
    if next < 1 {
        next = 1;
    }
    if next > total {
        next = total;
    }
    next as PokemonId
}
