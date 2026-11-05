//! Rows and columns of editor views, nested. The leaves are views; an inner
//! node lays its children side by side, or one above another, each with its
//! share of the room. Shares add up to one, so they survive window resizes.

use serde_json::{Value, json};

pub type Rect = (f32, f32, f32, f32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    /// Whether views meet one above another in this direction.
    pub fn stacked(self) -> bool {
        matches!(self, Dir::Up | Dir::Down)
    }

    fn before(self) -> bool {
        matches!(self, Dir::Left | Dir::Up)
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Node<T> {
    Leaf(T),
    /// Children side by side, or one above another when `stacked`.
    Split {
        stacked: bool,
        kids: Vec<(f32, Node<T>)>,
    },
}

#[derive(Clone, PartialEq, Debug)]
pub struct Tree<T> {
    pub root: Node<T>,
}

/// The hairline between child `index` and the next one of the split at `path`.
#[derive(Clone, PartialEq, Debug)]
pub struct Divider {
    pub path: Vec<usize>,
    pub index: usize,
    pub stacked: bool,
    pub rect: Rect,
}

/// Where each child of a split goes, with `gap` between neighbours.
fn kid_rects(stacked: bool, shares: &[f32], r: Rect, gap: f32) -> Vec<Rect> {
    let (x, y, w, h) = r;
    let n = shares.len();
    let length = if stacked { h } else { w };
    let room = (length - gap * n.saturating_sub(1) as f32).max(0.0);
    let (mut cum, mut start) = (0.0, 0.0);
    let mut out = Vec::with_capacity(n);
    for (i, share) in shares.iter().enumerate() {
        cum += share;
        let end = if i + 1 == n { room } else { (room * cum).round().clamp(start, room) };
        let at = start + i as f32 * gap;
        let len = end - start;
        out.push(if stacked { (x, y + at, w, len) } else { (x + at, y, len, h) });
        start = end;
    }
    out
}

fn shares<T>(kids: &[(f32, Node<T>)]) -> Vec<f32> {
    kids.iter().map(|k| k.0).collect()
}

/// Folds away splits of one child, merges a split into a parent laid out
/// the same way and makes the shares add up to one.
fn tidy<T>(node: Node<T>) -> Node<T> {
    let Node::Split { stacked, kids } = node else { return node };
    let mut out = Vec::new();
    for (share, kid) in kids {
        match tidy(kid) {
            Node::Split { kids: inner, .. } if inner.is_empty() => {}
            Node::Split { stacked: s, kids: inner } if s == stacked => {
                let sum: f32 = inner.iter().map(|k| k.0).sum();
                for (part, k) in inner {
                    out.push((share * part / sum.max(f32::EPSILON), k));
                }
            }
            kid => out.push((share, kid)),
        }
    }
    if out.len() == 1 {
        return out.pop().unwrap().1;
    }
    let sum: f32 = out.iter().map(|k| k.0).sum();
    let even = sum.is_nan() || sum <= 0.0 || out.iter().any(|k| !k.0.is_finite() || k.0 <= 0.0);
    let n = out.len() as f32;
    for k in &mut out {
        k.0 = if even { 1.0 / n } else { k.0 / sum };
    }
    Node::Split { stacked, kids: out }
}

fn collect<T: Copy>(node: &Node<T>, out: &mut Vec<T>) {
    match node {
        Node::Leaf(t) => out.push(*t),
        Node::Split { kids, .. } => kids.iter().for_each(|(_, k)| collect(k, out)),
    }
}

fn each_leaf<T>(node: &mut Node<T>, f: &mut impl FnMut(&mut T)) {
    match node {
        Node::Leaf(t) => f(t),
        Node::Split { kids, .. } => kids.iter_mut().for_each(|(_, k)| each_leaf(k, f)),
    }
}

fn find<T: PartialEq>(node: &Node<T>, t: &T, path: &mut Vec<usize>) -> bool {
    match node {
        Node::Leaf(l) => l == t,
        Node::Split { kids, .. } => kids.iter().enumerate().any(|(i, (_, k))| {
            path.push(i);
            find(k, t, path) || {
                path.pop();
                false
            }
        }),
    }
}

fn lay<T: Copy>(
    node: &Node<T>,
    r: Rect,
    gap: f32,
    path: &mut Vec<usize>,
    leaves: &mut Vec<(T, Rect)>,
    dividers: &mut Vec<Divider>,
) {
    match node {
        Node::Leaf(t) => leaves.push((*t, r)),
        Node::Split { stacked, kids } => {
            let rects = kid_rects(*stacked, &shares(kids), r, gap);
            for (i, (kid, kr)) in kids.iter().map(|k| &k.1).zip(&rects).enumerate() {
                path.push(i);
                lay(kid, *kr, gap, path, leaves, dividers);
                path.pop();
                if i + 1 < kids.len() {
                    let d = if *stacked { (r.0, kr.1 + kr.3, r.2, gap) } else { (kr.0 + kr.2, r.1, gap, r.3) };
                    dividers.push(Divider { path: path.clone(), index: i, stacked: *stacked, rect: d });
                }
            }
        }
    }
}

fn node_json<T>(node: &Node<T>, leaf: &impl Fn(&T) -> Option<Value>) -> Option<Value> {
    match node {
        Node::Leaf(t) => leaf(t),
        Node::Split { stacked, kids } => {
            let kids: Vec<Value> = kids
                .iter()
                .filter_map(|(share, k)| Some(json!({ "share": share, "view": node_json(k, leaf)? })))
                .collect();
            (!kids.is_empty()).then(|| json!({ "stacked": stacked, "kids": kids }))
        }
    }
}

fn node_from<T>(v: &Value, leaf: &mut impl FnMut(&Value) -> Option<T>) -> Option<Node<T>> {
    let Some(kids) = v.get("kids").and_then(Value::as_array) else { return leaf(v).map(Node::Leaf) };
    let stacked = v["stacked"].as_bool().unwrap_or(false);
    let kids: Vec<(f32, Node<T>)> = kids
        .iter()
        .filter_map(|k| Some((k["share"].as_f64().unwrap_or(0.0) as f32, node_from(&k["view"], leaf)?)))
        .collect();
    (!kids.is_empty()).then_some(Node::Split { stacked, kids })
}

impl<T: Copy + PartialEq> Tree<T> {
    pub fn new(leaf: T) -> Tree<T> {
        Tree { root: Node::Leaf(leaf) }
    }

    /// Views laid out in one row, or one column when `stacked`.
    pub fn even(leaves: &[T], stacked: bool) -> Option<Tree<T>> {
        let n = leaves.len() as f32;
        let kids = leaves.iter().map(|t| (1.0 / n, Node::Leaf(*t))).collect::<Vec<_>>();
        if kids.is_empty() {
            return None;
        }
        Some(Tree { root: tidy(Node::Split { stacked, kids }) })
    }

    /// The views from left to right and top to bottom.
    pub fn leaves(&self) -> Vec<T> {
        let mut out = Vec::new();
        collect(&self.root, &mut out);
        out
    }

    pub fn len(&self) -> usize {
        self.leaves().len()
    }

    pub fn contains(&self, t: T) -> bool {
        self.leaves().contains(&t)
    }

    fn path_of(&self, t: T) -> Option<Vec<usize>> {
        let mut path = Vec::new();
        find(&self.root, &t, &mut path).then_some(path)
    }

    fn node_mut(&mut self, path: &[usize]) -> Option<&mut Node<T>> {
        let mut node = &mut self.root;
        for &i in path {
            let Node::Split { kids, .. } = node else { return None };
            node = &mut kids.get_mut(i)?.1;
        }
        Some(node)
    }

    fn retidy(&mut self) {
        let root = std::mem::replace(&mut self.root, Node::Split { stacked: false, kids: Vec::new() });
        self.root = tidy(root);
    }

    /// Puts `new` where `old` is.
    pub fn replace(&mut self, old: T, new: T) -> bool {
        let mut done = false;
        each_leaf(&mut self.root, &mut |t| {
            if *t == old && !done {
                *t = new;
                done = true;
            }
        });
        done
    }

    /// Trades the places of two views.
    pub fn swap(&mut self, a: T, b: T) -> bool {
        if a == b || !self.contains(a) || !self.contains(b) {
            return false;
        }
        each_leaf(&mut self.root, &mut |t| {
            if *t == a {
                *t = b
            } else if *t == b {
                *t = a
            }
        });
        true
    }

    /// Puts `new` beside `at`, on the side `dir`, halving `at`'s room.
    pub fn split(&mut self, at: T, new: T, dir: Dir) -> bool {
        let Some(path) = self.path_of(at) else { return false };
        if let Some((&last, parent)) = path.split_last()
            && let Some(Node::Split { stacked, kids }) = self.node_mut(parent)
            && *stacked == dir.stacked()
        {
            let share = kids[last].0 / 2.0;
            kids[last].0 = share;
            kids.insert(if dir.before() { last } else { last + 1 }, (share, Node::Leaf(new)));
            return true;
        }
        let Some(node) = self.node_mut(&path) else { return false };
        let old = std::mem::replace(node, Node::Leaf(new));
        let pair = (0.5, Node::Leaf(new));
        let kids = if dir.before() { vec![pair, (0.5, old)] } else { vec![(0.5, old), pair] };
        *node = Node::Split { stacked: dir.stacked(), kids };
        true
    }

    /// Takes a view away; its room goes to the views beside it. The last
    /// view stays.
    pub fn remove(&mut self, t: T) -> bool {
        let Some(path) = self.path_of(t) else { return false };
        let Some((&last, parent)) = path.split_last() else { return false };
        if let Some(Node::Split { kids, .. }) = self.node_mut(parent) {
            kids.remove(last);
        }
        self.retidy();
        true
    }

    /// Keeps only the views `keep` allows, or nothing if none is left.
    pub fn retain(mut self, keep: impl Fn(T) -> bool) -> Option<Tree<T>> {
        for t in self.leaves() {
            if !keep(t) {
                if self.len() == 1 {
                    return None;
                }
                self.remove(t);
            }
        }
        Some(self)
    }

    pub fn layout(&self, area: Rect, gap: f32) -> (Vec<(T, Rect)>, Vec<Divider>) {
        let (mut leaves, mut dividers) = (Vec::new(), Vec::new());
        lay(&self.root, area, gap, &mut Vec::new(), &mut leaves, &mut dividers);
        (leaves, dividers)
    }

    /// Where the split at `path` is drawn.
    fn node_rect(&self, path: &[usize], area: Rect, gap: f32) -> Rect {
        let (mut node, mut r) = (&self.root, area);
        for &i in path {
            let Node::Split { stacked, kids } = node else { break };
            r = kid_rects(*stacked, &shares(kids), r, gap)[i];
            node = &kids[i].1;
        }
        r
    }

    /// Moves divider `index` of the split at `path` to `pos` along the
    /// split, keeping each of the two views at least `min` long.
    pub fn drag(&mut self, path: &[usize], index: usize, pos: f32, area: Rect, gap: f32, min: f32) {
        let r = self.node_rect(path, area, gap);
        let Some(Node::Split { stacked, kids }) = self.node_mut(path) else { return };
        if index + 1 >= kids.len() {
            return;
        }
        let (start, length) = if *stacked { (r.1, r.3) } else { (r.0, r.2) };
        let room = (length - gap * (kids.len() - 1) as f32).max(1.0);
        let before: f32 = kids[..index].iter().map(|k| k.0).sum();
        let pair = kids[index].0 + kids[index + 1].0;
        let least = (min / room).min(pair / 2.0);
        let at = ((pos - start - gap * index as f32 - gap / 2.0) / room).clamp(before + least, before + pair - least);
        kids[index].0 = at - before;
        kids[index + 1].0 = before + pair - at;
    }

    /// Gives every child of the split at `path` the same room.
    pub fn equalize(&mut self, path: &[usize]) {
        if let Some(Node::Split { kids, .. }) = self.node_mut(path) {
            let n = kids.len() as f32;
            kids.iter_mut().for_each(|k| k.0 = 1.0 / n);
        }
    }

    /// The tree for a session file; `leaf` names each view, or leaves it out.
    pub fn to_json(&self, leaf: impl Fn(&T) -> Option<Value>) -> Option<Value> {
        node_json(&self.root, &leaf)
    }

    /// Reads what `to_json` wrote. Views `leaf` cannot find are left out.
    pub fn from_json(v: &Value, mut leaf: impl FnMut(&Value) -> Option<T>) -> Option<Tree<T>> {
        let root = node_from(v, &mut leaf)?;
        Some(Tree { root: tidy(root) })
    }
}

/// The nearest view on side `dir` of `from` that overlaps it across.
pub fn neighbour<T: Copy + PartialEq>(rects: &[(T, Rect)], from: T, dir: Dir) -> Option<T> {
    let &(_, f) = rects.iter().find(|(t, _)| *t == from)?;
    let centre = |r: Rect| (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0);
    let (fx, fy) = centre(f);
    rects
        .iter()
        .filter(|(t, _)| *t != from)
        .filter_map(|&(t, r)| {
            let (away, overlap) = match dir {
                Dir::Left => (f.0 - (r.0 + r.2), r.1 < f.1 + f.3 && r.1 + r.3 > f.1),
                Dir::Right => (r.0 - (f.0 + f.2), r.1 < f.1 + f.3 && r.1 + r.3 > f.1),
                Dir::Up => (f.1 - (r.1 + r.3), r.0 < f.0 + f.2 && r.0 + r.2 > f.0),
                Dir::Down => (r.1 - (f.1 + f.3), r.0 < f.0 + f.2 && r.0 + r.2 > f.0),
            };
            let (cx, cy) = centre(r);
            let across = if dir.stacked() { (cx - fx).abs() } else { (cy - fy).abs() };
            (away >= -0.5 && overlap).then_some((away, across, t))
        })
        .min_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(.., t)| t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = (0.0, 0.0, 1001.0, 601.0);

    #[test]
    fn splits_nest_and_removing_folds_them_back() {
        let mut t = Tree::new(1);
        assert!(t.split(1, 2, Dir::Right));
        assert!(t.split(2, 3, Dir::Down));
        assert!(t.split(1, 4, Dir::Left));
        assert_eq!(t.leaves(), [4, 1, 2, 3]);
        let (rects, dividers) = t.layout(AREA, 1.0);
        let r = |id| rects.iter().find(|(t, _)| *t == id).unwrap().1;
        // 4 and 1 share the left half; 2 is above 3 in the right half.
        assert_eq!((r(4).0, r(1).0 + r(1).2 + 1.0), (0.0, r(2).0));
        assert_eq!((r(2).0, r(2).2), (r(3).0, r(3).2));
        assert_eq!(r(2).1 + r(2).3 + 1.0, r(3).1);
        assert_eq!(r(3).1 + r(3).3, 601.0);
        assert_eq!(dividers.len(), 3);
        assert!(dividers.iter().any(|d| d.stacked && d.path == [2]));
        // Every pixel of the width is a view or a divider.
        assert_eq!(r(4).2 + r(1).2 + r(2).2 + 2.0, 1001.0);

        assert!(t.remove(3));
        assert_eq!(t.leaves(), [4, 1, 2]);
        // A column of one folds into the row around it.
        let Node::Split { stacked: false, kids } = &t.root else { panic!() };
        assert_eq!(kids.len(), 3);
        assert!((kids.iter().map(|k| k.0).sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(t.remove(4) && t.remove(1));
        assert_eq!(t.root, Node::Leaf(2));
        assert!(!t.remove(2));
    }

    #[test]
    fn dividers_drag_within_limits_and_reset() {
        let mut t = Tree::even(&[1, 2, 3], false).unwrap();
        let (_, dividers) = t.layout(AREA, 1.0);
        let d = &dividers[0];
        t.drag(&d.path, d.index, 100.0, AREA, 1.0, 80.0);
        let (rects, _) = t.layout(AREA, 1.0);
        assert!((rects[0].1.2 - 100.0).abs() <= 1.0, "{:?}", rects[0]);
        assert_eq!(rects[2].1.2, (999.0_f32 - (999.0_f32 * 2.0 / 3.0).round()), "the third view keeps its room");
        t.drag(&d.path, d.index, -50.0, AREA, 1.0, 80.0);
        assert!(t.layout(AREA, 1.0).0[0].1.2 >= 79.0);
        t.equalize(&d.path);
        let widths: Vec<f32> = t.layout(AREA, 1.0).0.iter().map(|r| r.1.2).collect();
        assert_eq!(widths, [333.0, 333.0, 333.0]);
    }

    #[test]
    fn neighbours_are_found_by_place() {
        let mut t = Tree::new(1);
        t.split(1, 2, Dir::Right);
        t.split(2, 3, Dir::Down);
        let (rects, _) = t.layout(AREA, 1.0);
        assert_eq!(neighbour(&rects, 1, Dir::Right), Some(2));
        assert_eq!(neighbour(&rects, 3, Dir::Left), Some(1));
        assert_eq!(neighbour(&rects, 3, Dir::Up), Some(2));
        assert_eq!(neighbour(&rects, 2, Dir::Down), Some(3));
        assert_eq!(neighbour(&rects, 1, Dir::Up), None);
        assert!(t.swap(1, 3));
        assert_eq!(t.leaves(), [3, 2, 1]);
    }

    #[test]
    fn the_tree_and_its_shares_survive_a_session_file() {
        let mut t = Tree::new(1);
        t.split(1, 2, Dir::Down);
        t.split(2, 3, Dir::Right);
        let (_, d) = t.layout(AREA, 1.0);
        t.drag(&d[0].path, d[0].index, 150.0, AREA, 1.0, 10.0);
        let v = t.to_json(|n| Some(json!(n))).unwrap();
        let back = Tree::from_json(&v, |v| v.as_u64().map(|n| n as i32)).unwrap();
        assert_eq!(back.layout(AREA, 1.0), t.layout(AREA, 1.0));
        // A view whose file is gone is left out, and its room shared.
        let fewer = Tree::from_json(&v, |v| v.as_u64().map(|n| n as i32).filter(|n| *n != 2)).unwrap();
        assert_eq!(fewer.leaves(), [1, 3]);
        assert!(matches!(fewer.root, Node::Split { stacked: true, .. }));
        assert!(Tree::<i32>::from_json(&v, |_| None).is_none());
        assert!(t.clone().retain(|n| n == 3).is_some_and(|t| t.root == Node::Leaf(3)));
    }
}
