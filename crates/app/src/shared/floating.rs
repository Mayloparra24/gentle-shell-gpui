//! The card that floats over the window: the design canvas and the devices
//! panel share its geometry, drag and resize affordances, and shadow.
//!
//! Each panel keeps its own [`CardState`] and draws its own header and body;
//! this module owns where the card is, how a drag moves or resizes it, and
//! the bands and full-window layer that make the drag follow the pointer.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::*;

use crate::shared::ui;

/// The smallest card a resize allows.
pub const MIN_W: f32 = 320.;
pub const MIN_H: f32 = 220.;
/// Gap around a maximized card; the title bar stays clear.
pub const MAX_INSET: f32 = 10.;
/// Height of the window's title bar, which a maximized card leaves alone.
pub const TITLE_BAR: f32 = 36.;
/// The header you drag to move the card.
pub const HEADER: f32 = 38.;
/// The grab band along each edge, inside the card.
pub const GRIP: f32 = 6.;
/// The body's corner radius: the card's 18 px less the grip inset.
pub const BODY_RADIUS: f32 = 12.;

/// The card's place, anchored to the window's bottom-right so it stays put
/// when the window resizes: margins from the right and bottom, and its size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub right: f32,
    pub bottom: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edges {
    pub left: bool,
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragKind {
    Move,
    Resize(Edges),
}

#[derive(Clone, Copy)]
pub struct Drag {
    pub kind: DragKind,
    pub start: Point<Pixels>,
    pub frame: Frame,
}

/// Everything a floating card needs to move and resize itself.
pub struct CardState {
    frame: Frame,
    maximized: bool,
    drag: Option<Drag>,
    /// The window area the card lives in, as last laid out.
    area: Rc<Cell<(f32, f32)>>,
}

impl CardState {
    pub fn new(frame: Frame) -> Self {
        Self {
            frame,
            maximized: false,
            drag: None,
            area: Rc::new(Cell::new((0., 0.))),
        }
    }

    /// Put the card in a stage slot (tiling). Dragging or maximizing it later
    /// still works; a new stage re-tiles everything.
    pub fn set_frame(&mut self, frame: Frame) {
        self.frame = frame;
        self.maximized = false;
        self.drag = None;
    }

    pub fn maximized(&self) -> bool {
        self.maximized
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Keep the card inside the window area measured by [`area_canvas`].
    pub fn fit(&mut self) {
        let (aw, ah) = self.area.get();
        if aw > 0. && !self.maximized {
            self.frame = fitted(self.frame, (aw, ah));
        }
    }

    pub fn toggle_maximized(&mut self) {
        self.maximized = !self.maximized;
    }

    pub fn start(&mut self, kind: DragKind, start: Point<Pixels>) {
        if self.maximized {
            // Dragging a maximized card first restores it where it was.
            if kind != DragKind::Move {
                return;
            }
            self.maximized = false;
        }
        self.drag = Some(Drag {
            kind,
            start,
            frame: self.frame,
        });
    }

    /// Move or resize from a pointer position; whether the frame changed.
    pub fn drag_to(&mut self, position: Point<Pixels>) -> bool {
        let Some(drag) = &self.drag else { return false };
        let dx = (position.x - drag.start.x).as_f32();
        let dy = (position.y - drag.start.y).as_f32();
        let next = dragged(drag.frame, drag.kind, dx, dy, self.area.get());
        let changed = next != self.frame;
        self.frame = next;
        changed
    }

    pub fn end_drag(&mut self) -> bool {
        self.drag.take().is_some()
    }

    /// The cursor the whole-window drag layer shows.
    pub fn cursor(&self) -> Option<CursorStyle> {
        let drag = self.drag?;
        Some(match drag.kind {
            DragKind::Move => CursorStyle::ClosedHand,
            DragKind::Resize(edges) => cursor_for(edges),
        })
    }
}

/// The card element placed where [`CardState`] says. `card` must already be
/// absolute and sized by the caller (the card applies its own size here).
pub fn position(state: &CardState, card: Div) -> Div {
    if state.maximized {
        card.left(px(MAX_INSET))
            .right(px(MAX_INSET))
            .top(px(TITLE_BAR + MAX_INSET / 2.))
            .bottom(px(MAX_INSET))
    } else {
        card.right(px(state.frame.right))
            .bottom(px(state.frame.bottom))
            .w(px(state.frame.w))
            .h(px(state.frame.h))
    }
}

/// A canvas that records the window area; pair it with [`CardState::fit`].
pub fn area_canvas(state: &CardState) -> impl IntoElement {
    let area = state.area.clone();
    canvas(
        move |bounds, _, _| area.set((bounds.size.width.as_f32(), bounds.size.height.as_f32())),
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

/// The bands along the card's edges and corners that resize it.
pub fn grips<V: 'static>(
    state: &CardState,
    cx: &mut Context<V>,
    prefix: &'static str,
    on_start: impl Fn(&mut V, DragKind, Point<Pixels>, &mut Context<V>) + Clone + 'static,
) -> Vec<AnyElement> {
    if state.maximized {
        return Vec::new();
    }
    let edge = |left, top, right, bottom| Edges {
        left,
        top,
        right,
        bottom,
    };
    let corner = GRIP * 2.;
    [
        (
            edge(true, false, false, false),
            CursorStyle::ResizeLeftRight,
        ),
        (
            edge(false, false, true, false),
            CursorStyle::ResizeLeftRight,
        ),
        (edge(false, true, false, false), CursorStyle::ResizeUpDown),
        (edge(false, false, false, true), CursorStyle::ResizeUpDown),
        (
            edge(true, true, false, false),
            CursorStyle::ResizeUpLeftDownRight,
        ),
        (
            edge(false, false, true, true),
            CursorStyle::ResizeUpLeftDownRight,
        ),
        (
            edge(false, true, true, false),
            CursorStyle::ResizeUpRightDownLeft,
        ),
        (
            edge(true, false, false, true),
            CursorStyle::ResizeUpRightDownLeft,
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (edges, cursor))| {
        let band = div().id((prefix, index)).absolute().cursor(cursor);
        let band = match (edges.left, edges.top, edges.right, edges.bottom) {
            (true, false, false, false) => {
                band.left_0().top(px(corner)).bottom(px(corner)).w(px(GRIP))
            }
            (false, false, true, false) => band
                .right_0()
                .top(px(corner))
                .bottom(px(corner))
                .w(px(GRIP)),
            (false, true, false, false) => {
                band.top_0().left(px(corner)).right(px(corner)).h(px(GRIP))
            }
            (false, false, false, true) => band
                .bottom_0()
                .left(px(corner))
                .right(px(corner))
                .h(px(GRIP)),
            (true, true, ..) => band.left_0().top_0().size(px(corner)),
            (_, _, true, true) => band.right_0().bottom_0().size(px(corner)),
            (_, true, true, _) => band.right_0().top_0().size(px(corner)),
            _ => band.left_0().bottom_0().size(px(corner)),
        };
        let on_start = on_start.clone();
        band.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                on_start(this, DragKind::Resize(edges), event.position, cx);
            }),
        )
        .into_any_element()
    })
    .collect()
}

