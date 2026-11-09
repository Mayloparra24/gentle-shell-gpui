//! The effort (thinking level) control and its slider card.

use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::session::SessionView;
use crate::ui;

impl SessionView {
    /// The effort (thinking level) control: the level and a pixel meter in
    /// the composer bar. It opens a card with a slider in Gentle Shell's rose
    /// gradient, one stop per level the model accepts; click, drag or pick a
    /// label below it.
    pub(super) fn render_effort_picker(
        &self,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let levels = self.models.thinking_levels.clone();
        let current = self.models.thinking.clone();
        let index = effort_index(&levels, &current);
        let fraction = effort_fraction(&levels, &current);
        // Glide only right after a change: the card's elements are rebuilt
        // each time it opens, and a finished glide must not replay then.
        let from = match self.live.effort_moved_at {
            Some(at) if at.elapsed() < EFFORT_MOTION => self.live.effort_from,
            _ => fraction,
        };
        let moves = self.live.effort_moves;
        let weak = cx.entity().downgrade();
        let trigger = Button::new("thinking")
            .ghost()
            .xsmall()
            .text_color(ui::muted())
            .icon(Icon::new(effort_icon(&current)))
            .tooltip("Effort: how long the model thinks")
            .when(!narrow, |b| b.label(current.clone()))
            .child(effort_meter(index, levels.len()));
        Popover::new("effort-popover")
            .anchor(Anchor::BottomLeft)
            .trigger(trigger)
            .w(px(EFFORT_W))
            .appearance(false)
            .content(move |_, _, cx| {
                let (title, detail) = effort_copy(&current);
                let card = v_flex()
                    .p(px(EFFORT_PAD))
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Icon::new(effort_icon(&current))
                                    .small()
                                    .text_color(ui::petal()),
                            )
                            .child(ui::section_label("Effort"))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .font_family(ui::MONO)
                                    .text_size(px(11.))
                                    .text_color(ui::muted())
                                    .child(current.clone()),
                            ),
                    )
                    .child(
                        v_flex()
                            .items_center()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(px(17.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ui::petal())
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(ui::muted())
                                    .child(detail),
                            ),
                    )
                    .child(effort_slider(
                        &levels, &current, fraction, from, moves, &weak,
                    ))
                    .child(effort_labels(&levels, index, &weak));
                ui::island(cx)
                    .w_full()
                    .child(ui::popover_in(ElementId::Name("effort-in".into()), card))
                    .into_any_element()
            })
    }

    /// Set the thinking level from the effort card; the knob springs from
    /// where it was. Goes back if the backend refuses the level.
    fn set_thinking(&mut self, level: String, cx: &mut Context<Self>) {
        if self.models.thinking == level {
            return;
        }
        self.live.effort_from =
            effort_fraction(&self.models.thinking_levels, &self.models.thinking);
        self.live.effort_moved_at = Some(Instant::now());
        self.live.effort_moves += 1;
        let previous = std::mem::replace(&mut self.models.thinking, level.clone());
        cx.notify();
        self.request(
            serde_json::json!({"type": "set_thinking_level", "level": level}),
            cx,
            move |this, result, cx| {
                if this.report(result).is_none() {
                    this.models.thinking = previous;
                    cx.notify();
                } else if this.models.thinking == "max" {
                    cx.emit(crate::session::SessionEvent::MaxEffort);
                }
            },
        );
    }
}

/// The effort card's width and padding.
const EFFORT_W: f32 = 320.;
const EFFORT_PAD: f32 = 16.;
/// The slider fills the card inside its padding and 1 px border.
const EFFORT_TRACK_W: f32 = EFFORT_W - 2. * EFFORT_PAD - 2.;
const EFFORT_TRACK_H: f32 = 40.;
/// Gap between the track's edge and the knob.
const EFFORT_INSET: f32 = 3.;
const EFFORT_KNOB: f32 = EFFORT_TRACK_H - 2. * EFFORT_INSET;
/// How far the knob's left edge travels from the lowest to the highest stop.
const EFFORT_SPAN: f32 = EFFORT_TRACK_W - 2. * EFFORT_INSET - EFFORT_KNOB;
/// Knob glide after a change.
const EFFORT_MOTION: Duration = Duration::from_millis(420);
/// Pixel sparkles in the slider's fill, after the rose's pixel art: x along
/// the track (0..=1), y and size in px, and a twinkle phase. The fill clips
/// them, so more show the higher the effort.
const EFFORT_SPARKLES: [(f32, f32, f32, f32); 16] = [
    (0.07, 13., 2., 0.10),
    (0.12, 25., 3., 0.55),
    (0.18, 9., 2., 0.80),
    (0.24, 21., 2., 0.30),
    (0.30, 14., 3., 0.65),
    (0.37, 27., 2., 0.05),
    (0.43, 8., 2., 0.45),
    (0.49, 18., 3., 0.90),
    (0.55, 28., 2., 0.20),
    (0.61, 11., 2., 0.70),
    (0.67, 22., 3., 0.35),
    (0.72, 7., 2., 0.85),
    (0.78, 16., 2., 0.15),
    (0.84, 26., 3., 0.60),
    (0.90, 12., 2., 0.40),
    (0.95, 20., 2., 0.95),
];

