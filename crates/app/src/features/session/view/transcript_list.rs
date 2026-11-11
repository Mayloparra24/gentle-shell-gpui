//! Filas, mediciones y cachés de presentación de la lista virtualizada.

use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use gpui_kit::*;

use super::{CardSpec, card_spec, line_count};
use crate::shared::ui;
use gentle_core::transcript::{Dirty, Item};

/// Rows after the transcript items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tail {
    /// The prompt just sent, in the slot (`base`) Pi's echo of it takes.
    PendingBubble(usize),
    /// "Thinking…" until Pi picks the prompt up.
    Pending(usize),
    /// "Working…" between tool calls, at this item count.
    Between(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Row {
    /// The hero and the backend state (empty once both are gone).
    Header,
    Item(usize),
    Tail(Tail),
    /// The column's bottom padding.
    Footer,
}

/// The list's rows: the header, one per transcript item, the tail rows and
/// the footer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Shape {
    items: usize,
    tail: Vec<Tail>,
}

impl Shape {
    fn len(&self) -> usize {
        self.items + self.tail.len() + 2
    }

    fn row(&self, ix: usize) -> Option<Row> {
        let tail_start = 1 + self.items;
        match ix {
            0 => Some(Row::Header),
            ix if ix < tail_start => Some(Row::Item(ix - 1)),
            ix if ix < tail_start + self.tail.len() => Some(Row::Tail(self.tail[ix - tail_start])),
            ix if ix + 1 == self.len() => Some(Row::Footer),
            _ => None,
        }
    }
}

/// How the list's rows change from `old` to `new`, given the first changed
/// item: rows to re-measure in place, then rows to replace.
#[derive(Debug, PartialEq, Eq)]
struct ListUpdate {
    remeasure: Range<usize>,
    /// Old rows replaced, and how many new rows take their place.
    splice: Option<(Range<usize>, usize)>,
}

fn list_update(old: &Shape, new: &Shape, dirty_from: Option<usize>) -> ListUpdate {
    let (old_len, new_len) = (old.len(), new.len());
    let mut first = old_len.max(new_len);
    if let Some(from) = dirty_from {
        first = first.min(1 + from);
    }
    if old.items != new.items {
        first = first.min(1 + old.items.min(new.items));
    } else if let Some(i) =
        (0..old.tail.len().max(new.tail.len())).find(|&i| old.tail.get(i) != new.tail.get(i))
    {
        first = first.min(1 + old.items + i);
    }
    let common = old_len.min(new_len);
    ListUpdate {
        remeasure: first.min(common)..common,
        splice: (old_len != new_len).then(|| (common..old_len, new_len - common)),
    }
}

/// What the header row shows; a change re-measures it (and the first row,
/// whose top padding depends on it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HeaderKey {
    pub(super) hero: bool,
    pub(super) backend: u8,
}

/// Display data derived once per change of an item, not per frame.
pub(super) enum Prepared {
    Card(CardSpec),
    /// A custom message's line count.
    Lines(usize),
}

/// How long after it first shows a row still fades in.
const APPEAR_WINDOW: Duration = Duration::from_millis(300);

/// The conversation as a virtualized list: only rows on screen render, and
/// rows are re-measured only from the first changed item on.
pub(in crate::features::session) struct TranscriptList {
    state: ListState,
    /// The rows `state` was last told about.
    shape: Shape,
    header: Option<HeaderKey>,
    /// By transcript index.
    prepared: HashMap<usize, Prepared>,
    /// When each animated row first rendered: it animates in only then, not
    /// each time it scrolls back into view.
    born: HashMap<(&'static str, usize), Instant>,
    /// Row heights seen on screen, by row. The list only knows the heights of
    /// rows it has laid out; the rail estimates the rest from these.
    heights: HashMap<usize, f32>,
    /// Each item's size from its content (`gentle_core::scroll::item_weight`),
    /// by transcript index: how the rail sizes rows it hasn't seen.
    weights: Vec<f32>,
}

/// Header, tail and footer rows, in the same "lines" as item weights.
const HEADER_WEIGHT: f32 = 4.;
const TAIL_WEIGHT: f32 = 2.5;
const FOOTER_WEIGHT: f32 = 1.4;

impl TranscriptList {
    pub(in crate::features::session) fn new() -> Self {
        let shape = Shape::default();
        Self {
            state: ListState::new(shape.len(), ListAlignment::Top, px(600.)),
            shape,
            header: None,
            prepared: HashMap::new(),
            born: HashMap::new(),
            heights: HashMap::new(),
            weights: Vec::new(),
        }
    }

