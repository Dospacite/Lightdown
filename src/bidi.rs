//! Unicode bidi levels stay separate from logical source offsets and shaping.
use std::ops::Range;
use unicode_bidi::{BidiClass, BidiInfo, Level};

/// Most lines avoid allocating a bidi paragraph, including non-ASCII LTR text.
pub fn needed(text: &str) -> bool {
    if text.is_ascii() {
        return false;
    }
    text.chars().any(|c| {
        c >= '\u{590}'
            && matches!(
                unicode_bidi::bidi_class(c),
                BidiClass::R | BidiClass::AL | BidiClass::RLE | BidiClass::RLO | BidiClass::RLI
            )
    })
}

pub fn first_strong(text: &str) -> Option<bool> {
    // Markdown soft breaks stay inside one bidi paragraph. U+2028 keeps
    // their whitespace role without starting a new UAX #9 paragraph.
    let text = text.replace(['\n', '\r'], "\u{2028}");
    BidiInfo::new(&text, None).paragraphs.first().map(|p| p.level.is_rtl())
}

pub struct Paragraph {
    text: String,
    pub levels: Vec<Level>,
    pub rtl: bool,
    base: Option<bool>,
}
impl Paragraph {
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn new(text: &str) -> Option<Self> {
        if !needed(text) {
            return None;
        }
        Self::with_base(text, None)
    }
    pub fn with_base(text: &str, base: Option<bool>) -> Option<Self> {
        if base.is_none() && !needed(text) {
            return None;
        }
        let info = BidiInfo::new(text, base.map(|rtl| if rtl { Level::rtl() } else { Level::ltr() }));
        Some(Self {
            text: text.into(),
            levels: info.levels.clone(),
            base,
            rtl: info.paragraphs.first().is_some_and(|p| p.level.is_rtl()),
        })
    }
    /// L1 resets trailing whitespace separately for every wrapped display row.
    pub fn order(&self, range: Range<usize>, starts: &[usize]) -> (Vec<usize>, Vec<bool>) {
        let info = BidiInfo::new(&self.text, self.base.map(|rtl| if rtl { Level::rtl() } else { Level::ltr() }));
        let para = info.paragraphs.iter().find(|p| p.range.contains(&range.start)).or(info.paragraphs.first());
        let levels = para.map_or_else(|| self.levels.clone(), |p| info.reordered_levels(p, range));
        let levels: Vec<Level> = starts.iter().map(|&i| levels.get(i).copied().unwrap_or(Level::ltr())).collect();
        (BidiInfo::reorder_visual(&levels), levels.iter().map(|l| l.is_rtl()).collect())
    }
}

/// UAX #9 L4 mirroring before shaping, with original source byte ranges.
pub fn shape_logical(
    fonts: &mut crate::font::Fonts,
    face: u8,
    size: f32,
    text: &str,
    rtl: bool,
    mut emit: impl FnMut(usize, usize, f32, &[crate::font::Shaped]),
) {
    use swash::text::Codepoint;
    if !rtl || !text.chars().any(|c| c.mirror().is_some()) {
        return fonts.shape(face, size, text, emit);
    }
    let mut display = String::new();
    let mut offsets = Vec::new();
    for (at, c) in text.char_indices() {
        let mirrored = c.mirror().unwrap_or(c);
        offsets.resize(display.len() + mirrored.len_utf8(), at);
        display.push(mirrored);
    }
    offsets.push(text.len());
    fonts.shape(face, size, &display, |s, e, w, g| emit(offsets[s], offsets[e], w, g));
}

/// Shape logical runs first, then place their clusters in Unicode visual order.
pub fn shape(
    fonts: &mut crate::font::Fonts,
    face: u8,
    size: f32,
    text: &str,
    mut emit: impl FnMut(usize, usize, f32, &[crate::font::Shaped]),
) {
    let Some(p) = Paragraph::new(text) else {
        return fonts.shape(face, size, text, emit);
    };
    let mut clusters = Vec::new();
    // Split directional runs before shaping so Arabic adjoining Latin uses its own script.
    let mut a = 0;
    while a < text.len() {
        let mut b = a + text[a..].chars().next().unwrap().len_utf8();
        while b < text.len() && p.levels[b] == p.levels[a] {
            b += text[b..].chars().next().unwrap().len_utf8();
        }
        shape_logical(fonts, face, size, &text[a..b], p.levels[a].is_rtl(), |s, e, w, g| {
            clusters.push((
                a + s,
                a + e,
                w,
                g.iter().map(|v| crate::font::Shaped { face: v.face, id: v.id, x: v.x, y: v.y }).collect::<Vec<_>>(),
            ))
        });
        a = b;
    }
    let starts: Vec<_> = clusters.iter().map(|c| c.0).collect();
    let (order, _) = p.order(0..text.len(), &starts);
    for i in order {
        let (s, e, w, g) = &clusters[i];
        emit(*s, *e, *w, g);
    }
}