fn effort_index(levels: &[String], level: &str) -> usize {
    levels.iter().position(|l| l == level).unwrap_or(0)
}

/// Where `level` sits along the slider: 0 at the lowest stop, 1 at the
/// highest. Unknown levels sit at the start.
fn effort_fraction(levels: &[String], level: &str) -> f32 {
    if levels.len() < 2 {
        return 0.;
    }
    effort_index(levels, level) as f32 / (levels.len() - 1) as f32
}

/// The x of stop `index`'s center, where the knob's center rests.
fn effort_stop_x(index: usize, count: usize) -> f32 {
    let fraction = if count < 2 {
        0.
    } else {
        index as f32 / (count - 1) as f32
    };
    EFFORT_INSET + EFFORT_KNOB / 2. + EFFORT_SPAN * fraction
}

/// The fill reaches just past the knob at `fraction`.
fn effort_fill_w(fraction: f32) -> f32 {
    2. * EFFORT_INSET + EFFORT_KNOB + EFFORT_SPAN * fraction
}

/// The card's headline and what the level costs.
fn effort_copy(level: &str) -> (&'static str, &'static str) {
    match level {
        "off" => (
            "No thinking",
            "Answers right away: fastest, lightest on limits",
        ),
        "minimal" => ("A quick glance", "Barely thinks before answering"),
        "low" => ("Light thinking", "Quick reasoning for simple tasks"),
        "medium" => ("Balanced", "Thinks it through without dragging"),
        "high" => (
            "Deep thinking",
            "Better on hard problems; uses limits faster",
        ),
        "xhigh" => ("Extra deep", "Long reasoning; consumes usage limits faster"),
        "max" => (
            "All in",
            "Thinks as long as it takes; slowest and costliest",
        ),
        _ => ("Thinking", "How long the model reasons before answering"),
    }
}

fn effort_short(level: &str) -> &str {
    match level {
        "minimal" => "min",
        "medium" => "med",
        other => other,
    }
}

/// The wine the rose's pixels fade into: the start of the slider's gradient.
fn effort_deep() -> Hsla {
    let petal = ui::petal();
    hsla(petal.h, petal.s * 0.7, petal.l * 0.32, 1.)
}

/// The trigger's pixel meter: one rising bar per level above the lowest,
/// lit up to the current one.
fn effort_meter(index: usize, count: usize) -> Div {
    h_flex()
        .items_end()
        .gap(px(2.))
        .ml_0p5()
        .children((1..count).map(|i| {
            div().w(px(3.)).h(px(3. + 2. * i as f32)).bg(if i <= index {
                ui::petal()
            } else {
                ui::faint().opacity(0.35)
            })
        }))
}

/// Each level's mark, from resting to all-out.
fn effort_icon(level: &str) -> IconName {
    match level {
        "off" => IconName::Moon,
        "minimal" => IconName::Feather,
        "low" => IconName::Leaf,
        "high" => IconName::BrainCircuit,
        "xhigh" => IconName::Zap,
        "max" => IconName::Flame,
        _ => IconName::Brain,
    }
}

