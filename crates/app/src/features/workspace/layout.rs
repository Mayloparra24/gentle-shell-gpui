//! Pane size math: minimum pane sizes and split fits, the tiling direction
//! for a new pane, and where a dragged pane lands.

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

/// Where a new pane goes when tiling (Hyprland's dwindle): split `w`×`h`
/// along its longer side, else the other one if only that fits. `None` when
/// neither half would be usable.
pub(super) fn tile_axis(w: f32, h: f32) -> Option<SplitAxis> {
    let (long, short) = if w >= h {
        (SplitAxis::Horizontal, SplitAxis::Vertical)
    } else {
        (SplitAxis::Vertical, SplitAxis::Horizontal)
    };
    [long, short]
        .into_iter()
        .find(|axis| fits_split(*axis, w, h))
}

/// Where a pane dragged over another one lands: near an edge it moves to
/// that side of the target, in the middle the two swap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DropZone {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

impl DropZone {
    /// The zone under the pointer at `x`, `y` (0..1 across the target pane).
    pub(super) fn at(x: f32, y: f32) -> Self {
        let (dx, dy) = (x - 0.5, y - 0.5);
        if dx.abs() < 0.2 && dy.abs() < 0.2 {
            DropZone::Center
        } else if dx.abs() >= dy.abs() {
            if dx < 0. {
                DropZone::Left
            } else {
                DropZone::Right
            }
        } else if dy < 0. {
            DropZone::Top
        } else {
            DropZone::Bottom
        }
    }

    /// The split that puts the dragged pane on this side (axis, before the
    /// target); `None` for the middle, which swaps.
    pub(super) fn placement(self) -> Option<(SplitAxis, bool)> {
        match self {
            DropZone::Left => Some((SplitAxis::Horizontal, true)),
            DropZone::Right => Some((SplitAxis::Horizontal, false)),
            DropZone::Top => Some((SplitAxis::Vertical, true)),
            DropZone::Bottom => Some((SplitAxis::Vertical, false)),
            DropZone::Center => None,
        }
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

#[cfg(test)]
mod tests {
    use super::{DropZone, SplitAxis, tile_axis};

    #[test]
    fn tiling_splits_the_longer_side_while_it_fits() {
        assert_eq!(tile_axis(1200., 800.), Some(SplitAxis::Horizontal));
        assert_eq!(tile_axis(600., 800.), Some(SplitAxis::Vertical));
        // Wide but too narrow to halve: stack instead.
        assert_eq!(tile_axis(640., 500.), Some(SplitAxis::Vertical));
        // Neither half would be usable.
        assert_eq!(tile_axis(500., 300.), None);
    }

    #[test]
    fn drop_zones_follow_the_nearest_edge_and_swap_in_the_middle() {
        assert_eq!(DropZone::at(0.5, 0.5), DropZone::Center);
        assert_eq!(DropZone::at(0.05, 0.5), DropZone::Left);
        assert_eq!(DropZone::at(0.95, 0.4), DropZone::Right);
        assert_eq!(DropZone::at(0.5, 0.1), DropZone::Top);
        assert_eq!(DropZone::at(0.6, 0.9), DropZone::Bottom);
        assert_eq!(
            DropZone::Left.placement(),
            Some((SplitAxis::Horizontal, true))
        );
        assert_eq!(
            DropZone::Bottom.placement(),
            Some((SplitAxis::Vertical, false))
        );
        assert_eq!(DropZone::Center.placement(), None);
    }
}
