use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

const FILL: f32 = 0.9;
const LINE: f32 = 0.085;

#[derive(Clone, Copy)]
enum Shape {
    Laptop,
    Desktop,
    Mini,
    Studio,
    Rack,
    Cloud,
    Globe,
}

fn shape(name: &str) -> Option<Shape> {
    Some(match name {
        "macbook" | "laptopcomputer" | "laptop" => Shape::Laptop,
        "desktopcomputer" | "desktop" | "display" | "imac" => Shape::Desktop,
        "macmini" | "mini" => Shape::Mini,
        "macstudio" | "studio" => Shape::Studio,
        "server.rack" | "server" | "servers" | "xserve" => Shape::Rack,
        "cloud" => Shape::Cloud,
        "globe" => Shape::Globe,
        _ => return None,
    })
}

struct Canvas {
    pixmap: Pixmap,
    paint: Paint<'static>,
    stroke: Stroke,
    left: f32,
    top: f32,
    side: f32,
}

impl Canvas {
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        (self.left + x * self.side, self.top + y * self.side)
    }

    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32) {
        let (left, top) = self.at(x0, y0);
        let (right, bottom) = self.at(x1, y1);
        let r = radius * self.side;
        let mut path = PathBuilder::new();
        path.move_to(left + r, top);
        path.line_to(right - r, top);
        path.quad_to(right, top, right, top + r);
        path.line_to(right, bottom - r);
        path.quad_to(right, bottom, right - r, bottom);
        path.line_to(left + r, bottom);
        path.quad_to(left, bottom, left, bottom - r);
        path.line_to(left, top + r);
        path.quad_to(left, top, left + r, top);
        path.close();
        self.draw(path);
    }

    fn line(&mut self, points: &[(f32, f32)]) {
        let mut path = PathBuilder::new();
        for (index, &(x, y)) in points.iter().enumerate() {
            let (x, y) = self.at(x, y);
            if index == 0 {
                path.move_to(x, y);
            } else {
                path.line_to(x, y);
            }
        }
        self.draw(path);
    }

    fn dot(&mut self, x: f32, y: f32, radius: f32) {
        let (x, y) = self.at(x, y);
        let mut path = PathBuilder::new();
        path.push_circle(x, y, radius * self.side);
        if let Some(path) = path.finish() {
            self.pixmap.fill_path(
                &path,
                &self.paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    fn oval(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let (left, top) = self.at(x0, y0);
        let (right, bottom) = self.at(x1, y1);
        let mut path = PathBuilder::new();
        if let Some(bounds) = Rect::from_ltrb(left, top, right, bottom) {
            path.push_oval(bounds);
        }
        self.draw(path);
    }

    fn cloud(&mut self) {
        let mut path = PathBuilder::new();
        let point = |x: f32, y: f32| self.at(x, y);
        let (x, y) = point(0.26, 0.74);
        path.move_to(x, y);
        let (x, y) = point(0.76, 0.74);
        path.line_to(x, y);
        let curves = [
            [(0.95, 0.74), (0.97, 0.47), (0.78, 0.44)],
            [(0.74, 0.22), (0.47, 0.17), (0.38, 0.38)],
            [(0.12, 0.34), (0.04, 0.72), (0.26, 0.74)],
        ];
        for [a, b, end] in curves {
            let (ax, ay) = point(a.0, a.1);
            let (bx, by) = point(b.0, b.1);
            let (ex, ey) = point(end.0, end.1);
            path.cubic_to(ax, ay, bx, by, ex, ey);
        }
        path.close();
        self.draw(path);
    }

    fn draw(&mut self, path: PathBuilder) {
        if let Some(path) = path.finish() {
            self.pixmap.stroke_path(
                &path,
                &self.paint,
                &self.stroke,
                Transform::identity(),
                None,
            );
        }
    }
}

pub fn render(name: &str, width: u32, height: u32, color: (u8, u8, u8)) -> Option<Vec<u8>> {
    let name = system_name(name);
    #[cfg(target_os = "macos")]
    if let Some(pixels) = appkit::render(name, width, height, color) {
        return Some(pixels);
    }
    drawn(name, width, height, color)
}

fn system_name(name: &str) -> &str {
    match name {
        "server" | "servers" => "server.rack",
        "laptop" => "laptopcomputer",
        "desktop" => "desktopcomputer",
        "mini" => "macmini",
        "studio" => "macstudio",
        other => other,
    }
}

#[cfg(target_os = "macos")]
mod appkit {
    use objc2::AllocAnyThread;
    use objc2_app_kit::{
        NSBitmapImageRep, NSColor, NSCompositingOperation, NSDeviceRGBColorSpace,
        NSGraphicsContext, NSImage, NSImageSymbolConfiguration, NSRectFillUsingOperation,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    const FILL: f64 = 1.0;
    const SEMIBOLD: f64 = 0.3;

    pub fn render(name: &str, width: u32, height: u32, (r, g, b): (u8, u8, u8)) -> Option<Vec<u8>> {
        let configuration = NSImageSymbolConfiguration::configurationWithPointSize_weight(
            f64::from(height),
            SEMIBOLD,
        );
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(name),
            None,
        )?
        .imageWithSymbolConfiguration(&configuration)?;
        let size = image.size();
        if size.width <= 0.0 || size.height <= 0.0 {
            return None;
        }
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                width as isize,
                height as isize,
                8,
                4,
                true,
                false,
                NSDeviceRGBColorSpace,
                0,
                0,
            )
        }?;
        let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        let (w, h) = (f64::from(width), f64::from(height));
        let scale = (h * FILL / size.height).min(w * FILL / size.width);
        let (drawn_w, drawn_h) = (size.width * scale, size.height * scale);
        let target = NSRect::new(
            NSPoint::new((w - drawn_w) / 2.0, (h - drawn_h) / 2.0),
            NSSize::new(drawn_w, drawn_h),
        );
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&context));
        image.drawInRect_fromRect_operation_fraction(
            target,
            NSRect::ZERO,
            NSCompositingOperation::SourceOver,
            1.0,
        );
        NSColor::colorWithSRGBRed_green_blue_alpha(
            f64::from(r) / 255.0,
            f64::from(g) / 255.0,
            f64::from(b) / 255.0,
            1.0,
        )
        .set();
        NSRectFillUsingOperation(
            NSRect::new(NSPoint::ZERO, NSSize::new(w, h)),
            NSCompositingOperation::SourceAtop,
        );
        context.flushGraphics();
        NSGraphicsContext::restoreGraphicsState_class();
        let data = rep.bitmapData();
        if data.is_null() {
            return None;
        }
        let stride = rep.bytesPerRow() as usize;
        let row_bytes = width as usize * 4;
        let mut pixels = Vec::with_capacity(row_bytes * height as usize);
        for row in 0..height as usize {
            let line = unsafe { std::slice::from_raw_parts(data.add(row * stride), row_bytes) };
            pixels.extend_from_slice(line);
        }
        for pixel in pixels.as_chunks_mut::<4>().0 {
            let alpha = u16::from(pixel[3]);
            for channel in &mut pixel[..3] {
                if let Some(value) = (u16::from(*channel) * 255).checked_div(alpha) {
                    *channel = value.min(255) as u8;
                }
            }
        }
        Some(pixels)
    }
}

