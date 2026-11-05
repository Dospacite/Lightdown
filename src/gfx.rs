//! Software canvas over an XRGB8888 pixel buffer.

use crate::font::Bitmap;
use crate::pics::Pic;

pub struct Canvas<'a> {
    pub px: &'a mut [u32],
    pub w: i32,
    pub h: i32,
    /// Nothing is drawn outside this rectangle: left, top, right, bottom.
    clip: (i32, i32, i32, i32),
}

/// Coverage ramps for glyph masks. Light text on a dark ground reads thinner
/// than it is, so coverage is lifted slightly; dark text on a light ground
/// is left as it is.
static RAMPS: std::sync::OnceLock<[[u8; 256]; 2]> = std::sync::OnceLock::new();

fn ramp() -> &'static [u8; 256] {
    let ramps = RAMPS.get_or_init(|| {
        let mut t = [[0u8; 256]; 2];
        for (gamma, ramp) in [0.82, 1.0].into_iter().zip(&mut t) {
            for (i, v) in ramp.iter_mut().enumerate() {
                *v = ((i as f32 / 255.0).powf(gamma) * 255.0 + 0.5) as u8;
            }
        }
        t
    });
    &ramps[crate::theme::light() as usize]
}

#[inline]
fn mix(dst: u32, src: u32, a: u32) -> u32 {
    let ia = 255 - a;
    let rb = ((src & 0xff00ff) * a + (dst & 0xff00ff) * ia + 0x800080) >> 8 & 0xff00ff;
    let g = ((src & 0x00ff00) * a + (dst & 0x00ff00) * ia + 0x008000) >> 8 & 0x00ff00;
    rb | g
}

