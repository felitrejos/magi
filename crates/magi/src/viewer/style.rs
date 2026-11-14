use ratatui::style::{Color, Modifier, Style};

use super::model::Level;
use crate::proto::Agent;

pub const TEXT: Color = Color::Rgb(226, 229, 235);
pub const DIM: Color = Color::Rgb(139, 147, 162);
pub const FAINT: Color = Color::Rgb(92, 99, 114);
pub const BORDER: Color = Color::Rgb(78, 86, 104);
pub const TRACK: Color = Color::Rgb(40, 45, 56);
pub const GUIDE: Color = Color::Rgb(58, 64, 78);
pub const SELECTED: Color = Color::Rgb(33, 39, 52);
pub const ACCENT: Color = Color::Rgb(108, 164, 255);
pub const TEAL: Color = Color::Rgb(86, 196, 186);
pub const GOOD: Color = Color::Rgb(110, 204, 140);
pub const WARN: Color = Color::Rgb(238, 180, 76);
pub const WARN_TINT: Color = Color::Rgb(60, 47, 22);
pub const CRIT: Color = Color::Rgb(240, 96, 96);
pub const CLAUDE: Color = Color::Rgb(226, 151, 112);
pub const CODEX: Color = Color::Rgb(178, 194, 216);

pub fn fg(color: Color) -> Style {
    Style::new().fg(color)
}

pub fn bold(color: Color) -> Style {
    fg(color).add_modifier(Modifier::BOLD)
}

pub fn pill(color: Color, tint: Color) -> Style {
    bold(color).bg(tint)
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

pub fn agent(agent: Agent) -> (char, Color) {
    match agent {
        Agent::ClaudeCode => ('✻', CLAUDE),
        Agent::Codex => ('◇', CODEX),
    }
}