/// While dragging, a layer over the whole window follows the mouse, so a
/// fast drag never slips off the band it started on.
pub fn drag_layer<V: 'static>(
    state: &CardState,
    cx: &mut Context<V>,
    prefix: &'static str,
    on_move: impl Fn(&mut V, Point<Pixels>, &mut Context<V>) + Clone + 'static,
    on_up: impl Fn(&mut V, &mut Context<V>) + Clone + 'static,
) -> Option<AnyElement> {
    let cursor = state.cursor()?;
    let end = on_up.clone();
    Some(
        div()
            .id((prefix, 64usize))
            .absolute()
            .inset_0()
            .occlude()
            .cursor(cursor)
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                on_move(this, event.position, cx)
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| end(this, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| on_up(this, cx)),
            )
            .into_any_element(),
    )
}

pub fn cursor_for(edges: Edges) -> CursorStyle {
    match (edges.left || edges.right, edges.top || edges.bottom) {
        (true, false) => CursorStyle::ResizeLeftRight,
        (false, true) => CursorStyle::ResizeUpDown,
        _ if edges.left == edges.top => CursorStyle::ResizeUpLeftDownRight,
        _ => CursorStyle::ResizeUpRightDownLeft,
    }
}

/// A wide soft ambient shadow, a tighter key shadow and a faint rose glow.
/// Picked up (`lifted`, while dragged) it rises: everything spreads further
/// and drops lower.
pub fn shadow(lifted: bool) -> Vec<BoxShadow> {
    let lift = if lifted { 1.6 } else { 1. };
    let shadow = |alpha: f32, y: f32, blur: f32, spread: f32, color: Hsla| BoxShadow {
        color: color.opacity(alpha),
        offset: point(px(0.), px(y * lift)),
        blur_radius: px(blur * lift),
        spread_radius: px(spread),
        inset: false,
    };
    vec![
        shadow(0.5, 28., 70., -6., ui::backdrop()),
        shadow(0.45, 10., 24., -2., ui::backdrop()),
        shadow(if lifted { 0.16 } else { 0.1 }, 0., 40., 2., ui::petal()),
    ]
}