impl<'a> Canvas<'a> {
    pub fn new(px: &'a mut [u32], w: i32, h: i32) -> Canvas<'a> {
        Canvas { px, w, h, clip: (0, 0, w, h) }
    }

    pub fn clear(&mut self, color: u32) {
        self.px.fill(color);
    }

    /// Restricts drawing to a rectangle, until `unclip`.
    pub fn clip(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let x0 = (x.round() as i32).clamp(0, self.w);
        let y0 = (y.round() as i32).clamp(0, self.h);
        self.clip = (x0, y0, ((x + w).round() as i32).clamp(x0, self.w), ((y + h).round() as i32).clamp(y0, self.h));
    }

    pub fn unclip(&mut self) {
        self.clip = (0, 0, self.w, self.h);
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32) {
        let (cx0, cy0, cx1, cy1) = self.clip;
        let x0 = (x.round() as i32).clamp(cx0, cx1);
        let x1 = ((x + w).round() as i32).clamp(cx0, cx1);
        let y0 = (y.round() as i32).clamp(cy0, cy1);
        let y1 = ((y + h).round() as i32).clamp(cy0, cy1);
        for row in y0..y1 {
            let o = (row * self.w) as usize;
            self.px[o + x0 as usize..o + x1 as usize].fill(color);
        }
    }

    /// A rectangle at least one device pixel thick, for hairlines.
    pub fn line(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32) {
        let (x, y) = (x.round(), y.round());
        self.rect(x, y, w.round().max(1.0), h.round().max(1.0), color);
    }

    pub fn frame(&mut self, x: f32, y: f32, w: f32, h: f32, t: f32, color: u32) {
        self.line(x, y, w, t, color);
        self.line(x, y + h - t, w, t, color);
        self.line(x, y, t, h, color);
        self.line(x + w - t, y, t, h, color);
    }

    /// Draws an image into a rectangle, stretching it if the sizes differ.
    pub fn image(&mut self, x: i32, y: i32, w: i32, h: i32, pic: &Pic) {
        if w <= 0 || h <= 0 || pic.w == 0 || pic.h == 0 {
            return;
        }
        let (cx0, cy0, cx1, cy1) = self.clip;
        let (x0, x1) = (x.max(cx0), (x + w).min(cx1));
        let (y0, y1) = (y.max(cy0), (y + h).min(cy1));
        if x0 >= x1 {
            return;
        }
        if !pic.rgb.is_empty() {
            for row in y0..y1 {
                let sy = ((row - y) as i64 * pic.h as i64 / h as i64) as usize;
                let src = &pic.rgb[sy * pic.w as usize..];
                let line = &mut self.px[(row * self.w + x0) as usize..(row * self.w + x1) as usize];
                for (i, p) in line.iter_mut().enumerate() {
                    *p = src[(((x0 - x) as i64 + i as i64) * pic.w as i64 / w as i64) as usize];
                }
            }
            return;
        }
        if !pic.rgba.is_empty() {
            let rgba = pic.shown();
            for row in y0..y1 {
                let sy = ((row - y) as i64 * pic.h as i64 / h as i64) as usize;
                let line = &mut self.px[(row * self.w + x0) as usize..(row * self.w + x1) as usize];
                for (i, p) in line.iter_mut().enumerate() {
                    let sx = (((x0 - x) as i64 + i as i64) * pic.w as i64 / w as i64) as usize;
                    let rgba = &rgba[(sy * pic.w as usize + sx) * 4..];
                    let alpha = rgba[3] as u32;
                    let component = |c: usize, shift: u32| {
                        (rgba[c] as u32 + ((*p >> shift & 255u32) * (255 - alpha) + 127) / 255).min(255) << shift
                    };
                    *p = component(0, 16) | component(1, 8) | component(2, 0);
                }
            }
            return;
        }
        for row in y0..y1 {
            let sy = ((row - y) as i64 * pic.h as i64 / h as i64) as usize;
            let src = &pic.la[sy * pic.w as usize * 2..];
            let line = &mut self.px[(row * self.w + x0) as usize..(row * self.w + x1) as usize];
            for (i, p) in line.iter_mut().enumerate() {
                let sx = ((x0 - x) as i64 + i as i64) * pic.w as i64 / w as i64;
                let (l, a) = (src[sx as usize * 2] as u32, src[sx as usize * 2 + 1] as u32);
                // Luminance is premultiplied: add what shows through.
                let under = if a == 255 { 0 } else { mix(*p, 0, a) };
                *p = under + (l << 16 | l << 8 | l);
            }
        }
    }

    /// Lays a colour over a rectangle, letting what is under it show
    /// through: `alpha` of 255 covers it.
    pub fn shade(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32, alpha: u32) {
        let (cx0, cy0, cx1, cy1) = self.clip;
        let x0 = (x.round() as i32).clamp(cx0, cx1);
        let x1 = ((x + w).round() as i32).clamp(cx0, cx1);
        let y0 = (y.round() as i32).clamp(cy0, cy1);
        let y1 = ((y + h).round() as i32).clamp(cy0, cy1);
        for row in y0..y1 {
            let o = (row * self.w) as usize;
            for p in &mut self.px[o + x0 as usize..o + x1 as usize] {
                *p = mix(*p, color, alpha);
            }
        }
    }

    /// Coverage of a pixel by a rounded rectangle, from its distance to the
    /// nearest corner centre; 255 everywhere but the corners.
    fn corner_cover(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> u32 {
        let cx = px.clamp(x + r, x + w - r);
        let cy = py.clamp(y + r, y + h - r);
        let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
        ((r - d + 0.5).clamp(0.0, 1.0) * 255.0) as u32
    }

    /// Visits the pixels of the four `r`-square corners of a rectangle with
    /// how much of each the rounded rectangle covers.
    fn each_corner(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, mut f: impl FnMut(&mut u32, u32)) {
        let (x, y, w, h) = (x.round(), y.round(), w.round(), h.round());
        let r = r.round().min(w / 2.0).min(h / 2.0).max(0.0);
        if r < 1.0 {
            return;
        }
        let (cx0, cy0, cx1, cy1) = self.clip;
        for (ox, oy) in [(x, y), (x + w - r, y), (x, y + h - r), (x + w - r, y + h - r)] {
            for row in (oy as i32).max(cy0)..((oy + r) as i32).min(cy1) {
                for col in (ox as i32).max(cx0)..((ox + r) as i32).min(cx1) {
                    let cover = Self::corner_cover(col as f32 + 0.5, row as f32 + 0.5, x, y, w, h, r);
                    f(&mut self.px[(row * self.w + col) as usize], cover);
                }
            }
        }
    }

    /// A filled rectangle with corners rounded to `r`, smoothed at the edge.
    pub fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: u32) {
        let (x, y, w, h) = (x.round(), y.round(), w.round(), h.round());
        let r = r.round().min(w / 2.0).min(h / 2.0).max(0.0);
        if r < 1.0 {
            self.rect(x, y, w, h, color);
            return;
        }
        self.rect(x + r, y, w - r * 2.0, h, color);
        self.rect(x, y + r, r, h - r * 2.0, color);
        self.rect(x + w - r, y + r, r, h - r * 2.0, color);
        self.each_corner(x, y, w, h, r, |p, cover| {
            if cover > 0 {
                *p = mix(*p, color, cover);
            }
        });
    }

    /// How much of a pixel, by its centre, a rounded rectangle covers, from
    /// 0 to 1. Straight edges lie on whole pixels; corners are smoothed.
    fn cover(px: f32, py: f32, (x, y, w, h): (f32, f32, f32, f32), r: f32) -> f32 {
        if w <= 0.0 || h <= 0.0 || px < x || py < y || px >= x + w || py >= y + h {
            return 0.0;
        }
        let cx = px.clamp(x + r, x + w - r);
        let cy = py.clamp(y + r, y + h - r);
        let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
        (r - d + 0.5).clamp(0.0, 1.0)
    }

    /// Lays a colour over a rounded rectangle at `alpha` out of 255, letting
    /// what is under it show through. Nothing is laid inside `hole`.
    #[allow(clippy::too_many_arguments)]
    pub fn round_shade(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        color: u32,
        alpha: u32,
        hole: Option<(f32, f32, f32, f32)>,
    ) {
        let (x, y, w, h) = (x.round(), y.round(), w.round(), h.round());
        let r = r.round().min(w / 2.0).min(h / 2.0).max(0.0);
        let (cx0, cy0, cx1, cy1) = self.clip;
        let x0 = (x as i32).max(cx0);
        let x1 = ((x + w) as i32).min(cx1);
        let y0 = (y as i32).max(cy0);
        let y1 = ((y + h) as i32).min(cy1);
        if x0 >= x1 || y0 >= y1 || alpha == 0 {
            return;
        }
        let hole = hole.map(|(hx, hy, hw, hh)| {
            (hx.round() as i32, hy.round() as i32, (hx + hw).round() as i32, (hy + hh).round() as i32)
        });
        for row in y0..y1 {
            let fy = row as f32 + 0.5;
            let corner_row = fy < y + r || fy > y + h - r;
            let o = (row * self.w) as usize;
            // Where the row crosses the hole, only its two ends are laid.
            let (gap0, gap1) = match hole {
                Some((hx0, hy0, hx1, hy1)) if row >= hy0 && row < hy1 => (hx0.clamp(x0, x1), hx1.clamp(x0, x1)),
                _ => (x1, x1),
            };
            for col in (x0..gap0).chain(gap1.max(gap0)..x1) {
                let fx = col as f32 + 0.5;
                let a = if corner_row && (fx < x + r || fx > x + w - r) {
                    (Self::cover(fx, fy, (x, y, w, h), r) * alpha as f32) as u32
                } else {
                    alpha
                };
                if a > 0 {
                    let p = &mut self.px[o + col as usize];
                    *p = mix(*p, color, a);
                }
            }
        }
    }

    /// The edge of a rounded rectangle, `t` thick, over what is drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn round_ring(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, t: f32, color: u32) {
        let (x, y, w, h, t) = (x.round(), y.round(), w.round(), h.round(), t.round().max(1.0));
        let r = r.round().min(w / 2.0).min(h / 2.0).max(0.0);
        let inner = (x + t, y + t, w - t * 2.0, h - t * 2.0);
        let ri = (r - t).max(0.0);
        let (cx0, cy0, cx1, cy1) = self.clip;
        let y0 = (y as i32).max(cy0);
        let y1 = ((y + h) as i32).min(cy1);
        let x0 = (x as i32).max(cx0);
        let x1 = ((x + w) as i32).min(cx1);
        for row in y0..y1 {
            let fy = row as f32 + 0.5;
            let band = fy < y + t + r || fy > y + h - t - r;
            let o = (row * self.w) as usize;
            let visit = |col: i32, px: &mut [u32]| {
                let fx = col as f32 + 0.5;
                let a = Self::cover(fx, fy, (x, y, w, h), r) - Self::cover(fx, fy, inner, ri);
                if a > 0.0 {
                    let p = &mut px[o + col as usize];
                    *p = mix(*p, color, (a * 255.0) as u32);
                }
            };
            if band {
                for col in x0..x1 {
                    visit(col, self.px);
                }
            } else {
                let lt = ((x + t + r) as i32).min(x1);
                let rt = ((x + w - t - r) as i32).max(lt);
                for col in x0..lt {
                    visit(col, self.px);
                }
                for col in rt.max(x0)..x1 {
                    visit(col, self.px);
                }
            }
        }
    }

