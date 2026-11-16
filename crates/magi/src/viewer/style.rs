use std::sync::{LazyLock, PoisonError, RwLock};

use ratatui::style::{Color, Modifier, Style};

use super::model::Level;
use crate::proto::Agent;

const CLAUDE: Rgb = Rgb(217, 119, 87);
const DARK_LIFT: f32 = 0.035;
const LIGHT_LIFT: f32 = 0.03;
const RAISE: f32 = 0.045;
const PROSE: f32 = 0.13;
const FALLBACK: Palette = Palette {
    foreground: Rgb(228, 228, 232),
    background: Rgb(12, 12, 14),
    red: Rgb(240, 98, 92),
    green: Rgb(78, 196, 110),
    yellow: Rgb(226, 178, 58),
    blue: Rgb(92, 160, 250),
    magenta: Rgb(186, 140, 250),
};

static CURRENT: LazyLock<RwLock<(Palette, Theme)>> =
    LazyLock::new(|| RwLock::new((FALLBACK, Theme::from(FALLBACK))));

#[derive(Clone, Copy)]
pub struct Theme {
    pub text: Color,
    pub dim: Color,
    pub faint: Color,
    pub border: Color,
    pub rule: Color,
    pub track: Color,
    pub guide: Color,
    pub selected: Color,
    pub panel: Color,
    pub raised: Color,
    pub prose: Color,
    pub accent: Color,
    pub second: Color,
    pub warn: Color,
    pub warn_tint: Color,
    pub crit: Color,
    pub claude: Color,
    pub codex: Color,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rgb(u8, u8, u8);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Palette {
    foreground: Rgb,
    background: Rgb,
    red: Rgb,
    green: Rgb,
    yellow: Rgb,
    blue: Rgb,
    magenta: Rgb,
}

pub fn update(reply: &[u8]) -> bool {
    let mut current = CURRENT.write().unwrap_or_else(PoisonError::into_inner);
    let palette = Palette::parse(reply, current.0);
    if palette == current.0 {
        return false;
    }
    *current = (palette, Theme::from(palette));
    true
}

pub fn palette_env() -> String {
    let palette = CURRENT.read().unwrap_or_else(PoisonError::into_inner).0;
    [
        palette.foreground,
        palette.background,
        palette.red,
        palette.green,
        palette.yellow,
        palette.blue,
        palette.magenta,
    ]
    .iter()
    .map(|Rgb(r, g, b)| format!("{r:02x}{g:02x}{b:02x}"))
    .collect::<Vec<_>>()
    .join(",")
}

pub fn adopt_palette(text: &str) {
    let colors: Vec<Rgb> = text
        .split(',')
        .filter_map(|hex| {
            let channel = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
            (hex.len() == 6).then_some(())?;
            Some(Rgb(channel(0)?, channel(2)?, channel(4)?))
        })
        .collect();
    let [foreground, background, red, green, yellow, blue, magenta] = colors[..] else {
        return;
    };
    let palette = Palette {
        foreground,
        background,
        red,
        green,
        yellow,
        blue,
        magenta,
    };
    *CURRENT.write().unwrap_or_else(PoisonError::into_inner) = (palette, Theme::from(palette));
}

pub fn theme() -> Theme {
    CURRENT.read().unwrap_or_else(PoisonError::into_inner).1
}

impl From<Palette> for Theme {
    fn from(palette: Palette) -> Self {
        let toward_background = |amount: f32| palette.foreground.mix(palette.background, amount);
        Self {
            text: palette.foreground.into(),
            dim: toward_background(0.42),
            faint: toward_background(0.62),
            border: toward_background(0.66),
            rule: toward_background(0.64),
            track: toward_background(0.84),
            guide: toward_background(0.78),
            selected: toward_background(0.88),
            panel: palette.panel(0.0),
            raised: palette.panel(RAISE),
            prose: toward_background(PROSE),
            accent: palette.green.into(),
            second: palette.magenta.into(),
            warn: palette.yellow.into(),
            warn_tint: palette.yellow.mix(palette.background, 0.78),
            crit: palette.red.into(),
            claude: CLAUDE.into(),
            codex: palette.blue.into(),
        }
    }
}

impl Palette {
    fn panel(self, raise: f32) -> Color {
        let Rgb(r, g, b) = self.background;
        let lightness = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
        let lift = if lightness < 128 {
            DARK_LIFT
        } else {
            LIGHT_LIFT
        };
        self.background.mix(self.foreground, lift + raise)
    }

