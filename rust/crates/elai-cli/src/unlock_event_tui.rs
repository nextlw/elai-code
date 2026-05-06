//! Tela de Evento de Desbloqueio — Pausa a tarefa e mostra novo companheiro
//!
//! Esta tela é exibida quando um milestone é atingido, pausa a tarefa atual,
//! mostra o evento de unlock e permite ao usuário confirmar para continuar.

use std::io;

use ansi_to_tui::IntoText;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
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

use crate::buddy::{
    collection::UserCollection,
    pokemon_name, rarity_for_pokemon, sprite_for_id, EvaluationResult, PokemonId, Rarity,
};

/// Resultado do evento de desbloqueio
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnlockEventResult {
    /// Usuário confirmou — companheiro adicionado à coleção
    Confirmed,
    /// Usuário cancelou — companheiro não adicionado
    Cancelled,
    /// Erro occurred
    Error,
}

/// Informações do mascote sorteado
#[derive(Debug, Clone)]
pub struct RevealedMascot {
    pub pokemon_id: PokemonId,
    pub rarity: Rarity,
    pub name: String,
    pub sprite_text: String,
    pub stars: &'static str,
}

/// Executa a tela de evento de desbloqueio
pub fn run_unlock_event(
    rarity: Rarity,
    complexity_score: u8,
    chosen_pokemon: PokemonId,
    evaluation_reasoning: &str,
) -> io::Result<UnlockEventResult> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Pré-parseia sprite do mascote
    let sprite = sprite_for_id(chosen_pokemon)
        .into_text()
        .unwrap_or_else(|_| Text::from(format!("#{chosen_pokemon:03}")));

    let mascot = RevealedMascot {
        pokemon_id: chosen_pokemon,
        rarity,
        name: pokemon_name(chosen_pokemon).to_string(),
        sprite_text: sprite_for_id(chosen_pokemon).to_string(),
        stars: rarity.stars(),
    };

    let result = loop {
        terminal.draw(|f| {
            let size = f.area();
            draw_unlock_screen(f, size, &mascot, complexity_score, evaluation_reasoning);
        })?;

        // Input
        if let Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) = event::read()? {
            match code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    break UnlockEventResult::Confirmed;
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    break UnlockEventResult::Cancelled;
                }
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(result)
}