    /// The transcript was replaced: indices name other items now.
    pub(super) fn forget_items(&mut self) {
        self.prepared.clear();
        self.born.clear();
        self.heights.clear();
        self.weights.clear();
    }

    /// Note the heights of the rows on screen. Only those: a row laid out in
    /// the overdraw below can still be growing (its Markdown parses in the
    /// background) and its size isn't updated until it scrolls in.
    pub(super) fn record_heights(&mut self) {
        let viewport = self.state.viewport_bounds();
        let mut row = self.state.logical_scroll_top().item_ix;
        while row < self.shape.len() {
            let Some(bounds) = self.state.bounds_for_item(row) else {
                break;
            };
            if bounds.top() >= viewport.bottom() {
                break;
            }
            self.heights.insert(row, f32::from(bounds.size.height));
            row += 1;
        }
    }

    /// Row tops, total height and scroll offset: rows never laid out count
    /// by their content, scaled by what the laid-out rows measured. Also
    /// the pane's height.
    pub(super) fn estimate(&self) -> (gentle_core::scroll::Estimate, f32) {
        let mut weights = Vec::with_capacity(self.shape.len());
        weights.push(HEADER_WEIGHT);
        weights.extend(
            (0..self.shape.items).map(|ix| self.weights.get(ix).copied().unwrap_or(TAIL_WEIGHT)),
        );
        weights.extend(self.shape.tail.iter().map(|_| TAIL_WEIGHT));
        weights.push(FOOTER_WEIGHT);
        let top = self.state.logical_scroll_top();
        let viewport = f32::from(self.state.viewport_bounds().size.height);
        let estimate = gentle_core::scroll::estimate(
            &weights,
            &self.heights,
            top.item_ix,
            f32::from(top.offset_in_item),
            viewport,
        );
        (estimate, viewport)
    }

    /// Derive display data for items `from..`.
    pub(super) fn prepare(&mut self, items: &[Item], from: usize) {
        self.weights.resize(from.min(items.len()), TAIL_WEIGHT);
        let changed = items.get(from..).unwrap_or_default();
        for (ix, item) in (from..).zip(changed) {
            self.weights.push(gentle_core::scroll::item_weight(item));
            let prepared = match item {
                Item::Tool { .. } | Item::Shell { .. } => card_spec(item).map(Prepared::Card),
                Item::Custom { text, .. } => Some(Prepared::Lines(line_count(text))),
                Item::CommandOutput { notices, .. } => Some(Prepared::Lines(
                    notices.iter().map(|(_, text)| line_count(text)).sum(),
                )),
                _ => None,
            };
            match prepared {
                Some(prepared) => {
                    self.prepared.insert(ix, prepared);
                }
                None => {
                    self.prepared.remove(&ix);
                }
            }
        }
    }

    /// Tell the list which rows changed since the last sync.
    pub(super) fn sync(
        &mut self,
        items: usize,
        tail: Vec<Tail>,
        header: HeaderKey,
        dirty: Option<Dirty>,
    ) {
        let shape = Shape { items, tail };
        if dirty.is_some_and(|d| d.reset) {
            self.state.reset(shape.len());
            self.heights.clear();
        } else {
            if self.header.is_some_and(|old| old != header) {
                self.state.remeasure_items(0..self.shape.len().min(2));
                self.heights.retain(|row, _| *row >= 2);
            }
            let update = list_update(&self.shape, &shape, dirty.map(|d| d.from));
            if !update.remeasure.is_empty() {
                self.heights
                    .retain(|row, _| !update.remeasure.contains(row));
                self.state.remeasure_items(update.remeasure);
            }
            if let Some((range, count)) = update.splice {
                // Rows from the splice on now name other rows.
                self.heights.retain(|row, _| *row < range.start);
                self.state.splice(range, count);
            }
        }
        self.header = Some(header);
        self.shape = shape;
    }

    /// Item `ix` changed height (a fold toggled).
    pub(super) fn remeasure_item(&mut self, ix: usize) {
        let row = ix + 1;
        if row < self.shape.len() {
            self.heights.remove(&row);
            self.state.remeasure_items(row..row + 1);
        }
    }

