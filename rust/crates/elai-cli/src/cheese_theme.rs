use ratatui_cheese::theme::Palette;

use crate::render::RatatuiTheme;

#[must_use]
pub fn palette_from_ratatui(theme: &RatatuiTheme) -> Palette {
    let mut palette = Palette::dark();
    palette.foreground = theme.text_primary;
    palette.muted = theme.text_secondary;
    palette.faint = theme.border_inactive;
    palette.primary = theme.primary_accent;
    palette.secondary = theme.info;
    palette.surface = theme.border_inactive;
    palette.border = theme.border_active;
    palette.highlight = theme.primary_accent;
    palette.on_highlight = theme.accent_on_primary_bg;
    palette.error = theme.error;
    palette.success = theme.success;
    palette
}
