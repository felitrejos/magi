#[cfg(target_os = "macos")]
mod mac {
    use std::hash::{DefaultHasher, Hash, Hasher};
    use std::path::{Path, PathBuf};

    use mddog::render::engine::{Block, Engine};
    use ratatui::style::Color;

    use crate::viewer::style::{self, theme};

    const BODY_FONTS: &str = "system-ui";
    const HEADING_SCALE: &str = "1.3 1.18 1.08 1 1 1";
    const HEADING_WEIGHT: &str = "560 540 520 500 500 500";
    const LINE_HEIGHT: &str = "1.55";

    pub struct Typesetter {
        engine: Engine,
        theme: mddog::theme::Theme,
        scale: f64,
        look: Look,
        text: String,
        blocks: Vec<Block>,
        texts: Vec<String>,
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

    type Look = (bool, Color, Color);

    fn look() -> Look {
        (style::is_dark(), theme().panel, theme().text)
    }

    fn theme_file(dark: bool) -> Option<PathBuf> {
        let t = theme();
        let path = crate::net::state_dir().ok()?.join("page-theme.css");
        std::fs::create_dir_all(path.parent()?).ok()?;
        let tokens = [
            (
                "base",
                if dark { "github-dark" } else { "github-light" }.to_string(),
            ),
            ("layout", "obsidian".into()),
            ("bg", colour(t.panel)),
            ("text", colour(t.prose)),
            ("text-muted", colour(t.dim)),
            ("heading", colour(t.text)),
            ("link", colour(t.codex)),
            ("accent", colour(t.faint)),
            ("rule", colour(t.rule)),
            ("border", colour(t.border)),
            ("code-bg", colour(t.raised)),
            ("pre-bg", colour(t.raised)),
            ("alt-bg", colour(t.raised)),
            ("font-body", BODY_FONTS.into()),
            ("heading-scale", HEADING_SCALE.into()),
            ("heading-weight", HEADING_WEIGHT.into()),
            ("line-height", LINE_HEIGHT.into()),
        ];
        let mut css = format!(
            ":root {{\n  color-scheme: {};\n",
            if dark { "dark" } else { "light" }
        );
        for (name, value) in tokens {
            css.push_str(&format!("  --mddog-{name}: {value};\n"));
        }
        css.push_str("}\n");
        std::fs::write(&path, css).ok()?;
        Some(path)
    }

    impl Typesetter {
        pub fn new() -> Option<Self> {
            let look = look();
            let theme = mddog::theme::file(&theme_file(look.0)?).ok()?;
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
                texts: Vec::new(),
                lines: Vec::new(),
                width: 0,
                row_height: 1,
                page_height: 0.0,
            })
        }

        pub fn matches_terminal(&self) -> bool {
            self.look == look()
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
            self.texts = layout.texts;
        }

        pub fn rows(&self) -> usize {
            (self.page_height * self.scale / f64::from(self.row_height)).ceil() as usize
        }

        pub fn key(&self, row: usize) -> u64 {
            let (top, bottom) = self.span(row);
            let mut hasher = DefaultHasher::new();
            (row, self.width, self.row_height, self.look).hash(&mut hasher);
            for (index, block) in self
                .blocks
                .iter()
                .enumerate()
                .filter(|(_, block)| block.top < bottom && block.top + block.height > top)
            {
                (block.top.to_bits(), block.height.to_bits()).hash(&mut hasher);
                self.texts.get(index).hash(&mut hasher);
                let last = (block.end as usize + 1).min(self.lines.len());
                let first = (block.start as usize).saturating_sub(1).min(last);
                self.lines.get(first..last).hash(&mut hasher);
            }
            hasher.finish()
        }

        #[cfg(test)]
        pub fn row_of_text(&self, needle: &str) -> Option<usize> {
            let index = self.texts.iter().position(|text| text.contains(needle))?;
            let block = self.blocks.get(index)?;
            Some((block.top * self.scale / f64::from(self.row_height)) as usize)
        }

        pub fn rows_of(&self, first_line: u32, last_line: u32) -> Option<(usize, usize)> {
            let (top, bottom) = self
                .blocks
                .iter()
                .filter(|block| block.start >= first_line && block.end <= last_line)
                .fold(None, |span: Option<(f64, f64)>, block| {
                    let (top, bottom) = (block.top, block.top + block.height);
                    Some(span.map_or((top, bottom), |(a, b)| (a.min(top), b.max(bottom))))
                })?;
            let row = |at: f64| at * self.scale / f64::from(self.row_height);
            Some((row(top).floor() as usize, row(bottom).ceil() as usize))
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

    pub fn rows_of(&self, _first_line: u32, _last_line: u32) -> Option<(usize, usize)> {
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "writes a picture: MAGI_PNG=/path.png cargo test -- --ignored print_typeset"]
    fn print_typeset() {
        let path = std::env::var("MAGI_PNG").unwrap();
        let mut paper = Typesetter::new().unwrap();
        paper.fit(1400, 34);
        let sample = std::env::var("MAGI_MD")
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok());
        paper.show(
            sample.as_deref().unwrap_or("## Grouping the cores\n\nThe agent reads each core's **cluster type** once, from the device tree, and sends it with the machine's identity. The view then shows `Performance` and `Efficiency` apart.\n\n- Each group is numbered on its own\n- Its average sits beside its title\n\n> A quote stays quiet.\n\n```rust\nfn kinds() -> Vec<CoreKind> {\n    mac::kinds().unwrap_or_default()\n}\n```\n\nCommitted and pushed, see [the commit](https://example.com)."),
            std::path::Path::new("."),
        );
        let rows = paper.rows();
        let mut pixmap = tiny_skia::Pixmap::new(1400, 34 * rows as u32).unwrap();
        for row in 0..rows {
            let Some(rgba) = paper.pixels(row, 1400, 34) else {
                continue;
            };
            let strip =
                tiny_skia::Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(1400, 34).unwrap())
                    .unwrap();
            pixmap.draw_pixmap(
                0,
                34 * row as i32,
                strip.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                tiny_skia::Transform::identity(),
                None,
            );
        }
        pixmap.save_png(path).unwrap();
    }

    #[test]
    fn a_changed_short_paragraph_gets_new_pixels_and_unchanged_rows_keep_theirs() {
        let mut paper = Typesetter::new().unwrap();
        paper.fit(1200, 34);
        let keys = |paper: &Typesetter| {
            (0..paper.rows())
                .map(|row| paper.key(row))
                .collect::<Vec<_>>()
        };
        paper.show(
            "First paragraph.\n\nThat's it.\n\nAfter.",
            std::path::Path::new("."),
        );
        let changed = paper.row_of_text("That's it.").unwrap();
        let before = keys(&paper);
        paper.show(
            "First paragraph.\n\nWhat else.\n\nAfter.",
            std::path::Path::new("."),
        );
        let after = keys(&paper);
        assert_eq!(before[0], after[0]);
        assert_ne!(before[changed], after[changed]);
    }
}