    pub(super) fn prepared(&self, ix: usize) -> Option<&Prepared> {
        self.prepared.get(&ix)
    }

    pub(super) fn row(&self, ix: usize) -> Option<Row> {
        self.shape.row(ix)
    }

    pub(super) fn state(&self) -> ListState {
        self.state.clone()
    }

    /// Scroll to the bottom at the next layout.
    pub(in crate::features::session) fn scroll_to_end(&self) {
        self.state.scroll_to_end();
    }

    /// Scrolled to (within a few lines of) the bottom: new output follows.
    pub(in crate::features::session) fn at_bottom(&self) -> bool {
        let offset = -self.state.scroll_px_offset_for_scrollbar().y;
        let max = self.state.max_offset_for_scrollbar().y;
        offset >= max - px(48.)
    }

    /// `element`, fading in if the row first showed just now.
    pub(super) fn appear(
        &mut self,
        id: (&'static str, usize),
        now: Instant,
        element: Div,
    ) -> AnyElement {
        let born = *self.born.entry(id).or_insert(now);
        if now.duration_since(born) < APPEAR_WINDOW {
            ui::appear(id, element).into_any_element()
        } else {
            element.into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ListUpdate, Row, Shape, Tail, list_update};

    #[test]
    fn rows_map_to_header_items_tail_and_footer() {
        let shape = Shape {
            items: 2,
            tail: vec![Tail::Between(2)],
        };
        assert_eq!(shape.len(), 5);
        assert_eq!(shape.row(0), Some(Row::Header));
        assert_eq!(shape.row(2), Some(Row::Item(1)));
        assert_eq!(shape.row(3), Some(Row::Tail(Tail::Between(2))));
        assert_eq!(shape.row(4), Some(Row::Footer));
        assert_eq!(shape.row(5), None);

        let empty = Shape::default();
        assert_eq!(empty.len(), 2);
        assert_eq!(empty.row(0), Some(Row::Header));
        assert_eq!(empty.row(1), Some(Row::Footer));
        assert_eq!(empty.row(2), None);
    }

    #[test]
    fn list_updates_start_at_the_first_changed_row() {
        let shape = |items, tail: &[Tail]| Shape {
            items,
            tail: tail.to_vec(),
        };
        // Streaming into the last of 3 items: only its row is re-measured.
        assert_eq!(
            list_update(&shape(3, &[]), &shape(3, &[]), Some(2)),
            ListUpdate {
                remeasure: 3..5,
                splice: None
            }
        );
        // Nothing changed: nothing to do.
        assert_eq!(
            list_update(&shape(3, &[]), &shape(3, &[]), None),
            ListUpdate {
                remeasure: 5..5,
                splice: None
            }
        );
        // An item appended: the old footer slot is re-measured, one row added.
        assert_eq!(
            list_update(&shape(3, &[]), &shape(4, &[]), Some(3)),
            ListUpdate {
                remeasure: 4..5,
                splice: Some((5..5, 1))
            }
        );
        // Removing items remeasures from the first removed item and truncates rows.
        assert_eq!(
            list_update(&shape(4, &[]), &shape(2, &[]), Some(2)),
            ListUpdate {
                remeasure: 3..4,
                splice: Some((4..6, 0))
            }
        );
        // The pending bubble becomes the real message; the indicator goes.
        let pending = [Tail::PendingBubble(1), Tail::Pending(1)];
        assert_eq!(
            list_update(&shape(1, &pending), &shape(2, &[]), Some(1)),
            ListUpdate {
                remeasure: 2..4,
                splice: Some((4..5, 0))
            }
        );
        // Only the tail changed.
        assert_eq!(
            list_update(&shape(2, &[]), &shape(2, &[Tail::Between(2)]), None),
            ListUpdate {
                remeasure: 3..4,
                splice: Some((4..4, 1))
            }
        );
        // A same-length tail replacement remeasures from that tail without splicing.
        assert_eq!(
            list_update(
                &shape(2, &[Tail::Pending(2)]),
                &shape(2, &[Tail::Between(2)]),
                None,
            ),
            ListUpdate {
                remeasure: 3..5,
                splice: None
            }
        );
    }
}
