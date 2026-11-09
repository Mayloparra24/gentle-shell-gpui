//! Pane size math: minimum pane sizes and split fits.

use super::*;

/// Smallest useful pane; splits and divider drags stop here.
pub(super) const MIN_PANE_WIDTH: f32 = 320.;
pub(super) const MIN_PANE_HEIGHT: f32 = 200.;
/// Space a divider takes between two panes.
pub(super) const DIVIDER: f32 = 6.;

/// Whether a `w`×`h` pane split along `axis` leaves both halves usable.
pub(super) fn fits_split(axis: SplitAxis, w: f32, h: f32) -> bool {
    match axis {
        SplitAxis::Horizontal => (w - DIVIDER) / 2. >= MIN_PANE_WIDTH,
        SplitAxis::Vertical => (h - DIVIDER) / 2. >= MIN_PANE_HEIGHT,
    }
}

/// Each leaf's size when `node` fills `w`×`h`.
pub(super) fn leaf_sizes(
    node: &PaneTree<PaneId>,
    w: f32,
    h: f32,
    out: &mut Vec<(PaneId, f32, f32)>,
) {
    match node {
        PaneTree::Leaf(id) => out.push((*id, w, h)),
        PaneTree::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            if *axis == SplitAxis::Horizontal {
                let inner = w - DIVIDER;
                leaf_sizes(first, inner * ratio, h, out);
                leaf_sizes(second, inner * (1. - ratio), h, out);
            } else {
                let inner = h - DIVIDER;
                leaf_sizes(first, w, inner * ratio, out);
                leaf_sizes(second, w, inner * (1. - ratio), out);
            }
        }
    }
}
