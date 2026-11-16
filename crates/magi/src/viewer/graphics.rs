use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::{self, Write};

use flate2::Compression;
use flate2::write::ZlibEncoder;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

use super::base64;
use super::chart;
use super::diacritics::DIACRITICS;
use super::style::theme;
use super::symbols;

const PLACEHOLDER: char = '\u{10EEEE}';
const CHUNK: usize = 4096;
const OVERSAMPLE: f32 = 2.0;
const MAX_PIXELS: f32 = 1_000_000.0;
const FILL_ALPHA: u8 = 56;
const FIRST_ID: u32 = 0x4d_0001;
const LAST_ID: u32 = 0xff_fff0;
const EIGHTHS: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];
const PLACEHOLDER_TERMINALS: [&str; 2] = ["rex ", "ghostty "];
const KITTY_PLACEHOLDERS_SINCE: (u32, u32) = (0, 28);

#[derive(Clone, Copy)]
pub enum Kind {
    Area,
    Meter,
}

pub struct Series<'a> {
    pub values: &'a [f32],
    pub color: Color,
}

pub struct Chart<'a> {
    pub kind: Kind,
    pub max: f32,
    pub guide: bool,
    pub series: Vec<Series<'a>>,
}

pub struct Graphics {
    cell: Option<(u32, u32)>,
    slots: HashMap<String, Slot>,
    used: HashSet<String>,
    uploads: Vec<Vec<u8>>,
    stale: Vec<u32>,
    next_id: u32,
    missing: HashSet<String>,
}

struct Slot {
    id: u32,
    key: u64,
}

impl Graphics {
    #[cfg(test)]
    pub fn cells_only() -> Self {
        Self::new(None)
    }

    #[cfg(test)]
    pub fn pixels(cell_width: u32, cell_height: u32) -> Self {
        Self::new(Some((cell_width, cell_height)))
    }

    fn new(cell: Option<(u32, u32)>) -> Self {
        Self {
            cell,
            slots: HashMap::new(),
            used: HashSet::new(),
            uploads: Vec::new(),
            stale: Vec::new(),
            next_id: FIRST_ID,
            missing: HashSet::new(),
        }
    }

    pub fn from_reply(reply: &[u8]) -> Self {
        Self::new(parse_reply(reply))
    }

    pub fn meter(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        slot: String,
        fraction: f32,
        color: Color,
    ) {
        self.chart(
            buf,
            area,
            slot,
            &Chart {
                kind: Kind::Meter,
                max: 1.0,
                guide: false,
                series: vec![Series {
                    values: &[fraction],
                    color,
                }],
            },
        );
    }

    pub fn chart(&mut self, buf: &mut Buffer, area: Rect, slot: String, chart: &Chart) {
        let area = area.intersection(buf.area);
        if area.is_empty() {
            return;
        }
        let Some((cell_width, cell_height)) = self.cell else {
            draw_cells(buf, area, chart);
            return;
        };
        let cols = area.width.min(DIACRITICS.len() as u16);
        let rows = area.height.min(DIACRITICS.len() as u16);
        let key = fingerprint(chart, cols, rows);
        let id = match self.slots.get(&slot) {
            Some(existing) if existing.key == key => existing.id,
            _ => {
                let id = self.allocate();
                let (width, height, scale) = pixel_size(cols, rows, cell_width, cell_height);
                if let Some(pixels) = render(chart, width, height, scale) {
                    self.uploads
                        .push(upload(id, cols, rows, width, height, &pixels));
                }
                if let Some(old) = self.slots.insert(slot.clone(), Slot { id, key }) {
                    self.stale.push(old.id);
                }
                id
            }
        };
        self.used.insert(slot);
        place(
            buf,
            Rect {
                width: cols,
                height: rows,
                ..area
            },
            id,
        );
    }

    pub fn icon(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        slot: String,
        symbols: &[String],
        color: Color,
    ) -> bool {
        let area = area.intersection(buf.area);
        let Some((cell_width, cell_height)) = self.cell else {
            return false;
        };
        if area.is_empty() {
            return false;
        }
        let mut hasher = DefaultHasher::new();
        (symbols, color, area.width, area.height).hash(&mut hasher);
        let key = hasher.finish();
        let id = match self.slots.get(&slot) {
            Some(existing) if existing.key == key => existing.id,
            _ => {
                let (width, height, _) =
                    pixel_size(area.width, area.height, cell_width, cell_height);
                let Some(pixels) = symbols.iter().find_map(|symbol| {
                    if self.missing.contains(symbol) {
                        return None;
                    }
                    let pixels = symbols::render(symbol, width, height, rgb(color));
                    if pixels.is_none() {
                        self.missing.insert(symbol.clone());
                    }
                    pixels
                }) else {
                    return false;
                };
                let id = self.allocate();
                self.uploads
                    .push(upload(id, area.width, area.height, width, height, &pixels));
                if let Some(old) = self.slots.insert(slot.clone(), Slot { id, key }) {
                    self.stale.push(old.id);
                }
                id
            }
        };
        self.used.insert(slot);
        place(buf, area, id);
        true
    }