pub fn hit(fonts: &mut crate::font::Fonts, face: u8, size: f32, text: &str, x: f32) -> usize {
    let p = Paragraph::new(text);
    let mut pen = 0.0;
    let mut best = (f32::MAX, 0);
    let mut inside = None;
    shape(fonts, face, size, text, |s, e, w, _| {
        let piece = &text[s..e];
        let count = piece.chars().count().max(1) as f32;
        let rtl = p.as_ref().is_some_and(|p| p.levels[s].is_rtl());
        for (i, at) in piece.char_indices().map(|c| c.0).chain(std::iter::once(piece.len())).enumerate() {
            let f = i as f32 / count;
            let dx = (pen + w * if rtl { 1.0 - f } else { f } - x).abs();
            if dx < best.0 {
                best = (dx, s + at);
            }
            if x >= pen && x < pen + w && inside.is_none_or(|v: (f32, usize)| dx < v.0) {
                inside = Some((dx, s + at));
            }
        }
        pen += w;
    });
    inside.unwrap_or(best).1
}

pub fn caret_x(fonts: &mut crate::font::Fonts, face: u8, size: f32, text: &str, at: usize) -> f32 {
    let Some(p) = Paragraph::new(text) else {
        return fonts.width(face, size, &text[..at.min(text.len())]);
    };
    let mut x = 0.0;
    let mut result = None;
    let mut trailing = None;
    shape(fonts, face, size, text, |s, e, w, _| {
        let rtl = p.levels[s].is_rtl();
        if s <= at && at < e {
            let piece = &text[s..e];
            let count = piece.chars().count().max(1) as f32;
            let n = text[s..crate::editor::floor(text, at)].chars().count() as f32 / count;
            result = Some(x + w * if rtl { 1.0 - n } else { n });
        }
        if at == e {
            trailing = Some(x + if rtl { 0.0 } else { w });
        }
        x += w;
    });
    result.or(trailing).unwrap_or(x)
}

