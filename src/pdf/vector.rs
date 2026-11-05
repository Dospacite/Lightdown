//! Monochrome vector output for TeX formula pages. Glyphs remain paths.
use super::*;
use hayro::hayro_interpret::FillRule;

#[derive(Default)]
struct Svg { body: String, clips: usize, groups: usize, serial: usize, unsupported: bool }
impl Svg {
    fn path(&mut self, path: &BezPath, transform: Affine, mode: &DrawMode) {
        let matrix = transform.as_coeffs();
        let transform = format!("matrix({} {} {} {} {} {})", matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5]);
        let style = match mode {
            DrawMode::Invisible => return,
            DrawMode::Fill(rule) => format!("fill=\"black\" fill-rule=\"{}\"", if *rule == FillRule::EvenOdd { "evenodd" } else { "nonzero" }),
            DrawMode::Stroke(stroke) => format!("fill=\"none\" stroke=\"black\" stroke-width=\"{}\"", stroke.line_width),
            DrawMode::FillAndStroke(_, stroke) => format!("fill=\"black\" stroke=\"black\" stroke-width=\"{}\"", stroke.line_width),
        };
        self.body += &format!("<path d=\"{}\" transform=\"{transform}\" {style}/>", path.to_svg());
    }
}
impl<'a> Device<'a> for Svg {
    fn draw_path(&mut self, path: &BezPath, props: DrawProps<'a>, mode: &DrawMode) { self.path(path, props.transform, mode); }
    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, mode: &DrawMode) {
        for glyph in run.glyphs() {
            match &**glyph {
                Glyph::Outline(g) => self.path(&g.outline(), props.transform * glyph.transform(), mode),
                Glyph::Type3(g) => g.interpret(self, props.transform, glyph.transform(), &props.paint),
            }
        }
    }
    fn push_clip_path(&mut self, clip: &ClipPath) {
        self.serial += 1; self.clips += 1;
        self.body += &format!("<defs><clipPath id=\"c{}\"><path d=\"{}\"/></clipPath></defs><g clip-path=\"url(#c{})\">", self.serial, clip.path.to_svg(), self.serial);
    }
    fn pop_clip(&mut self) { if self.clips > 0 { self.body += "</g>"; self.clips -= 1; } }
    fn push_transparency_group(&mut self, opacity: f32, mask: Option<SoftMask<'a>>, _: BlendMode) {
        self.unsupported |= mask.is_some(); self.groups += 1; self.body += &format!("<g opacity=\"{opacity}\">");
    }
    fn pop_transparency_group(&mut self) { if self.groups > 0 { self.body += "</g>"; self.groups -= 1; } }
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) { self.unsupported = true; }
}
pub fn formula_svg(key: &Path) -> Option<String> {
    let (path, _, page) = parse(key)?;
    let pdf = load(&path).ok()?;
    let page = pdf.pages().get(page)?;
    let (w, h) = page.render_dimensions();
    let cache = InterpreterCache::new();
    let settings = InterpreterSettings { render_annotations: false, ..Default::default() };
    let mut context = Context::new(page.initial_transform(true).to_kurbo(), Rect::new(0.0, 0.0, w as f64, h as f64), &cache, page.xref(), settings);
    let mut svg = Svg::default();
    interpret_page(page, &mut context, &mut svg);
    if svg.unsupported { return None; }
    Some(format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}pt\" height=\"{h}pt\" viewBox=\"0 0 {w} {h}\">{}</svg>", svg.body))
}