    fn parse(reply: &[u8], base: Self) -> Self {
        let text = String::from_utf8_lossy(reply);
        let mut palette = base;
        for answer in text.split("\x1b]").skip(1) {
            let answer = answer.split(['\x07', '\x1b']).next().unwrap_or_default();
            let Some((slot, value)) = answer.rsplit_once(";rgb:") else {
                continue;
            };
            let Some(color) = Rgb::parse(value) else {
                continue;
            };
            let target = match slot {
                "10" => &mut palette.foreground,
                "11" => &mut palette.background,
                "4;1" => &mut palette.red,
                "4;2" => &mut palette.green,
                "4;3" => &mut palette.yellow,
                "4;4" => &mut palette.blue,
                "4;5" => &mut palette.magenta,
                _ => continue,
            };
            *target = color;
        }
        palette
    }
}

impl Rgb {
    fn parse(value: &str) -> Option<Self> {
        let mut channels = value.split('/').map(|hex| {
            let max = 16u32.checked_pow(u32::try_from(hex.len()).ok()?)? - 1;
            let level = u32::from_str_radix(hex, 16).ok()?;
            (hex.len() <= 4).then(|| (level * 255 + max / 2) / max)
        });
        let mut next = || channels.next().flatten().map(|level| level as u8);
        let color = Self(next()?, next()?, next()?);
        channels.next().is_none().then_some(color)
    }

    fn mix(self, other: Self, amount: f32) -> Color {
        let blend = |from: u8, to: u8| {
            (f32::from(from) + (f32::from(to) - f32::from(from)) * amount).round() as u8
        };
        Color::Rgb(
            blend(self.0, other.0),
            blend(self.1, other.1),
            blend(self.2, other.2),
        )
    }
}

impl From<Rgb> for Color {
    fn from(Rgb(r, g, b): Rgb) -> Self {
        Color::Rgb(r, g, b)
    }
}

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
    let theme = theme();
    if fraction >= 0.85 {
        theme.crit
    } else if fraction >= 0.6 {
        theme.warn
    } else {
        theme.accent
    }
}

pub fn level(level: Level) -> Color {
    let theme = theme();
    match level {
        Level::Normal => theme.accent,
        Level::High => theme.warn,
        Level::Critical => theme.crit,
    }
}

pub fn agent(agent: Agent) -> (char, Color) {
    match agent {
        Agent::ClaudeCode => ('✻', theme().claude),
        Agent::Codex => ('◇', theme().codex),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_comes_from_the_terminal_and_fills_gaps_from_the_fallback() {
        let reply = b"\x1b_Gi=31;OK\x1b\\\x1b]10;rgb:e6e6/eded/f3f3\x1b\\\x1b]11;rgb:0d0d/1111/1717\x07\x1b]4;2;rgb:3f/b9/50\x1b\\\x1b]4;5;rgb:bcbc/8c8c/ffff\x1b\\\x1b[?62;22c";
        let palette = Palette::parse(reply, FALLBACK);
        assert_eq!(palette.foreground, Rgb(230, 237, 243));
        assert_eq!(palette.background, Rgb(13, 17, 23));
        assert_eq!(palette.green, Rgb(63, 185, 80));
        assert_eq!(palette.magenta, Rgb(188, 140, 255));
        assert_eq!(palette.red, FALLBACK.red);
        assert_eq!(Palette::parse(b"\x1b[?62c", palette), palette);
    }

    #[test]
    fn channels_of_any_hex_width_scale_to_eight_bits() {
        assert_eq!(Rgb::parse("f/0/8"), Some(Rgb(255, 0, 136)));
        assert_eq!(Rgb::parse("fff/000/800"), Some(Rgb(255, 0, 128)));
        assert_eq!(Rgb::parse("ffff/0000/8080"), Some(Rgb(255, 0, 128)));
        assert_eq!(Rgb::parse("ff/00"), None);
        assert_eq!(Rgb::parse("ff/00/00/00"), None);
        assert_eq!(Rgb::parse("fffff/0/0"), None);
        assert_eq!(Rgb::parse("zz/00/00"), None);
    }

    #[test]
    fn panels_lift_a_little_toward_the_text_colour_on_dark_and_light_themes() {
        let dark = Palette {
            foreground: Rgb(230, 237, 243),
            background: Rgb(13, 17, 23),
            ..FALLBACK
        };
        assert_eq!(dark.panel(0.0), Color::Rgb(21, 25, 31));
        let light = Palette {
            foreground: Rgb(30, 30, 30),
            background: Rgb(250, 250, 250),
            ..FALLBACK
        };
        assert_eq!(light.panel(0.0), Color::Rgb(243, 243, 243));
    }

    #[test]
    fn greys_sit_between_text_and_background_on_light_themes_too() {
        let light = Theme::from(Palette {
            foreground: Rgb(30, 30, 30),
            background: Rgb(250, 250, 250),
            ..FALLBACK
        });
        let Color::Rgb(dim, ..) = light.dim else {
            unreachable!()
        };
        let Color::Rgb(track, ..) = light.track else {
            unreachable!()
        };
        assert!(30 < dim && dim < track && track < 250, "{dim} {track}");
    }
}
