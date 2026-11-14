use ratatui::style::{Color, Modifier, Style};

use super::model::Level;

pub const TEXT: Color = Color::Rgb(220, 223, 230);
pub const DIM: Color = Color::Rgb(128, 134, 148);
pub const FAINT: Color = Color::Rgb(70, 75, 88);
pub const SELECTED: Color = Color::Rgb(32, 36, 46);
pub const ACCENT: Color = Color::Rgb(108, 164, 255);
pub const SEND: Color = Color::Rgb(86, 196, 186);
pub const GOOD: Color = Color::Rgb(110, 204, 140);
pub const WARN: Color = Color::Rgb(238, 180, 76);
pub const CRIT: Color = Color::Rgb(240, 96, 96);

pub fn fg(color: Color) -> Style {
    Style::new().fg(color)
}

pub fn bold(color: Color) -> Style {
    fg(color).add_modifier(Modifier::BOLD)
}

pub fn load(fraction: f32) -> Color {
    if fraction >= 0.85 {
        CRIT
    } else if fraction >= 0.6 {
        WARN
    } else {
        ACCENT
    }
}

pub fn level(level: Level) -> Color {
    match level {
        Level::Normal => ACCENT,
        Level::High => WARN,
        Level::Critical => CRIT,
    }
}