    /// A soft shadow under a rounded box, falling `drop` pixels below it and
    /// fading out over `blur` pixels, `strength` out of 255 at its darkest.
    #[allow(clippy::too_many_arguments)]
    pub fn shadow(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, blur: f32, drop: f32, color: u32, strength: u32) {
        let steps = blur.round().max(1.0) as i32;
        let hole = Some((x + r, y + r, w - r * 2.0, h - r * 2.0));
        for i in (1..=steps).rev() {
            let k = i as f32;
            // Each layer adds a little; where they overlap they add up to
            // the full strength, so the edge fades smoothly outward.
            let a = (strength as f32 * 2.0 / (steps + 1) as f32 * (1.0 - (k - 1.0) / steps as f32)).round() as u32;
            let a = a.clamp(1, 255);
            self.round_shade(x - k, y - k + drop, w + k * 2.0, h + k * 2.0, r + k, color, a / 2 + 1, hole);
        }
    }

    /// A filled circle.
    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, color: u32) {
        self.round_rect(cx - r, cy - r, r * 2.0, r * 2.0, r, color);
    }

    pub fn glyph(&mut self, x: i32, y: i32, bmp: &Bitmap, color: u32) {
        let ramp = ramp();
        let (gx, gy) = (x + bmp.left, y - bmp.top);
        let (cx0, cy0, cx1, cy1) = self.clip;
        let x0 = gx.max(cx0);
        let x1 = (gx + bmp.w as i32).min(cx1);
        let y0 = gy.max(cy0);
        let y1 = (gy + bmp.h as i32).min(cy1);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let step = if bmp.color { 4 } else { 1 };
        for row in y0..y1 {
            let src = ((row - gy) as usize * bmp.w as usize + (x0 - gx) as usize) * step;
            let dst = (row * self.w + x0) as usize;
            let n = (x1 - x0) as usize;
            let line = &mut self.px[dst..dst + n];
            if bmp.color {
                for (i, p) in line.iter_mut().enumerate() {
                    let c = &bmp.data[src+i*4..src+i*4+4];
                    let rgb = (c[0] as u32)<<16 | (c[1] as u32)<<8 | c[2] as u32;
                    if c[3] == 255 { *p = rgb; }
                    else if c[3] != 0 { *p = mix(*p, rgb, c[3] as u32); }
                }
            } else {
                for (p, &m) in line.iter_mut().zip(&bmp.data[src..src + n]) {
                    if m != 0 {
                        *p = mix(*p, color, ramp[m as usize] as u32);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod color_glyph_tests {
    use super::*;
    #[test]
    fn rgba_glyphs_preserve_color_alpha_and_clipping() {
        let bmp = Bitmap { left: 0, top: 0, w: 3, h: 1, color: true,
            data: vec![255,0,0,255, 0,255,0,128, 0,0,255,0].into() };
        let mut px = [0xffffff; 3];
        let mut c = Canvas::new(&mut px, 3, 1);
        c.glyph(0,0,&bmp,0);
        assert_eq!(c.px[0], 0xff0000);
        assert_eq!(c.px[1], mix(0xffffff,0x00ff00,128));
        assert_eq!(c.px[2], 0xffffff);
        c.clear(0);
        c.clip(1.0,0.0,1.0,1.0);
        c.glyph(0,0,&bmp,0xffffff);
        assert_eq!(c.px[0], 0);
        assert_ne!(c.px[1], 0);
        assert_eq!(c.px[2], 0);
    }
}
