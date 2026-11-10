//! Panes: what a tiling leaf shows, the factories that build the non-agent
//! kinds, and the split tree each tab lays its panes out with.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

/// What a pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneKind {
    /// A Gentle Shell conversation ([`crate::features::session::SessionView`]).
    Agent,
    Terminal,
    File,
}

impl PaneKind {
    pub fn icon(self) -> IconName {
        match self {
            PaneKind::Agent => IconName::Bot,
            PaneKind::Terminal => IconName::SquareTerminal,
            PaneKind::File => IconName::FileText,
        }
    }
}

/// A leaf's content, whatever its kind.
pub struct PaneItem {
    pub kind: PaneKind,
    pub view: AnyView,
    /// Focused when the pane becomes the active one.
    pub focus: FocusHandle,
    /// The header and tab label, read on every render.
    pub title: Rc<dyn Fn(&App) -> SharedString>,
    /// Whether closing the pane would lose edits (file panes); `None` never does.
    pub unsaved: Option<UnsavedProbe>,
}

/// Reads a pane's "has unsaved edits" state.
pub type UnsavedProbe = Rc<dyn Fn(&App) -> bool>;

/// Builds a pane for a folder (terminal) or a file (file viewer).
pub type PaneFactory = Rc<dyn Fn(PathBuf, &mut Window, &mut App) -> PaneItem>;

/// Pane kinds other modules provide; a `None` factory makes the actions that
/// need it show a notice instead.
#[derive(Clone, Default)]
pub struct PaneFactories {
    pub terminal: Option<PaneFactory>,
    pub file: Option<PaneFactory>,
    /// A file's changes against HEAD (saved and restored as a file pane).
    pub diff: Option<PaneFactory>,
}

impl Global for PaneFactories {}

impl PaneFactories {
    pub fn get(cx: &App) -> Self {
        cx.try_global::<Self>().cloned().unwrap_or_default()
    }
}

pub type PaneId = usize;

/// How a split arranges its two children.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    /// Side by side; the divider is vertical (split right).
    Horizontal,
    /// Stacked; the divider is horizontal (split down).
    Vertical,
}

/// Binary split tree; `L` is a live [`PaneId`] or a saved pane description.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneTree<L> {
    Leaf(L),
    Split {
        axis: SplitAxis,
        /// Share of the space the first child takes, `0..1`.
        ratio: f32,
        first: Box<PaneTree<L>>,
        second: Box<PaneTree<L>>,
    },
}

/// Dividers can't squeeze a pane below this share.
pub const MIN_RATIO: f32 = 0.1;

