//! Tela de seleção dos 3 mascotes iniciais (starters).
//!
//! Mostra os 151 mascotes disponíveis e permite ao usuário selecionar 3.
//! Os starters são desbloqueados imediatamente na coleção.

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
    collection::{load_or_create_collection, save_collection},
    pokemon_name, sprite_for_id, PokemonId, POKEMON_COUNT,
};

const CARD_WIDTH: u16 = 24;
const CARD_HEIGHT: u16 = 14;

const STARTER_COUNT: usize = 3;

/// Executa a seleção de starters. Retorna os 3 IDs escolhidos ou `None` se cancelado.
pub fn run_starter_picker() -> io::Result<Option<[PokemonId; STARTER_COUNT]>> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Pré-parseia todos os sprites
    let sprites: Vec<Text<'static>> = (1..=POKEMON_COUNT)
        .map(|id| {
            sprite_for_id(id)
                .into_text()
                .unwrap_or_else(|_| Text::from(format!("#{id:03}")))
        })
        .collect();

    let mut selected: Vec<PokemonId> = Vec::with_capacity(STARTER_COUNT);
    let mut current: PokemonId = 1;
    let mut scroll_row: usize = 0;

    let result = loop {
        let mut should_quit = false;
        terminal.draw(|f| {
            let size = f.area();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // header
                    Constraint::Min(CARD_HEIGHT),
                    Constraint::Length(3), // footer
                ])
                .split(size);

            // Header
            let selected_display = if selected.is_empty() {
                "Nenhum selecionado".to_string()
            } else {
                selected
                    .iter()
                    .map(|&id| format!("#{id:03} {}", pokemon_name(id)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };

            let header = Line::from(vec![
                Span::styled(" 🐾 Escolha seus 3 iniciais ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw("   "),
                Span::styled(format!("[{}/{}]", selected.len(), STARTER_COUNT), Style::default().fg(Color::Cyan)),
            ]);
            f.render_widget(Paragraph::new(header), chunks[0]);

            // Subheader com selecionados
            let subheader = Line::from(vec![
                Span::styled("Selecionados: ", Style::default().fg(Color::Gray)),
                Span::styled(&selected_display, Style::default().fg(Color::Green)),
            ]);
            f.render_widget(Paragraph::new(subheader), Rect::new(chunks[0].x, chunks[0].y + 1, chunks[0].width, 1));

            // Grid
            let grid_area = chunks[1];
            let cols_per_row = ((grid_area.width / CARD_WIDTH).max(1)) as usize;
            let visible_rows = ((grid_area.height / CARD_HEIGHT).max(1)) as usize;
            let total_rows = (POKEMON_COUNT as usize).div_ceil(cols_per_row);

            // Ajusta scroll
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
                    let is_selected = selected.contains(&pokemon_id);
                    let is_focused = pokemon_id == current;

                    draw_starter_card(f, card_area, &sprites[card_idx], pokemon_id, is_selected, is_focused);
                }
            }

            // Footer
            let footer_text = if selected.len() == STARTER_COUNT {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled("↵ Enter", Style::default().fg(Color::Yellow).add_modifier(Modifier::Bold)),
                    Span::raw(" confirmar   "),
                    Span::styled("Esc", Style::default().fg(Color::Red)),
                    Span::raw(" cancelar   "),
                    Span::styled("Espaço", Style::default().fg(Color::Green)),
                    Span::raw(" selecionar   "),
                    Span::styled("↑↓←→", Style::default().fg(Color::Cyan)),
                    Span::raw(" navegar"),
                ])
            } else {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled("Espaço", Style::default().fg(Color::Green)),
                    Span::raw(" selecionar   "),
                    Span::styled("↑↓←→", Style::default().fg(Color::Cyan)),
                    Span::raw(" navegar   "),
                    Span::styled("Esc", Style::default().fg(Color::Red)),
                    Span::raw(" cancelar"),
                ])
            };
            f.render_widget(Paragraph::new(footer_text), chunks[2]);
        })?;

        // Input
        if let Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) = event::read()? {
            match code {
                KeyCode::Esc => {
                    should_quit = true;
                }
                KeyCode::Enter => {
                    if selected.len() == STARTER_COUNT {
                        let arr = [selected[0], selected[1], selected[2]];
                        break Some(arr);
                    }
                }
                KeyCode::Space => {
                    if selected.contains(&current) {
                        selected.retain(|&id| id != current);
                    } else if selected.len() < STARTER_COUNT {
                        selected.push(current);
                    }
                }
                KeyCode::Up => {
                    if current > 1 {
                        current -= 1;
                    }
                }
                KeyCode::Down => {
                    if current < POKEMON_COUNT {
                        current += 1;
                    }
                }
                KeyCode::Left => {
                    if current > cols_per_row as PokemonId {
                        current -= cols_per_row as PokemonId;
                    } else {
                        current = 1;
                    }
                }
                KeyCode::Right => {
                    if current + cols_per_row as PokemonId <= POKEMON_COUNT {
                        current += cols_per_row as PokemonId;
                    } else {
                        current = POKEMON_COUNT;
                    }
                }
                _ => {}
            }
        }

        if should_quit {
            break None;
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(result)
}

fn draw_starter_card(
    f: &mut ratatui::Frame,
    area: Rect,
    sprite: &Text<'_>,
    pokemon_id: PokemonId,
    is_selected: bool,
    is_focused: bool,
) {
    let inner = Rect::new(area.x + 1, area.y + 1, area.width - 2, area.height - 2);

    // Border
    let border_style = if is_selected {
        Style::default().fg(Color::Green)
    } else if is_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(format!(" #{:03} ", pokemon_id));

    f.render_widget(block, area);

    // Sprite (renderizado apenas se couber)
    if inner.width >= 18 && inner.height >= 10 {
        let sprite_area = Rect::new(inner.x, inner.y, inner.width.min(20), inner.height.saturating_sub(3));
        f.render_widget(Paragraph::new(sprite.clone()), sprite_area);
    }

    // Nome e status
    let name_y = inner.y + inner.height.saturating_sub(3);
    if name_y > inner.y {
        let name = pokemon_name(pokemon_id);
        let style = if is_selected {
            Style::default().fg(Color::Green).add_modifier(Modifier::Bold)
        } else {
            Style::default().fg(Color::White)
        };

        let indicator = if is_selected { "✓ " } else { "  " };
        let spans = vec![
            Span::raw(indicator),
            Span::styled(name, style),
        ];
        f.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(inner.x, name_y, inner.width, 1),
        );
    }
}

/// Finaliza a seleção de starters — salva na coleção e retorna os IDs.
pub fn finalize_starter_selection(starter_ids: [PokemonId; STARTER_COUNT]) -> std::io::Result<()> {
    let mut collection = load_or_create_collection();

    // Os starters são definidos na criação da coleção
    // Se já existe uma coleção, não sobrescrevemos - apenas retornamos sucesso
    if collection.unlocked_count() == 0 {
        use crate::buddy::collection::UserCollection;
        // Nova coleção com starters
        collection = UserCollection::new_with_starters(starter_ids);
    }

    save_collection(&collection)
}