/// `frame` after dragging by (`dx`, `dy`) in a `w` × `h` area. Edges you
/// don't drag stay where they were; the card keeps a minimum size and stays
/// inside the area.
pub fn dragged(frame: Frame, kind: DragKind, dx: f32, dy: f32, (aw, ah): (f32, f32)) -> Frame {
    let mut next = frame;
    match kind {
        DragKind::Move => {
            next.right = frame.right - dx;
            next.bottom = frame.bottom - dy;
        }
        DragKind::Resize(edges) => {
            // The far edges in area coordinates, which a resize keeps fixed.
            let left = aw - frame.right - frame.w;
            let top = ah - frame.bottom - frame.h;
            if edges.left {
                next.w = (frame.w - dx).clamp(MIN_W, (aw - frame.right).max(MIN_W));
            }
            if edges.right {
                next.w = (frame.w + dx).clamp(MIN_W, (aw - left).max(MIN_W));
                next.right = aw - left - next.w;
            }
            if edges.top {
                next.h = (frame.h - dy).clamp(MIN_H, (ah - frame.bottom).max(MIN_H));
            }
            if edges.bottom {
                next.h = (frame.h + dy).clamp(MIN_H, (ah - top).max(MIN_H));
                next.bottom = ah - top - next.h;
            }
        }
    }
    fitted(next, (aw, ah))
}

/// `frame` kept inside a `w` × `h` area (the window may have shrunk).
pub fn fitted(frame: Frame, (aw, ah): (f32, f32)) -> Frame {
    if aw <= 0. || ah <= 0. {
        return frame;
    }
    let w = frame.w.min(aw).max(MIN_W.min(aw));
    let h = frame.h.min(ah - TITLE_BAR).max(MIN_H.min(ah));
    Frame {
        w,
        h,
        right: frame.right.clamp(0., (aw - w).max(0.)),
        bottom: frame.bottom.clamp(0., (ah - TITLE_BAR - h).max(0.)),
    }
}

/// Space between docked cards, and between them and the window's edges.
pub const DOCK_GAP: f32 = 12.;
pub const DOCK_MARGIN: f32 = 16.;
/// The share of a wide window the docked cards may take; the rest stays for
/// the conversation.
const DOCK_SHARE: f32 = 0.62;
/// Below this width the cards may use more of the window, up to this much.
const DOCK_MIN_ROOM: f32 = 720.;