    pub fn flush(&mut self, out: &mut impl Write) -> io::Result<()> {
        let unused: Vec<String> = self
            .slots
            .keys()
            .filter(|slot| !self.used.contains(*slot))
            .cloned()
            .collect();
        for slot in unused {
            if let Some(gone) = self.slots.remove(&slot) {
                self.stale.push(gone.id);
            }
        }
        self.used.clear();
        for sequence in self.uploads.drain(..) {
            out.write_all(&sequence)?;
        }
        for id in self.stale.drain(..) {
            write!(out, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\")?;
        }
        out.flush()
    }

    pub fn clear(&mut self, out: &mut impl Write) -> io::Result<()> {
        for slot in self.slots.values() {
            write!(out, "\x1b_Ga=d,d=I,i={},q=2\x1b\\", slot.id)?;
        }
        self.slots.clear();
        out.flush()
    }

    pub fn set_cell(&mut self, cell: (u32, u32)) -> bool {
        if self.cell.is_none_or(|known| known == cell) {
            return false;
        }
        self.cell = Some(cell);
        self.stale
            .extend(self.slots.drain().map(|(_, slot)| slot.id));
        true
    }

    pub fn has_pixels(&self) -> bool {
        self.cell.is_some()
    }

    pub fn samples_per_cell(&self) -> usize {
        if self.cell.is_some() { 2 } else { 1 }
    }

    fn allocate(&mut self) -> u32 {
        loop {
            let id = self.next_id;
            self.next_id = if id >= LAST_ID { FIRST_ID } else { id + 1 };
            if !self.slots.values().any(|slot| slot.id == id) && !self.stale.contains(&id) {
                return id;
            }
        }
    }
}

fn parse_reply(reply: &[u8]) -> Option<(u32, u32)> {
    let text = String::from_utf8_lossy(reply);
    if !text.contains("_Gi=31;OK") || !places_unicode_placeholders(&text) {
        return None;
    }
    let size = text.split("\x1b[6;").nth(1)?.split('t').next()?;
    let (height, width) = size.split_once(';')?;
    let (height, width): (u32, u32) = (height.parse().ok()?, width.parse().ok()?);
    (height > 0 && width > 0).then_some((width, height))
}

fn places_unicode_placeholders(reply: &str) -> bool {
    let name = reply
        .split("\x1bP>|")
        .nth(1)
        .and_then(|rest| rest.split("\x1b\\").next())
        .unwrap_or_default()
        .to_ascii_lowercase();
    kitty_version(&name).is_some_and(|version| version >= KITTY_PLACEHOLDERS_SINCE)
        || PLACEHOLDER_TERMINALS
            .iter()
            .any(|terminal| name.starts_with(terminal))
}

fn kitty_version(name: &str) -> Option<(u32, u32)> {
    let mut parts = name.strip_prefix("kitty(")?.split(['.', ')']);
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn fingerprint(chart: &Chart, cols: u16, rows: u16) -> u64 {
    let mut hasher = DefaultHasher::new();
    (
        cols,
        rows,
        chart.max.to_bits(),
        chart.kind as u8,
        chart.guide,
        theme().track,
        theme().guide,
    )
        .hash(&mut hasher);
    for series in &chart.series {
        series.color.hash(&mut hasher);
        for value in series.values {
            value.to_bits().hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn rgb(color: Color) -> (u8, u8, u8) {
    let Color::Rgb(r, g, b) = color else {
        unreachable!("chart colours are RGB tokens")
    };
    (r, g, b)
}

fn pixel_size(cols: u16, rows: u16, cell_width: u32, cell_height: u32) -> (u32, u32, f32) {
    let base_width = f32::from(cols) * cell_width as f32;
    let base_height = f32::from(rows) * cell_height as f32;
    let scale = OVERSAMPLE.min((MAX_PIXELS / (base_width * base_height)).sqrt());
    (
        (base_width * scale).round().max(1.0) as u32,
        (base_height * scale).round().max(1.0) as u32,
        scale,
    )
}

fn finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

fn render(chart: &Chart, width: u32, height: u32, density: f32) -> Option<Vec<u8>> {
    let mut pixmap = Pixmap::new(width, height)?;
    let (w, h) = (width as f32, height as f32);
    let scale = |value: f32| h - (finite(value) / chart.max).clamp(0.0, 1.0) * (h - 2.0) - 1.0;
    if chart.guide {
        guide(&mut pixmap, scale(chart.max / 2.0), w, density);
    }
    for series in &chart.series {
        let values = series.values;
        if values.is_empty() {
            continue;
        }
        let (r, g, b) = rgb(series.color);
        let mut solid = Paint::default();
        solid.set_color_rgba8(r, g, b, 255);
        match chart.kind {
            Kind::Meter => {
                let thickness = (h * 0.4).max(2.0);
                let top = (h - thickness) / 2.0;
                let (tr, tg, tb) = rgb(theme().track);
                let mut track = Paint::default();
                track.set_color_rgba8(tr, tg, tb, 255);
                let fraction = finite(values[0]).clamp(0.0, 1.0);
                let filled = if fraction > 0.0 {
                    (w * fraction).max(thickness)
                } else {
                    0.0
                };
                for (paint, right) in [(&track, w), (&solid, filled)] {
                    if right <= 0.0 {
                        continue;
                    }
                    let path = pill(top, right, thickness)?;
                    pixmap.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
                }
            }
            Kind::Area => {
                let step = if values.len() > 1 {
                    w / (values.len() - 1) as f32
                } else {
                    w
                };
                let x = |index: usize| w - (values.len() - 1 - index) as f32 * step;
                let mut line = PathBuilder::new();
                let mut area = PathBuilder::new();
                area.move_to(x(0), h);
                for (index, value) in values.iter().enumerate() {
                    let point = (x(index), scale(*value));
                    if index == 0 {
                        line.move_to(point.0, point.1);
                    } else {
                        line.line_to(point.0, point.1);
                    }
                    area.line_to(point.0, point.1);
                }
                area.line_to(w, h);
                area.close();
                let mut fill = Paint::default();
                fill.set_color_rgba8(r, g, b, FILL_ALPHA);
                if let Some(path) = area.finish() {
                    pixmap.fill_path(&path, &fill, FillRule::Winding, Transform::identity(), None);
                }
                if let Some(path) = line.finish() {
                    let stroke = Stroke {
                        width: 1.5 * density,
                        ..Stroke::default()
                    };
                    pixmap.stroke_path(&path, &solid, &stroke, Transform::identity(), None);
                }
            }
        }
    }
    Some(pixmap.take_demultiplied())
}

fn guide(pixmap: &mut Pixmap, y: f32, width: f32, density: f32) {
    let (r, g, b) = rgb(theme().guide);
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, 255);
    let dash = 4.0 * density;
    let mut x = 0.0;
    while x < width {
        if let Some(rect) = tiny_skia::Rect::from_xywh(x, y.round(), dash, density.max(1.0)) {
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }
        x += dash * 2.0;
    }
}

fn pill(top: f32, right: f32, thickness: f32) -> Option<tiny_skia::Path> {
    let radius = thickness / 2.0;
    let mut path = PathBuilder::new();
    if right <= thickness {
        path.push_circle(radius, top + radius, radius);
        return path.finish();
    }
    let kappa = 0.552_284_8 * radius;
    let bottom = top + thickness;
    path.move_to(radius, top);
    path.line_to(right - radius, top);
    path.cubic_to(
        right - radius + kappa,
        top,
        right,
        top + radius - kappa,
        right,
        top + radius,
    );
    path.cubic_to(
        right,
        top + radius + kappa,
        right - radius + kappa,
        bottom,
        right - radius,
        bottom,
    );
    path.line_to(radius, bottom);
    path.cubic_to(
        radius - kappa,
        bottom,
        0.0,
        top + radius + kappa,
        0.0,
        top + radius,
    );
    path.cubic_to(0.0, top + radius - kappa, radius - kappa, top, radius, top);
    path.close();
    path.finish()
}

pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.get(..8)? != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let word = |at: usize| Some(u32::from_be_bytes(png.get(at..at + 4)?.try_into().ok()?));
    Some((word(16)?, word(20)?)).filter(|(width, height)| *width > 0 && *height > 0)
}

pub fn inline_png(png: &[u8], id: u32, cols: u16, rows: u16) -> String {
    let (cols, rows) = (
        cols.min(DIACRITICS.len() as u16),
        rows.min(DIACRITICS.len() as u16),
    );
    let payload = base64::encode(png);
    let chunks: Vec<&[u8]> = payload.as_bytes().chunks(CHUNK).collect();
    let mut out = String::with_capacity(payload.len() + usize::from(rows * cols) * 12);
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        if index == 0 {
            out.push_str(&format!(
                "\x1b_Ga=T,f=100,i={id},U=1,c={cols},r={rows},q=2,m={more};"
            ));
        } else {
            out.push_str(&format!("\x1b_Gm={more},q=2;"));
        }
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push_str("\x1b\\");
    }
    for row in 0..rows {
        out.push_str(&format!(
            "\x1b[38;2;{};{};{}m",
            (id >> 16) as u8,
            (id >> 8) as u8,
            id as u8
        ));
        for col in 0..cols {
            out.push(PLACEHOLDER);
            out.push(DIACRITICS[usize::from(row)]);
            out.push(DIACRITICS[usize::from(col)]);
        }
        out.push_str("\x1b[39m\n");
    }
    out
}

fn upload(id: u32, cols: u16, rows: u16, width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(3));
    let compressed = encoder
        .write_all(rgba)
        .and_then(|()| encoder.finish())
        .expect("compressing into memory cannot fail");
    let payload = base64::encode(&compressed);
    let chunks: Vec<&[u8]> = payload.as_bytes().chunks(CHUNK).collect();
    let mut out = Vec::with_capacity(payload.len() + chunks.len() * 16 + 96);
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        if index == 0 {
            out.extend_from_slice(
                format!(
                    "\x1b_Ga=T,f=32,o=z,s={width},v={height},i={id},U=1,c={cols},r={rows},q=2,m={more};"
                )
                .as_bytes(),
            );
        } else {
            out.extend_from_slice(format!("\x1b_Gm={more},q=2;").as_bytes());
        }
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

fn place(buf: &mut Buffer, area: Rect, id: u32) {
    let color = Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8);
    let mut symbol = String::with_capacity(12);
    for row in 0..area.height {
        for col in 0..area.width {
            symbol.clear();
            symbol.push(PLACEHOLDER);
            symbol.push(DIACRITICS[row as usize]);
            symbol.push(DIACRITICS[col as usize]);
            buf[(area.x + col, area.y + row)]
                .set_symbol(&symbol)
                .set_fg(color);
        }
    }
}

fn draw_cells(buf: &mut Buffer, area: Rect, chart: &Chart) {
    let width = area.width as usize;
    let height = area.height as usize;
    if chart.guide && height % 2 == 1 {
        draw_guide_cells(buf, area);
    }
    for series in chart.series.iter().rev() {
        let scaled: Vec<f32> = series
            .values
            .iter()
            .map(|value| value / chart.max)
            .collect();
        let values = match chart.kind {
            Kind::Meter => {
                draw_meter_cells(buf, area, finite(series.values[0]), series.color);
                continue;
            }
            Kind::Area => scaled,
        };
        for (row, line) in chart::columns(&values, width, height).iter().enumerate() {
            for (col, cell) in line.iter().enumerate() {
                if let Some(symbol) = cell {
                    buf[(area.x + col as u16, area.y + row as u16)]
                        .set_char(*symbol)
                        .set_fg(series.color);
                }
            }
        }
    }
}

fn draw_guide_cells(buf: &mut Buffer, area: Rect) {
    let y = area.y + area.height / 2;
    for x in area.x..area.x + area.width {
        buf[(x, y)].set_char('┈').set_fg(theme().guide);
    }
}

fn draw_meter_cells(buf: &mut Buffer, area: Rect, fraction: f32, color: Color) {
    let width = area.width as usize;
    let fraction = fraction.clamp(0.0, 1.0);
    let eighths = (fraction * (width * 8) as f32).round() as usize;
    let eighths = if fraction > 0.0 { eighths.max(1) } else { 0 };
    for col in 0..width {
        let cell = &mut buf[(area.x + col as u16, area.y)];
        match eighths.saturating_sub(col * 8).min(8) {
            8 => cell.set_char('█').set_fg(color),
            0 => cell.set_char('█').set_fg(theme().track),
            partial => cell
                .set_char(EIGHTHS[partial - 1])
                .set_fg(color)
                .set_bg(theme().track),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLUE: Color = Color::Rgb(108, 164, 255);

    fn chart(values: &[f32]) -> Chart<'_> {
        Chart {
            kind: Kind::Area,
            max: 1.0,
            guide: true,
            series: vec![Series {
                values,
                color: BLUE,
            }],
        }
    }

    #[test]
    fn pixels_need_kitty_graphics_with_unicode_placeholders_and_a_cell_size() {
        let reply = |extra: &str| {
            format!("\x1b_Gi=31;OK\x1b\\{extra}\x1b[6;16;8t\x1b[?62;22c").into_bytes()
        };
        assert_eq!(
            parse_reply(&reply("\x1bP>|rex 0.1.0-preview.18\x1b\\")),
            Some((8, 16))
        );
        assert_eq!(
            parse_reply(&reply("\x1bP>|ghostty 1.2.3\x1b\\")),
            Some((8, 16))
        );
        assert_eq!(
            parse_reply(&reply("\x1bP>|kitty(0.42.1)\x1b\\")),
            Some((8, 16))
        );
        assert_eq!(
            parse_reply(&reply("\x1bP>|kitty(1.0.0)\x1b\\")),
            Some((8, 16))
        );
        assert_eq!(parse_reply(&reply("\x1bP>|kitty(0.27.0)\x1b\\")), None);
        assert_eq!(parse_reply(&reply("\x1bP>|WezTerm 20240203\x1b\\")), None);
        assert_eq!(parse_reply(&reply("")), None);
        assert_eq!(parse_reply(b"\x1b[6;16;8t\x1b[?62;22c"), None);
        assert_eq!(
            parse_reply(b"\x1b_Gi=31;OK\x1b\\\x1bP>|rex 0.1\x1b\\\x1b[?62c"),
            None
        );
    }

    #[test]
    fn placeholders_name_the_image_row_and_column_and_keep_the_background() {
        let mut graphics = Graphics::new(Some((8, 16)));
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 4));
        buf[(3, 2)].set_bg(Color::Rgb(1, 2, 3));
        graphics.chart(
            &mut buf,
            Rect::new(2, 1, 3, 2),
            "cpu".into(),
            &chart(&[0.1, 0.5, 0.9]),
        );
        let id = FIRST_ID;
        let cell = &buf[(3, 2)];
        assert_eq!(
            cell.symbol(),
            format!("{PLACEHOLDER}{}{}", DIACRITICS[1], DIACRITICS[1])
        );
        assert_eq!(
            cell.fg,
            Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8)
        );
        assert_eq!(cell.bg, Color::Rgb(1, 2, 3));
        assert_eq!(buf[(0, 0)].symbol(), " ");
        let mut out = Vec::new();
        graphics.flush(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with(&format!(
            "\x1b_Ga=T,f=32,o=z,s=48,v=64,i={id},U=1,c=3,r=2,q=2,m=0;"
        )));
        assert!(!text.contains("a=d"));
    }

    #[test]
    fn unchanged_charts_are_not_sent_again_and_replaced_ones_are_freed() {
        let mut graphics = Graphics::new(Some((8, 16)));
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 8));
        let area = Rect::new(0, 0, 40, 8);
        let mut flushed = |graphics: &mut Graphics, values: &[f32]| {
            graphics.chart(&mut buf, area, "cpu".into(), &chart(values));
            let mut out = Vec::new();
            graphics.flush(&mut out).unwrap();
            String::from_utf8(out).unwrap()
        };
        let first = flushed(&mut graphics, &[0.2, 0.4]);
        assert!(first.contains("a=T"));
        assert_eq!(flushed(&mut graphics, &[0.2, 0.4]), "");
        let changed = flushed(&mut graphics, &[0.2, 0.4, 0.6]);
        assert!(changed.contains(&format!("i={}", FIRST_ID + 1)));
        assert!(changed.ends_with(&format!("\x1b_Ga=d,d=I,i={FIRST_ID},q=2\x1b\\")));
        let mut out = Vec::new();
        graphics.flush(&mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("\x1b_Ga=d,d=I,i={},q=2\x1b\\", FIRST_ID + 1)
        );
    }

    #[test]
    fn big_panes_are_drawn_at_a_bounded_pixel_count_with_the_same_shape() {
        assert_eq!(pixel_size(40, 8, 8, 16), (640, 256, 2.0));
        let (width, height, scale) = pixel_size(297, 297, 16, 32);
        assert!((width * height) as f32 <= MAX_PIXELS * 1.01);
        assert!(scale < 1.0);
        let shape = |w: u32, h: u32| w as f32 / h as f32;
        assert!((shape(width, height) - shape(297 * 16, 297 * 32)).abs() < 0.01);
    }

    #[test]
    fn ids_still_in_use_are_never_handed_out_again() {
        let mut graphics = Graphics::pixels(8, 16);
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
        let area = buf.area;
        graphics.chart(&mut buf, area, "disk".into(), &chart(&[0.5]));
        graphics.next_id = LAST_ID;
        assert_eq!(graphics.allocate(), LAST_ID);
        assert_eq!(graphics.allocate(), FIRST_ID + 1);
    }

    #[test]
    fn large_images_are_sent_in_chunks_of_at_most_4096_bytes() {
        let mut state = 0x2545_f491_u32;
        let noise: Vec<u8> = (0..40_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let sent = String::from_utf8(upload(7, 10, 2, 100, 100, &noise)).unwrap();
        let chunks: Vec<&str> = sent
            .split("\x1b\\")
            .filter(|chunk| !chunk.is_empty())
            .collect();
        assert!(chunks.len() > 2, "{}", chunks.len());
        assert!(chunks[0].starts_with("\x1b_Ga=T,") && chunks[0].contains("m=1;"));
        assert!(chunks[1].starts_with("\x1b_Gm=1,q=2;"));
        assert!(chunks.last().unwrap().starts_with("\x1b_Gm=0,q=2;"));
        for chunk in chunks {
            let payload = chunk.split_once(';').unwrap().1;
            assert!(payload.len() <= CHUNK);
        }
    }

    #[test]
    fn without_kitty_charts_and_meters_fall_back_to_block_characters() {
        let mut graphics = Graphics::cells_only();
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        let area = buf.area;
        graphics.chart(&mut buf, area, "cpu".into(), &chart(&[1.0, 1.0, 1.0, 1.0]));
        assert_eq!(buf[(3, 0)].symbol(), "█");
        let mut meter = Buffer::empty(Rect::new(0, 0, 4, 1));
        let area = meter.area;
        graphics.meter(&mut meter, area, "m".into(), 0.56, BLUE);
        assert_eq!(meter[(0, 0)].symbol(), "█");
        assert_eq!(meter[(2, 0)].symbol(), "▎");
        assert_eq!((meter[(2, 0)].fg, meter[(2, 0)].bg), (BLUE, theme().track));
        assert_eq!(meter[(3, 0)].fg, theme().track);
        let mut out = Vec::new();
        graphics.flush(&mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn the_cell_guide_sits_on_the_true_middle_and_never_covers_a_line() {
        let two = |values: &'static [f32]| Chart {
            kind: Kind::Area,
            max: 1.0,
            guide: true,
            series: vec![
                Series {
                    values,
                    color: BLUE,
                },
                Series {
                    values: &[0.0, 0.0],
                    color: theme().track,
                },
            ],
        };
        let mut graphics = Graphics::cells_only();
        let mut odd = Buffer::empty(Rect::new(0, 0, 2, 3));
        let area = odd.area;
        graphics.chart(&mut odd, area, "n".into(), &two(&[1.0, 0.0]));
        assert_eq!(odd[(0, 1)].symbol(), "█");
        assert_eq!(odd[(1, 1)].symbol(), "┈");
        let mut even = Buffer::empty(Rect::new(0, 0, 2, 4));
        let area = even.area;
        graphics.chart(&mut even, area, "n".into(), &two(&[0.0, 0.0]));
        assert!((0..4).all(|y| even[(1, y)].symbol() != "┈"));
    }

    #[test]
    fn rendered_pixels_follow_the_values() {
        let pixels = render(&chart(&[1.0, 1.0]), 20, 10, 2.0).unwrap();
        let alpha = |x: usize, y: usize| pixels[(y * 20 + x) * 4 + 3];
        assert_eq!(alpha(10, 1), 255);
        assert!(alpha(10, 8) > 0 && alpha(10, 8) < 255);
        let empty = render(&chart(&[0.0, 0.0]), 20, 10, 1.0).unwrap();
        assert_eq!(empty[(2 * 20 + 10) * 4 + 3], 0);
        let middle = (1..9)
            .find(|y| empty[(y * 20) * 4 + 3] == 255)
            .expect("a dashed 50% guide");
        assert!((4..=6).contains(&middle), "{middle}");
    }
}