fn draw_unlock_screen(
    f: &mut ratatui::Frame,
    area: Rect,
    mascot: &RevealedMascot,
    complexity_score: u8,
    reasoning: &str,
) {
    // Layout: header | sprite | info | footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // header dramática
            Constraint::Min(15), // sprite area
            Constraint::Length(8), // info do mascote
            Constraint::Length(4), // footer
        ])
        .split(area);

    // ── HEADER: Evento de Desbloqueio ──
    let header_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::Bold);

    let header_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title_align(Alignment::Center);

    let header_text = vec![
        Line::from(vec![
            Span::styled("╔═══════════════════════════════════════════════════╗", Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("║         🎉  EVENTO DE DESBLOQUEIO  🎉            ║", header_style),
        ]),
        Line::from(vec![
            Span::styled("║                                                   ║", Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("║     Você desbloqueou um novo companheiro!        ║", Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("╚═══════════════════════════════════════════════════╝", Style::default().fg(Color::Yellow)),
        ]),
    ];
    f.render_widget(
        Paragraph::new(header_text).block(header_block),
        chunks[0],
    );

    // ── SPRITE AREA ──
    let sprite_area = chunks[1];
    let inner = Rect::new(
        sprite_area.x + 1,
        sprite_area.y + 1,
        sprite_area.width.saturating_sub(2),
        sprite_area.height.saturating_sub(2),
    );

    // Decorações nas laterais
    let left_decor = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(Color::Cyan));
    let right_decor = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::Cyan));

    // Sprite centralizado
    let sprite_width = (sprite_area.width.min(40)).saturating_sub(4);
    let sprite_x = sprite_area.x + (sprite_area.width - sprite_width) / 2;
    let sprite_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(rarity_border_color(mascot.rarity)))
        .title(format!(" #{:03} ", mascot.pokemon_id))
        .title_style(Style::default().fg(Color::White).add_modifier(Modifier::Bold))
        .title_align(Alignment::Center);

    let sprite_area_inner = Rect::new(
        sprite_x,
        sprite_area.y + 2,
        sprite_width,
        sprite_area.height.saturating_sub(4),
    );

    f.render_widget(sprite_block, sprite_area_inner);
    f.render_widget(Paragraph::new(mascot.sprite_text.clone()), sprite_area_inner);

    // ── INFO DO MASCOTE ──
    let info_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(chunks[2]);

    // Painel esquerdo: raridade e info
    let rarity_style = rarity_style_for(mascot.rarity);
    let info_lines = vec![
        Line::from(vec![
            Span::styled("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━", rarity_style),
        ]),
        Line::from(vec![
            Span::styled(" ✨ ", rarity_style),
            Span::styled(mascot.name.to_uppercase(), rarity_style.add_modifier(Modifier::Bold)),
            Span::raw(" "),
            Span::styled(mascot.stars, rarity_style),
        ]),
        Line::from(vec![
            Span::styled("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━", rarity_style),
        ]),
        Line::from(vec![
            Span::styled(" Raridade: ", Style::default().fg(Color::Gray)),
            Span::styled(mascot.rarity.as_str().to_uppercase(), rarity_style),
        ]),
        Line::from(vec![
            Span::styled(" ID: ", Style::default().fg(Color::Gray)),
            Span::styled(format!("#{:03}", mascot.pokemon_id), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled(" Complexidade: ", Style::default().fg(Color::Gray)),
            Span::styled(format!("{}/10", complexity_score), complexity_color(complexity_score)),
        ]),
    ];

    let left_block = Block::default()
        .borders(Borders::ALL)
        .border_style(rarity_style)
        .title(" 📋 Informações ");
    f.render_widget(
        Paragraph::new(info_lines).block(left_block),
        info_chunks[0],
    );

    // Painel direito: reasoning
    let reasoning_lines = vec![
        Line::from(vec![
            Span::styled("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![
            Span::styled(" 🧠 Avaliação da Tarefa", Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![
            Span::raw(" "),
        ]),
        Line::from(wrap_reasoning(reasoning, info_chunks[1].width.saturating_sub(4))),
    ];

    let right_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" 📊 Detalhes ");
    f.render_widget(
        Paragraph::new(reasoning_lines).block(right_block),
        info_chunks[1],
    );

    // ── FOOTER: Ações ──
    let footer_lines = vec![
        Line::from(vec![
            Span::styled("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("↵ Enter", Style::default().fg(Color::Green).add_modifier(Modifier::Bold)),
            Span::raw(" ou "),
            Span::styled("Y", Style::default().fg(Color::Green).add_modifier(Modifier::Bold)),
            Span::raw(" — Adicionar à coleção  |  "),
            Span::styled("Esc", Style::default().fg(Color::Red).add_modifier(Modifier::Bold)),
            Span::raw(" ou "),
            Span::styled("N", Style::default().fg(Color::Red).add_modifier(Modifier::Bold)),
            Span::raw(" — Recusar"),
        ]),
        Line::from(vec![
            Span::styled("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━", Style::default().fg(Color::DarkGray)),
        ]),
    ];

    f.render_widget(Paragraph::new(footer_lines), chunks[3]);
}

fn rarity_style_for(rarity: Rarity) -> Style {
    match rarity {
        Rarity::Common => Style::default().fg(Color::Gray),
        Rarity::Uncommon => Style::default().fg(Color::Green),
        Rarity::Rare => Style::default().fg(Color::Cyan),
        Rarity::Epic => Style::default().fg(Color::Magenta),
        Rarity::Legendary => Style::default().fg(Color::Yellow),
    }
}

fn rarity_border_color(rarity: Rarity) -> Color {
    match rarity {
        Rarity::Common => Color::DarkGray,
        Rarity::Uncommon => Color::Green,
        Rarity::Rare => Color::Cyan,
        Rarity::Epic => Color::Magenta,
        Rarity::Legendary => Color::Yellow,
    }
}

fn complexity_color(score: u8) -> Style {
    match score {
        1..=3 => Style::default().fg(Color::Green),
        4..=6 => Style::default().fg(Color::Yellow),
        7..=8 => Style::default().fg(Color::Red),
        9..=10 => Style::default().fg(Color::Magenta),
        _ => Style::default().fg(Color::White),
    }
}

fn wrap_reasoning(text: &str, max_width: u16) -> Vec<Span<'_>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in words {
        if current_line.len() + word.len() + 1 > max_width as usize {
            if !current_line.is_empty() {
                lines.push(Span::raw(&current_line));
            }
            current_line = word.to_string();
        } else {
            if !current_line.is_empty() {
                current_line.push(' ');
            }
            current_line.push_str(word);
        }
    }
    if !current_line.is_empty() {
        lines.push(Span::raw(&current_line));
    }

    lines
}

/// Resultado da avaliação de complexidade
#[derive(Debug, Clone)]
pub struct EvaluationResult {
    pub complexity_score: u8,
    pub determined_rarity: Rarity,
    pub bonus_rarity: bool,
    pub reasoning: String,
}