/// Visual character-boundary order for fields that do not have a cached layout.
pub fn visual_move(text: &str, cur: usize, right: bool) -> usize {
    let Some(p) = Paragraph::new(text) else {
        return if right { crate::editor::next(text, cur) } else { crate::editor::prev(text, cur) };
    };
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let starts: Vec<usize> = chars.iter().map(|c| c.0).collect();
    let (order, rtl) = p.order(0..text.len(), &starts);
    let mut stops = Vec::new();
    let mut trailing = None;
    for (x, i) in order.into_iter().enumerate() {
        let (a, c) = chars[i];
        let b = a + c.len_utf8();
        stops.push((a, x + usize::from(rtl[i])));
        if b == text.len() {
            trailing = Some((b, x + usize::from(!rtl[i])));
        }
    }
    stops.extend(trailing);
    stops.sort_by_key(|s| (s.1, s.0));
    let i = stops.iter().position(|s| s.0 == cur).unwrap_or(0);
    if right { stops.get(i + 1).map_or(cur, |s| s.0) } else { stops.get(i.wrapping_sub(1)).map_or(cur, |s| s.0) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_levels_preserve_numbers_and_logical_text() {
        for (text, expected) in [("abc אבג 123", "abc 123 גבא"), ("אבג", "גבא"), ("سلام", "مالس")] {
            let p = Paragraph::new(text).unwrap();
            let chars: Vec<_> = text.char_indices().collect();
            let starts: Vec<_> = chars.iter().map(|c| c.0).collect();
            let (order, _) = p.order(0..text.len(), &starts);
            let visual: String = order.iter().map(|&i| chars[i].1).collect();
            assert_eq!(visual, expected);
            assert_eq!(p.text, text);
        }
        assert!(Paragraph::new("café 日本語").is_none());
    }
    #[test]
    fn rtl_field_arrows_follow_visual_order() {
        let s = "אבג";
        assert_eq!(visual_move(s, 0, false), 2);
        assert_eq!(visual_move(s, 2, false), 4);
        assert_eq!(visual_move(s, 4, true), 2);
        assert_eq!(visual_move("سلام", 0, false), 2);
    }
    fn pane(text: &str) -> crate::pane::Pane {
        use std::{cell::RefCell, rc::Rc};
        let mut p = crate::pane::Pane::new(
            Rc::new(RefCell::new(crate::font::Fonts::new())),
            Rc::new(RefCell::new(crate::pics::Pics::new())),
        );
        p.set_rect(0.0, 0.0, 1200.0, 700.0, 1.0);
        p.set_text(text.into(), crate::pane::Doc::Text);
        p
    }
    #[test]
    fn bidi_layout_hits_logical_offsets_and_splits_selections() {
        let mut p = pane("abc אבג def");
        let l = p.layout(0);
        let rtl: Vec<_> = l.clusters.iter().filter(|c| c.rtl).collect();
        assert_eq!(rtl.iter().map(|c| c.start).collect::<Vec<_>>(), vec![8, 6, 4]);
        for c in &l.clusters {
            let r = &l.rows[c.row as usize];
            let expected = if c.rtl { c.end } else { c.start };
            assert_eq!(l.hit(c.x + c.w * 0.2, r.y + r.h / 2.0), expected, "offset {}", c.start);
        }
        assert_eq!(l.selection_spans(0, 0, 6).len(), 2);
        assert!(l.carets.iter().all(|c| p.buf.text.is_char_boundary(c.0 as usize)));
        assert_eq!(p.buf.text, "abc אבג def");
    }
    #[test]
    fn bidi_editor_arrows_and_logical_home_end() {
        use crate::app::{Key, Mods};
        let mut p = pane("אבג");
        p.select(0, 0);
        let x = p.caret().unwrap().0;
        p.key(Key::Left, Mods::default(), None);
        assert_eq!(p.buf.cur, 2);
        assert!(p.caret().unwrap().0 < x);
        p.key(Key::Right, Mods::default(), None);
        assert_eq!(p.buf.cur, 0);
        p.key(Key::End, Mods::default(), None);
        assert_eq!(p.buf.cur, 6);
        p.key(Key::Home, Mods::default(), None);
        assert_eq!(p.buf.cur, 0);
        let mut ltr = pane("ordinary café");
        assert!(ltr.layout(0).carets.is_empty());
    }
    #[test]
    fn bidi_arabic_is_contextually_shaped_by_swash() {
        let mut fonts = crate::font::Fonts::new();
        let mut joined = Vec::new();
        fonts.shape(crate::font::UI, 24.0, "سلام", |_, _, _, g| joined.extend(g.iter().map(|g| (g.face, g.id))));
        let mut isolated = Vec::new();
        for c in "سلام".chars() {
            fonts.shape(crate::font::UI, 24.0, &c.to_string(), |_, _, _, g| {
                isolated.extend(g.iter().map(|g| (g.face, g.id)))
            });
        }
        assert!(!joined.is_empty());
        assert_ne!(joined, isolated, "Arabic needs a font with contextual joining");
        let mut p = pane("مرحبا بالعالم 123");
        let l = p.layout(0);
        assert!(l.clusters.iter().any(|c| c.rtl));
        assert!(l.glyphs.iter().all(|g| g.id != 0));
    }
    #[test]
    fn bidi_mixed_arrows_visit_each_primary_caret_without_loops() {
        let mut p = pane("abc אבג def");
        let l = p.layout(0);
        let mut at = 0;
        let mut visited = vec![at];
        for _ in 0..30 {
            let next = l.visual_move(at, true).unwrap();
            if next == at {
                break;
            }
            assert!(!visited.contains(&next));
            assert_eq!(l.visual_move(next, false), Some(at));
            visited.push(next);
            at = next;
        }
        assert_eq!(visited.len(), "abc אבג def".chars().count() + 1);
    }
    #[test]
    fn bidi_markdown_soft_lines_share_paragraph_direction() {
        let mut p = pane("שלום first\ncontinuation 123\n\nEnglish אבג\nאבג");
        p.set_doc(crate::pane::Doc::Markdown);
        let md = crate::md::parse(&p.buf.text, &p.buf.starts);
        assert_eq!(md.lines[0].bidi_base, Some(true));
        assert_eq!(md.lines[1].bidi_base, Some(true));
        assert_eq!(md.lines[3].bidi_base, Some(false));
        assert_eq!(md.lines[4].bidi_base, Some(false));
        let line = p.layout(1);
        assert!(!line.carets.is_empty());
        assert!(line.rows[0].text_x > 0.0);
    }
}