/// The slider: a pill whose rose fill glides with the knob, twinkling pixel
/// sparkles inside it, and one hit zone per stop (click or drag across).
fn effort_slider(
    levels: &[String],
    current: &str,
    fraction: f32,
    from: f32,
    moves: u64,
    weak: &WeakEntity<SessionView>,
) -> Div {
    let count = levels.len();
    let sparkle = {
        let petal = ui::petal();
        hsla(petal.h, 0.9, 0.93, 1.)
    };
    let glide = move |t: f32| from + (fraction - from) * ui::spring(t);
    let fill = div()
        .absolute()
        .left_0()
        .top_0()
        .bottom_0()
        .rounded_full()
        .overflow_hidden()
        .bg(linear_gradient(
            90.,
            linear_color_stop(effort_deep(), 0.),
            linear_color_stop(ui::petal(), 1.),
        ))
        .children(
            EFFORT_SPARKLES
                .iter()
                .enumerate()
                .map(|(i, &(x, y, size, phase))| {
                    div()
                        .absolute()
                        .left(px(x * EFFORT_TRACK_W))
                        .top(px(y))
                        .size(px(size))
                        .bg(sparkle)
                        .with_animation(
                            ElementId::NamedInteger("effort-sparkle".into(), i as u64),
                            Animation::new(Duration::from_millis(1400 + (i as u64 % 5) * 260))
                                .repeat(),
                            move |el, t| {
                                let wave = 0.5 + 0.5 * (std::f32::consts::TAU * (t + phase)).sin();
                                el.opacity(0.15 + 0.85 * wave)
                            },
                        )
                }),
        )
        .with_animation(
            ElementId::NamedInteger("effort-fill".into(), moves),
            Animation::new(EFFORT_MOTION),
            move |el, t| el.w(px(effort_fill_w(glide(t)))),
        );
    let knob = div()
        .absolute()
        .top(px(EFFORT_INSET))
        .size(px(EFFORT_KNOB))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(ui::fg())
        .shadow(vec![
            BoxShadow {
                color: ui::backdrop().opacity(0.4),
                offset: point(px(0.), px(2.)),
                blur_radius: px(6.),
                spread_radius: px(0.),
                inset: false,
            },
            BoxShadow {
                color: ui::petal().opacity(0.2 + 0.4 * fraction),
                offset: point(px(0.), px(0.)),
                blur_radius: px(18.),
                spread_radius: px(1.),
                inset: false,
            },
        ])
        .child(
            Icon::new(effort_icon(current))
                .xsmall()
                .text_color(effort_deep()),
        )
        .with_animation(
            ElementId::NamedInteger("effort-knob".into(), moves),
            Animation::new(EFFORT_MOTION),
            move |el, t| el.left(px(EFFORT_INSET + EFFORT_SPAN * glide(t))),
        );
    let zones = levels.iter().enumerate().map(|(i, level)| {
        let left = if i == 0 {
            0.
        } else {
            (effort_stop_x(i - 1, count) + effort_stop_x(i, count)) / 2.
        };
        let right = if i + 1 == count {
            EFFORT_TRACK_W
        } else {
            (effort_stop_x(i, count) + effort_stop_x(i + 1, count)) / 2.
        };
        let (down, drag) = (weak.clone(), weak.clone());
        let (pick, over) = (level.clone(), level.clone());
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(left))
            .w(px(right - left))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                let _ = down.update(cx, |this, cx| this.set_thinking(pick.clone(), cx));
            })
            .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    let _ = drag.update(cx, |this, cx| this.set_thinking(over.clone(), cx));
                }
            })
    });
    div()
        .relative()
        .w(px(EFFORT_TRACK_W))
        .h(px(EFFORT_TRACK_H))
        .rounded_full()
        .bg(ui::well())
        .child(fill)
        .child(knob)
        .children(zones)
}

/// Level names under their stops; the current one in the brand color.
fn effort_labels(levels: &[String], index: usize, weak: &WeakEntity<SessionView>) -> Div {
    let count = levels.len();
    div()
        .relative()
        .w(px(EFFORT_TRACK_W))
        .h(px(16.))
        .children(levels.iter().enumerate().map(|(i, level)| {
            let weak = weak.clone();
            let pick = level.clone();
            div()
                .id(("effort-label", i))
                .absolute()
                .top_0()
                .left(px(effort_stop_x(i, count) - 22.))
                .w(px(44.))
                .flex()
                .justify_center()
                .font_family(ui::MONO)
                .text_size(px(10.5))
                .cursor_pointer()
                .text_color(if i == index { ui::petal() } else { ui::faint() })
                .hover(|s| s.text_color(ui::fg()))
                .on_click(move |_, _, cx| {
                    let _ = weak.update(cx, |this, cx| this.set_thinking(pick.clone(), cx));
                })
                .child(effort_short(level).to_string())
        }))
}

#[cfg(test)]
mod tests {
    use super::{
        EFFORT_INSET, EFFORT_KNOB, EFFORT_TRACK_W, effort_fill_w, effort_fraction, effort_stop_x,
    };

    #[test]
    fn effort_stops_span_the_track_and_unknown_levels_sit_at_the_start() {
        let levels: Vec<String> = ["off", "low", "medium", "high", "xhigh"]
            .map(String::from)
            .to_vec();
        assert_eq!(effort_fraction(&levels, "off"), 0.);
        assert_eq!(effort_fraction(&levels, "medium"), 0.5);
        assert_eq!(effort_fraction(&levels, "xhigh"), 1.);
        // A level the model doesn't list (e.g. after a model switch).
        assert_eq!(effort_fraction(&levels, "max"), 0.);
        // A model with one level has nowhere to slide.
        assert_eq!(effort_fraction(&levels[..1], "off"), 0.);
        // The knob's center rests inside the track at both ends.
        assert_eq!(effort_stop_x(0, 5), EFFORT_INSET + EFFORT_KNOB / 2.);
        assert_eq!(
            effort_stop_x(4, 5),
            EFFORT_TRACK_W - EFFORT_INSET - EFFORT_KNOB / 2.
        );
        // At the top the fill reaches the track's end, never past it.
        assert_eq!(effort_fill_w(1.), EFFORT_TRACK_W);
    }
}