/// Frames for cards docked at the window's bottom-right, from each card's
/// natural `(w, h)`, in order from left to right. They share one row while
/// they fit at their natural size or smaller (never below the card minimum);
/// otherwise they wrap into more rows, the first row at the bottom. They
/// never take more than [`DOCK_SHARE`] of a wide window, so the conversation
/// stays usable beside them.
pub fn docked(sizes: &[(f32, f32)], (aw, ah): (f32, f32)) -> Vec<Frame> {
    let count = sizes.len();
    if count == 0 || aw <= 0. || ah <= 0. {
        return Vec::new();
    }
    let room_w = (aw * DOCK_SHARE)
        .max(DOCK_MIN_ROOM.min(aw - DOCK_MARGIN * 2.))
        .max(1.);
    let room_h = (ah - TITLE_BAR - DOCK_MARGIN * 2.).max(1.);
    let gaps = |n: usize| DOCK_GAP * n.saturating_sub(1) as f32;
    let layout = |rows: usize| -> Option<Vec<Vec<(f32, f32)>>> {
        let row_h = (room_h - gaps(rows)) / rows as f32;
        let chunks: Vec<&[(f32, f32)]> = sizes.chunks(count.div_ceil(rows)).collect();
        // One scale for every card, so they keep their relative sizes.
        let scale = chunks
            .iter()
            .map(|row| {
                let w: f32 = row.iter().map(|size| size.0).sum();
                let h = row.iter().map(|size| size.1).fold(0., f32::max);
                ((room_w - gaps(row.len())) / w).min(row_h / h)
            })
            .fold(1., f32::min);
        let rows: Vec<Vec<(f32, f32)>> = chunks
            .iter()
            .map(|row| {
                row.iter()
                    .map(|&(w, h)| ((w * scale).max(MIN_W), (h * scale).max(MIN_H)))
                    .collect()
            })
            .collect();
        let fits = rows.iter().all(|row| {
            row.iter().map(|size| size.0).sum::<f32>() + gaps(row.len()) <= room_w + 0.5
                && row.iter().all(|size| size.1 <= row_h + 0.5)
        });
        fits.then_some(rows)
    };
    // The last resort is one card per row, each fitted to its slot.
    let rows = (1..=count).find_map(layout).unwrap_or_else(|| {
        let row_h = (room_h - gaps(count)) / count as f32;
        sizes
            .iter()
            .map(|&(w, h)| vec![(w.min(room_w), h.min(row_h))])
            .collect()
    });
    let mut frames = Vec::with_capacity(count);
    let mut bottom = DOCK_MARGIN;
    for row in rows {
        let mut right = DOCK_MARGIN + row.iter().map(|size| size.0).sum::<f32>() + gaps(row.len());
        for &(w, h) in &row {
            right -= w;
            frames.push(Frame {
                right,
                bottom,
                w,
                h,
            });
            right -= DOCK_GAP;
        }
        bottom += row.iter().map(|size| size.1).fold(0., f32::max) + DOCK_GAP;
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::{
        DOCK_MARGIN, DragKind, Edges, Frame, MIN_H, MIN_W, TITLE_BAR, docked, dragged, fitted,
    };

    const AREA: (f32, f32) = (1400., 900.);
    const FRAME: Frame = Frame {
        right: 16.,
        bottom: 16.,
        w: 560.,
        h: 380.,
    };

    fn edges(left: bool, top: bool, right: bool, bottom: bool) -> DragKind {
        DragKind::Resize(Edges {
            left,
            top,
            right,
            bottom,
        })
    }

    #[test]
    fn dragging_moves_and_resizes_from_the_grabbed_edges_only() {
        let f = FRAME;
        // Moving left and up grows the right and bottom margins.
        let moved = dragged(f, DragKind::Move, -100., -50., AREA);
        assert_eq!(
            (moved.right, moved.bottom, moved.w, moved.h),
            (116., 66., f.w, f.h)
        );
        // The left edge grows the card leftward; its right side stays.
        let wider = dragged(f, edges(true, false, false, false), -200., 0., AREA);
        assert_eq!((wider.w, wider.right), (f.w + 200., f.right));
        // The right edge keeps the left side in place.
        let left_side = AREA.0 - f.right - f.w;
        let narrower = dragged(f, edges(false, false, true, false), -100., 0., AREA);
        assert_eq!(narrower.w, f.w - 100.);
        assert_eq!(AREA.0 - narrower.right - narrower.w, left_side);
        // The top-left corner grows both ways.
        let big = dragged(f, edges(true, true, false, false), -300., -200., AREA);
        assert_eq!((big.w, big.h), (f.w + 300., f.h + 200.));
        // Never below the minimum size.
        let tiny = dragged(f, edges(true, true, false, false), 900., 900., AREA);
        assert_eq!((tiny.w, tiny.h), (MIN_W, MIN_H));
    }

    #[test]
    fn the_card_stays_inside_the_window() {
        // Dragged past the window's edges, it stops at them.
        let away = dragged(FRAME, DragKind::Move, 500., 500., AREA);
        assert_eq!((away.right, away.bottom), (0., 0.));
        let up = dragged(FRAME, DragKind::Move, -5000., -5000., AREA);
        assert_eq!(up.right, AREA.0 - up.w);
        assert_eq!(up.bottom, AREA.1 - TITLE_BAR - up.h);
        // A window that shrank keeps the card whole.
        let big = Frame {
            right: 400.,
            bottom: 300.,
            w: 900.,
            h: 600.,
        };
        let fit = fitted(big, (800., 500.));
        assert!(fit.w <= 800. && fit.right + fit.w <= 800.);
        assert!(fit.bottom + fit.h <= 500. - TITLE_BAR);
    }

    const PHONE: (f32, f32) = (400., 760.);
    const PAGE: (f32, f32) = (560., 380.);

    fn overlaps(a: &Frame, b: &Frame) -> bool {
        a.right < b.right + b.w
            && b.right < a.right + a.w
            && a.bottom < b.bottom + b.h
            && b.bottom < a.bottom + a.h
    }

    fn inside(frame: &Frame, (aw, ah): (f32, f32)) -> bool {
        frame.right >= 0.
            && frame.bottom >= 0.
            && frame.right + frame.w <= aw
            && frame.bottom + frame.h <= ah - TITLE_BAR
    }

    #[test]
    fn docked_cards_keep_their_size_in_a_big_window_left_to_right() {
        let frames = docked(&[PHONE, PAGE], (2000., 1200.));
        assert_eq!((frames[0].w, frames[0].h), PHONE);
        assert_eq!((frames[1].w, frames[1].h), PAGE);
        // Bottom-aligned, the later card on the right.
        assert_eq!(frames[1].right, DOCK_MARGIN);
        assert!(frames[0].right > frames[1].right + frames[1].w);
        assert_eq!(frames[0].bottom, frames[1].bottom);
    }

    #[test]
    fn docked_cards_leave_the_conversation_room_in_a_laptop_window() {
        let area = (1500., 950.);
        let frames = docked(&[PHONE, PAGE], area);
        let left = frames
            .iter()
            .map(|f| area.0 - f.right - f.w)
            .fold(f32::MAX, f32::min);
        assert!(left >= area.0 * 0.35, "cards reach x = {left}");
        assert!(frames.iter().all(|f| inside(f, area)));
        assert!(!overlaps(&frames[0], &frames[1]));
        // Shrunk, not stretched: the phone keeps its proportions.
        assert!((frames[0].w / frames[0].h - PHONE.0 / PHONE.1).abs() < 0.01);
    }

    #[test]
    fn many_docked_cards_wrap_into_rows_without_overlapping() {
        let area = (1100., 900.);
        let frames = docked(&[PAGE, PAGE, PAGE, PAGE, PAGE], area);
        assert_eq!(frames.len(), 5);
        assert!(frames.iter().all(|f| inside(f, area)));
        assert!(frames.iter().map(|f| f.bottom).any(|b| b > DOCK_MARGIN));
        for (i, a) in frames.iter().enumerate() {
            for b in &frames[i + 1..] {
                assert!(!overlaps(a, b), "{a:?} overlaps {b:?}");
            }
        }
    }
}