impl<L: Clone + PartialEq> PaneTree<L> {
    /// Leaves in reading order (left to right, top to bottom).
    pub fn leaves(&self) -> Vec<L> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<L>) {
        match self {
            PaneTree::Leaf(l) => out.push(l.clone()),
            PaneTree::Split { first, second, .. } => {
                first.collect(out);
                second.collect(out);
            }
        }
    }

    pub fn contains(&self, target: &L) -> bool {
        match self {
            PaneTree::Leaf(l) => l == target,
            PaneTree::Split { first, second, .. } => {
                first.contains(target) || second.contains(target)
            }
        }
    }

    /// Put `new` beside `target` (after it), halving its space. `false` when
    /// `target` is not in the tree.
    pub fn split(&mut self, target: &L, new: L, axis: SplitAxis) -> bool {
        self.insert(target, new, axis, false)
    }

    /// Put `new` beside `target`, halving its space: before it (left or
    /// above) or after it (right or below). `false` when `target` is not in
    /// the tree.
    pub fn insert(&mut self, target: &L, new: L, axis: SplitAxis, before: bool) -> bool {
        match self {
            PaneTree::Leaf(l) if l == target => {
                let (first, second) = if before {
                    (new, l.clone())
                } else {
                    (l.clone(), new)
                };
                *self = PaneTree::Split {
                    axis,
                    ratio: 0.5,
                    first: Box::new(PaneTree::Leaf(first)),
                    second: Box::new(PaneTree::Leaf(second)),
                };
                true
            }
            PaneTree::Leaf(_) => false,
            PaneTree::Split { first, second, .. } => {
                if first.contains(target) {
                    first.insert(target, new, axis, before)
                } else {
                    second.insert(target, new, axis, before)
                }
            }
        }
    }

    /// Move `moving` next to `target` (dragging a pane onto another pane's
    /// edge): it leaves its place, whose sibling takes the room, and splits
    /// `target`. Unchanged when either is missing or they are the same.
    pub fn moved(self, moving: &L, target: &L, axis: SplitAxis, before: bool) -> Self {
        if moving == target || !self.contains(moving) || !self.contains(target) {
            return self;
        }
        let Some(mut rest) = self.clone().without(moving) else {
            return self;
        };
        if rest.insert(target, moving.clone(), axis, before) {
            rest
        } else {
            self
        }
    }

    /// The tree without `target`; its sibling takes the split's place.
    /// `None` when nothing is left.
    pub fn without(self, target: &L) -> Option<Self> {
        self.filter_map(&mut |l| (l != target).then(|| l.clone()))
    }

    /// Map every leaf, dropping those `f` rejects and collapsing the splits
    /// they leave with one child.
    pub fn filter_map<M>(&self, f: &mut impl FnMut(&L) -> Option<M>) -> Option<PaneTree<M>> {
        match self {
            PaneTree::Leaf(l) => f(l).map(PaneTree::Leaf),
            PaneTree::Split {
                axis,
                ratio,
                first,
                second,
            } => match (first.filter_map(f), second.filter_map(f)) {
                (Some(a), Some(b)) => Some(PaneTree::Split {
                    axis: *axis,
                    ratio: *ratio,
                    first: Box::new(a),
                    second: Box::new(b),
                }),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            },
        }
    }

    /// The leaf after (or before) `current`, wrapping around.
    pub fn cycle(&self, current: &L, forward: bool) -> Option<L> {
        let leaves = self.leaves();
        let len = leaves.len();
        let index = leaves.iter().position(|l| l == current)?;
        let next = if forward {
            (index + 1) % len
        } else {
            (index + len - 1) % len
        };
        leaves.into_iter().nth(next)
    }

    /// Where focus goes once `closing` is gone: the pane before it, or the
    /// one after when it was first.
    pub fn focus_after_close(&self, closing: &L) -> Option<L> {
        let leaves = self.leaves();
        let index = leaves.iter().position(|l| l == closing)?;
        let rest: Vec<L> = leaves.into_iter().filter(|l| l != closing).collect();
        if rest.is_empty() {
            return None;
        }
        rest.into_iter().nth(index.saturating_sub(1))
    }

    /// Exchange two leaves (dragging a pane onto another).
    pub fn swap(&mut self, a: &L, b: &L) {
        match self {
            PaneTree::Leaf(l) => {
                if l == a {
                    *l = b.clone();
                } else if l == b {
                    *l = a.clone();
                }
            }
            PaneTree::Split { first, second, .. } => {
                first.swap(a, b);
                second.swap(a, b);
            }
        }
    }

    /// Set the ratio of the split at `path` (`false` = first child, `true` =
    /// second, from the root), clamped so neither side vanishes.
    pub fn set_ratio(&mut self, path: &[bool], value: f32) -> bool {
        match (self, path.split_first()) {
            (PaneTree::Split { ratio, .. }, None) => {
                *ratio = value.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
                true
            }
            (PaneTree::Split { first, second, .. }, Some((side, rest))) => {
                if *side {
                    second.set_ratio(rest, value)
                } else {
                    first.set_ratio(rest, value)
                }
            }
            (PaneTree::Leaf(_), _) => false,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            PaneTree::Leaf(_) => 1,
            PaneTree::Split { first, second, .. } => first.len() + second.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PaneTree, SplitAxis};

    fn leaf(id: usize) -> PaneTree<usize> {
        PaneTree::Leaf(id)
    }

    #[test]
    fn insert_goes_before_or_after_the_target() {
        let mut tree = leaf(1);
        assert!(tree.insert(&1, 2, SplitAxis::Horizontal, true));
        assert_eq!(tree.leaves(), vec![2, 1]);
        assert!(tree.insert(&1, 3, SplitAxis::Vertical, false));
        assert_eq!(tree.leaves(), vec![2, 1, 3]);
        assert!(!tree.insert(&9, 4, SplitAxis::Vertical, true));
    }

    #[test]
    fn moving_a_pane_collapses_its_old_place_and_splits_the_target() {
        // [1 | [2 / 3]]: move 1 below 3.
        let mut tree = leaf(1);
        tree.split(&1, 2, SplitAxis::Horizontal);
        tree.split(&2, 3, SplitAxis::Vertical);
        let moved = tree.clone().moved(&1, &3, SplitAxis::Vertical, false);
        assert_eq!(moved.leaves(), vec![2, 3, 1]);
        assert_eq!(moved.len(), 3);
        // 1 left its column: 2 is now on top, with 3 and 1 stacked under it.
        match &moved {
            PaneTree::Split {
                axis: SplitAxis::Vertical,
                first,
                second,
                ..
            } => {
                assert_eq!(**first, leaf(2));
                assert_eq!(second.leaves(), vec![3, 1]);
            }
            other => panic!("unexpected shape: {other:?}"),
        }
        // Onto itself, or a pane that isn't there: nothing changes.
        assert_eq!(
            tree.clone().moved(&2, &2, SplitAxis::Horizontal, true),
            tree
        );
        assert_eq!(
            tree.clone().moved(&9, &2, SplitAxis::Horizontal, true),
            tree
        );
        // The last two panes can trade sides.
        let mut pair = leaf(1);
        pair.split(&1, 2, SplitAxis::Horizontal);
        assert_eq!(
            pair.moved(&1, &2, SplitAxis::Horizontal, false).leaves(),
            vec![2, 1]
        );
    }
    #[test]
    fn split_nests_beside_the_target() {
        let mut tree = leaf(1);
        assert!(tree.split(&1, 2, SplitAxis::Horizontal));
        assert!(tree.split(&1, 3, SplitAxis::Vertical));
        assert_eq!(tree.leaves(), vec![1, 3, 2]);
        assert!(!tree.split(&9, 4, SplitAxis::Vertical));
        match &tree {
            PaneTree::Split { axis, first, .. } => {
                assert_eq!(*axis, SplitAxis::Horizontal);
                assert!(matches!(
                    **first,
                    PaneTree::Split {
                        axis: SplitAxis::Vertical,
                        ..
                    }
                ));
            }
            PaneTree::Leaf(_) => panic!("expected a split"),
        }
    }

    #[test]
    fn close_collapses_the_parent_split() {
        let mut tree = leaf(1);
        tree.split(&1, 2, SplitAxis::Horizontal);
        tree.split(&2, 3, SplitAxis::Vertical);
        let tree = tree.without(&2).unwrap();
        assert_eq!(tree.leaves(), vec![1, 3]);
        // The vertical split had one child left, so it is gone.
        match &tree {
            PaneTree::Split { axis, second, .. } => {
                assert_eq!(*axis, SplitAxis::Horizontal);
                assert_eq!(**second, leaf(3));
            }
            PaneTree::Leaf(_) => panic!("expected a split"),
        }
        let tree = tree.without(&1).unwrap();
        assert_eq!(tree, leaf(3));
        assert_eq!(tree.without(&3), None);
    }

    #[test]
    fn focus_cycles_in_reading_order() {
        let mut tree = leaf(1);
        tree.split(&1, 2, SplitAxis::Horizontal);
        tree.split(&1, 3, SplitAxis::Vertical);
        assert_eq!(tree.cycle(&1, true), Some(3));
        assert_eq!(tree.cycle(&3, true), Some(2));
        assert_eq!(tree.cycle(&2, true), Some(1));
        assert_eq!(tree.cycle(&1, false), Some(2));
        assert_eq!(tree.cycle(&7, true), None);
        assert_eq!(tree.focus_after_close(&1), Some(3));
        assert_eq!(tree.focus_after_close(&3), Some(1));
        assert_eq!(tree.focus_after_close(&2), Some(3));
        assert_eq!(leaf(1).focus_after_close(&1), None);
    }

    #[test]
    fn ratios_follow_paths_and_clamp() {
        let mut tree = leaf(1);
        tree.split(&1, 2, SplitAxis::Horizontal);
        tree.split(&2, 3, SplitAxis::Vertical);
        assert!(tree.set_ratio(&[], 0.3));
        assert!(tree.set_ratio(&[true], 0.99));
        assert!(!tree.set_ratio(&[false], 0.5));
        let PaneTree::Split { ratio, second, .. } = &tree else {
            panic!("expected a split")
        };
        assert_eq!(*ratio, 0.3);
        let PaneTree::Split { ratio, .. } = &**second else {
            panic!("expected a split")
        };
        assert_eq!(*ratio, 0.9);
    }

    #[test]
    fn swap_and_filter_map_keep_the_shape() {
        let mut tree = leaf(1);
        tree.split(&1, 2, SplitAxis::Horizontal);
        tree.swap(&1, &2);
        assert_eq!(tree.leaves(), vec![2, 1]);
        let mapped = tree.filter_map(&mut |l| (*l != 2).then(|| l * 10)).unwrap();
        assert_eq!(mapped, leaf(10));
        assert_eq!(tree.len(), 2);
    }
}
