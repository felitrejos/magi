use ratatui::style::{Color, Modifier, Style};

use super::model::Level;
use crate::proto::Agent;

pub const TEXT: Color = Color::Rgb(226, 226, 226);
pub const DIM: Color = Color::Rgb(140, 140, 140);
pub const FAINT: Color = Color::Rgb(96, 96, 96);
pub const BORDER: Color = Color::Rgb(84, 84, 84);
pub const RULE: Color = Color::Rgb(92, 92, 92);
pub const TRACK: Color = Color::Rgb(46, 46, 48);
pub const GUIDE: Color = Color::Rgb(60, 60, 62);
pub const SELECTED: Color = Color::Rgb(38, 38, 44);
pub const ACCENT: Color = Color::Rgb(165, 214, 167);
pub const LAVENDER: Color = Color::Rgb(199, 159, 215);
pub const WARN: Color = Color::Rgb(255, 215, 95);
pub const WARN_TINT: Color = Color::Rgb(58, 50, 24);
pub const CRIT: Color = Color::Rgb(236, 112, 112);
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