fn drawn(name: &str, width: u32, height: u32, (r, g, b): (u8, u8, u8)) -> Option<Vec<u8>> {
    let shape = shape(name)?;
    let pixmap = Pixmap::new(width, height)?;
    let side = width.min(height) as f32 * FILL;
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, 255);
    paint.anti_alias = true;
    let mut canvas = Canvas {
        pixmap,
        paint,
        stroke: Stroke {
            width: side * LINE,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        },
        left: (width as f32 - side) / 2.0,
        top: (height as f32 - side) / 2.0,
        side,
    };
    match shape {
        Shape::Laptop => {
            canvas.rect(0.2, 0.2, 0.8, 0.64, 0.05);
            canvas.line(&[(0.06, 0.78), (0.94, 0.78)]);
        }
        Shape::Desktop => {
            canvas.rect(0.1, 0.16, 0.9, 0.64, 0.06);
            canvas.line(&[(0.5, 0.64), (0.5, 0.8)]);
            canvas.line(&[(0.32, 0.82), (0.68, 0.82)]);
        }
        Shape::Mini => {
            canvas.rect(0.1, 0.4, 0.9, 0.68, 0.07);
            canvas.dot(0.76, 0.54, 0.035);
        }
        Shape::Studio => {
            canvas.rect(0.18, 0.28, 0.82, 0.74, 0.07);
            canvas.dot(0.32, 0.6, 0.035);
            canvas.dot(0.44, 0.6, 0.035);
        }
        Shape::Rack => {
            for (top, bottom) in [(0.14, 0.45), (0.55, 0.86)] {
                canvas.rect(0.1, top, 0.9, bottom, 0.06);
                let middle = (top + bottom) / 2.0;
                canvas.dot(0.27, middle, 0.045);
                canvas.line(&[(0.5, middle), (0.74, middle)]);
            }
        }
        Shape::Cloud => canvas.cloud(),
        Shape::Globe => {
            canvas.oval(0.12, 0.12, 0.88, 0.88);
            canvas.oval(0.33, 0.12, 0.67, 0.88);
            canvas.line(&[(0.12, 0.5), (0.88, 0.5)]);
        }
    }
    Some(canvas.pixmap.take_demultiplied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rex_host_icon_is_drawn_in_the_given_colour_and_others_are_not() {
        for name in [
            "macbook",
            "desktopcomputer",
            "macmini",
            "macstudio",
            "server.rack",
            "server",
            "cloud",
            "globe",
        ] {
            for pixels in [
                render(name, 32, 32, (200, 100, 50)),
                drawn(system_name(name), 32, 32, (200, 100, 50)),
            ] {
                let pixels = pixels.expect(name);
                assert_eq!(pixels.len(), 32 * 32 * 4);
                let solid: Vec<&[u8; 4]> = pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|pixel| pixel[3] > 160)
                    .collect();
                assert!(!solid.is_empty(), "{name} drew nothing");
                assert!(
                    solid.iter().all(|pixel| pixel[..3]
                        .iter()
                        .zip([200u8, 100, 50])
                        .all(|(got, want)| got.abs_diff(want) <= 3)),
                    "{name}"
                );
            }
        }
        assert!(render("no.such.icon", 32, 32, (0, 0, 0)).is_none());
    }
}
