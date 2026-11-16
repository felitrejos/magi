#[cfg(target_os = "macos")]
mod mac {
    use std::hash::{DefaultHasher, Hash, Hasher};
    use std::path::{Path, PathBuf};

    use mddog::render::engine::{Block, Engine};
    use ratatui::style::Color;

    use crate::viewer::style::{self, theme};

    pub struct Typesetter {
        engine: Engine,
        theme: mddog::theme::Theme,
        scale: f64,
        look: (bool, Color),
        text: String,
        blocks: Vec<Block>,
        lines: Vec<u64>,
        width: u32,
        row_height: u32,
        page_height: f64,
    }

    fn colour(color: Color) -> String {
        match color {
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            _ => "#000000".into(),
        }
    }

    fn theme_file(look: (bool, Color)) -> Option<PathBuf> {
        let (dark, background) = look;
        let base = if dark { "github-dark" } else { "github-light" };
        let path = crate::net::state_dir().ok()?.join("page-theme.css");
        std::fs::create_dir_all(path.parent()?).ok()?;
        let css = format!(
            ":root {{\n  --mddog-base: {base};\n  color-scheme: {scheme};\n  --mddog-bg: {bg};\n}}\n",
            scheme = if dark { "dark" } else { "light" },
            bg = colour(background),
        );
        std::fs::write(&path, css).ok()?;
        Some(path)
    }

    impl Typesetter {
        pub fn new() -> Option<Self> {
            let look = (style::is_dark(), theme().panel);
            let theme = mddog::theme::file(&theme_file(look)?).ok()?;
            let scale = mddog::term::screen::display_scale();
            let engine = Engine::new(1.0, scale, true);
            engine.load("", Path::new("."), true, &theme);
            Some(Self {
                engine,
                theme,
                scale,
                look,
                text: String::new(),
                blocks: Vec::new(),
                lines: Vec::new(),
                width: 0,
                row_height: 1,
                page_height: 0.0,
            })
        }

        pub fn matches_terminal(&self) -> bool {
            self.look == (style::is_dark(), theme().panel)
        }

        pub fn show(&mut self, text: &str, base: &Path) {
            if text == self.text {
                return;
            }
            self.text = text.to_string();
            self.lines = text
                .lines()
                .map(|line| {
                    let mut hasher = DefaultHasher::new();
                    line.hash(&mut hasher);
                    hasher.finish()
                })
                .collect();
            self.engine.load(text, base, true, &self.theme);
            self.relaid();
        }

        pub fn fit(&mut self, width: u32, row_height: u32) {
            if width == self.width && row_height == self.row_height {
                return;
            }
            self.engine.resize(f64::from(width) / self.scale);
            self.width = width;
            self.row_height = row_height.max(1);
            self.relaid();
        }

        fn relaid(&mut self) {
            let layout = self.engine.layout();
            self.page_height = layout.height;
            self.blocks = layout.blocks;
        }

        pub fn rows(&self) -> usize {
            (self.page_height * self.scale / f64::from(self.row_height)).ceil() as usize
        }

        pub fn key(&self, row: usize) -> u64 {
            let (top, bottom) = self.span(row);
            let mut hasher = DefaultHasher::new();
            (row, self.width, self.row_height, self.look.0).hash(&mut hasher);
            for block in self
                .blocks
                .iter()
                .filter(|block| block.top < bottom && block.top + block.height > top)
            {
                (block.top.to_bits(), block.height.to_bits()).hash(&mut hasher);
                let lines = (block.start as usize)..(block.end as usize).min(self.lines.len());
                self.lines.get(lines).hash(&mut hasher);
            }
            hasher.finish()
        }

        fn span(&self, row: usize) -> (f64, f64) {
            let height = f64::from(self.row_height) / self.scale;
            (row as f64 * height, (row + 1) as f64 * height)
        }

        pub fn pixels(&self, row: usize, width: u32, height: u32) -> Option<Vec<u8>> {
            let (top, _) = self.span(row);
            let snapshot =
                self.engine
                    .snapshot(top, f64::from(height) / self.scale, self.scale, |len| {
                        Some(vec![0u8; len])
                    })?;
            (snapshot.width == width as usize && snapshot.height == height as usize)
                .then_some(snapshot.rgba)
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::Typesetter;

#[cfg(not(target_os = "macos"))]
pub enum Typesetter {}

#[cfg(not(target_os = "macos"))]
impl Typesetter {
    pub fn new() -> Option<Self> {
        None
    }

    pub fn matches_terminal(&self) -> bool {
        match *self {}
    }

    pub fn show(&mut self, _text: &str, _base: &std::path::Path) {
        match *self {}
    }

    pub fn fit(&mut self, _width: u32, _row_height: u32) {
        match *self {}
    }

    pub fn rows(&self) -> usize {
        match *self {}
    }

    pub fn key(&self, _row: usize) -> u64 {
        match *self {}
    }

    pub fn pixels(&self, _row: usize, _width: u32, _height: u32) -> Option<Vec<u8>> {
        match *self {}
    }
}

pub fn draw_rows(
    buf: &mut ratatui::buffer::Buffer,
    graphics: &mut super::graphics::Graphics,
    typesetter: &Typesetter,
    area: ratatui::layout::Rect,
    first: usize,
    slot: &str,
) {
    let total = typesetter.rows();
    for offset in 0..(area.height as usize).min(total.saturating_sub(first)) {
        let row = first + offset;
        let strip = ratatui::layout::Rect::new(area.x, area.y + offset as u16, area.width, 1);
        graphics.strip(
            buf,
            strip,
            format!("{slot}:{row}"),
            typesetter.key(row),
            |width, height| typesetter.pixels(row, width, height),
        );
    }
}
