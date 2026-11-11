//! The devices card: header with the device and status, the live preview (or
//! the device list), and a footer with the status and actions.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{DevicesEvent, DevicesPanel};
use crate::shared::floating;
use crate::shared::ui;
use gentle_core::devices::{Device, State};

pub(super) fn render(
    panel: &mut DevicesPanel,
    window: &mut Window,
    cx: &mut Context<DevicesPanel>,
) -> impl IntoElement {
    // A frame replaced while no window was around can release its atlas tile
    // now; otherwise every 1.5 s capture would leave one behind.
    for image in std::mem::take(&mut panel.pending_drops) {
        let _ = window.drop_image(image);
    }
    panel.card.fit();
    let card = ui::island(cx)
        .absolute()
        .flex()
        .flex_col()
        .occlude()
        .shadow(floating::shadow(panel.card.dragging()))
        .map(|card| floating::position(&panel.card, card))
        .child(header(panel, cx))
        .child(body(panel, cx))
        .child(footer(panel, cx))
        .children(floating::grips(
            &panel.card,
            cx,
            "devices-grip",
            |this, kind, start, cx| this.start_drag(kind, start, cx),
        ));
    let drag_layer = floating::drag_layer(
        &panel.card,
        cx,
        "devices-drag",
        |this, position, cx| this.drag_to(position, cx),
        |this, cx| this.end_drag(cx),
    );
    div()
        .absolute()
        .inset_0()
        .child(floating::area_canvas(&panel.card))
        .child(card)
        .when_some(drag_layer, |root, layer| root.child(layer))
}

fn header(panel: &DevicesPanel, cx: &mut Context<DevicesPanel>) -> impl IntoElement {
    let maximized = panel.card.maximized();
    h_flex()
        .id("devices-header")
        .h(px(floating::HEADER))
        .flex_shrink_0()
        .px_3()
        .gap_2()
        .items_center()
        .cursor(CursorStyle::OpenHand)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if event.click_count == 2 {
                    this.toggle_maximized(cx);
                } else {
                    this.start_drag(floating::DragKind::Move, event.position, cx);
                }
            }),
        )
        .child(
            Icon::new(IconName::Smartphone)
                .small()
                .text_color(ui::petal()),
        )
        .child(
            div()
                .max_w(px(190.))
                .truncate()
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ui::fg())
                .whitespace_nowrap()
                .child(panel.title()),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_size(px(10.5))
                .text_color(ui::faint())
                .child(
                    panel
                        .resolved()
                        .map(Device::subtitle)
                        .unwrap_or_else(|| "Android emulators · iOS simulators".to_string()),
                ),
        )
        .child(
            div()
                .size(px(6.))
                .rounded_full()
                .bg(panel.status_color())
                .flex_shrink_0(),
        )
        .child(
            h_flex()
                .gap_0p5()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Button::new("devices-refresh")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::RefreshCw))
                        .tooltip("Find devices again")
                        .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                )
                .child(
                    Button::new("devices-maximize")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(if maximized {
                            IconName::Minimize2
                        } else {
                            IconName::Maximize2
                        }))
                        .tooltip(if maximized { "Restore" } else { "Maximize" })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_maximized(cx))),
                )
                .child(
                    Button::new("devices-close")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Minus))
                        .tooltip("Hide · keeps running in the top bar")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DevicesEvent::Hide))),
                ),
        )
}

fn body(panel: &DevicesPanel, cx: &mut Context<DevicesPanel>) -> impl IntoElement {
    let show_list = panel.picking || panel.resolved().is_none();
    div()
        .flex_1()
        .min_h_0()
        .mx(px(floating::GRIP))
        .mb(px(floating::GRIP))
        .relative()
        .rounded(px(floating::BODY_RADIUS))
        .bg(ui::well())
        .overflow_hidden()
        .child(if show_list {
            device_list(panel, cx).into_any_element()
        } else {
            preview(panel, cx).into_any_element()
        })
}

fn preview(panel: &DevicesPanel, cx: &mut Context<DevicesPanel>) -> impl IntoElement {
    let device = panel.resolved();
    let note = panel.error.clone().or_else(|| panel.capture_error.clone());
    let area = panel.preview.clone();
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .p_3()
        .gap_3()
        .child(
            canvas(
                move |bounds, window, _| {
                    area.set((
                        bounds.size.width.as_f32(),
                        bounds.size.height.as_f32(),
                        window.scale_factor(),
                    ));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .when_some(panel.frame.clone(), |d, frame| {
            d.child(
                img(frame)
                    .object_fit(ObjectFit::Contain)
                    .size_full()
                    .rounded(px(10.)),
            )
        })
        .when(panel.frame.is_none(), |d| {
            d.when(panel.busy(), |d| {
                d.child(Spinner::new().small().color(ui::accent()))
            })
            .when(!panel.busy(), |d| {
                d.child(Icon::new(IconName::Smartphone).text_color(ui::faint()))
            })
            .child(
                div()
                    .text_sm()
                    .text_color(ui::muted())
                    .text_center()
                    .child(panel.status()),
            )
            .when_some(device.cloned(), |d, device| {
                d.when(device.state == State::Shutdown, |d| {
                    d.child(
                        Button::new("devices-preview-launch")
                            .primary()
                            .small()
                            .icon(Icon::new(IconName::Play))
                            .label("Launch")
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.launch(device.clone(), cx)),
                            ),
                    )
                })
            })
        })
        .when_some(note, |d, note| {
            d.child(
                div()
                    .max_w(px(320.))
                    .text_xs()
                    .text_color(ui::danger())
                    .text_center()
                    .child(note),
            )
        })
}

fn device_list(panel: &DevicesPanel, cx: &mut Context<DevicesPanel>) -> impl IntoElement {
    let rows = panel.devices.iter().enumerate().map(|(index, device)| {
        let selected = panel.selected.as_deref() == Some(device.key().as_str());
        let key = device.key();
        let id = SharedString::from(format!(
            "devices-row-{}-{index}-{key}",
            device.platform.label()
        ));
        let state = match device.state {
            State::Ready => ("Running", ui::success()),
            State::Booting => ("Booting", ui::champagne()),
            State::Shutdown => ("Stopped", ui::faint()),
        };
        let owned = device.clone();
        h_flex()
            .id(id)
            .px_2()
            .py_2()
            .gap_2()
            .rounded(px(ui::ISLAND_ROW))
            .cursor_pointer()
            .when(selected, |d| {
                d.bg(ui::raised())
                    .border_1()
                    .border_color(ui::border_strong())
            })
            .when(!selected, |d| {
                d.border_1()
                    .border_color(transparent_black())
                    .hover(|d| d.bg(ui::surface_hover()))
            })
            .on_click(cx.listener(move |this, _, _, cx| this.pick(owned.clone(), cx)))
            .child(Icon::new(IconName::Smartphone).small().text_color(
                if device.state.is_running() {
                    ui::petal()
                } else {
                    ui::faint()
                },
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .text_color(ui::fg())
                            .child(device.name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(ui::faint())
                            .child(device.subtitle()),
                    ),
            )
            .child(ui::chip(state.0, state.1))
            .when(device.state == State::Shutdown, |d| {
                d.child(Icon::new(IconName::Play).xsmall().text_color(ui::muted()))
            })
    });
    let notes = panel.notes.clone();
    v_flex()
        .id("devices-list")
        .size_full()
        .p_2()
        .gap_1()
        .overflow_y_scroll()
        .when(panel.discovering, |d| {
            d.child(
                h_flex()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .items_center()
                    .child(Spinner::new().xsmall().color(ui::accent()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(ui::muted())
                            .child("Looking for emulators and simulators…"),
                    ),
            )
        })
        .when(panel.devices.is_empty() && !panel.discovering, |d| {
            d.child(
                v_flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .px_4()
                    .py_8()
                    .child(
                        Icon::new(IconName::Smartphone)
                            .text_color(ui::faint()),
                    )
                    .child(
                        div()
                            .max_w(px(300.))
                            .text_center()
                            .text_sm()
                            .text_color(ui::muted())
                            .child("No emulators or simulators found. Create an AVD in Android Studio, or a simulator in Xcode."),
                    ),
            )
        })
        .children(rows)
        .children(notes.into_iter().map(|note| {
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(ui::faint())
                .child(note)
        }))
}

fn footer(panel: &DevicesPanel, cx: &mut Context<DevicesPanel>) -> impl IntoElement {
    let running = panel.running().is_some();
    h_flex()
        .h(px(48.))
        .flex_shrink_0()
        .px_3()
        .gap_2()
        .items_center()
        .border_t_1()
        .border_color(ui::border())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(panel.status_color())
                .child(panel.status()),
        )
        .child(
            Button::new("devices-list-toggle")
                .ghost()
                .small()
                .icon(Icon::new(IconName::List))
                .tooltip(if panel.picking {
                    "Back to the preview"
                } else {
                    "All devices"
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picking = !this.picking;
                    cx.notify();
                })),
        )
        .when(running, |d| {
            d.child(
                Button::new("devices-stop")
                    .ghost()
                    .small()
                    .icon(Icon::new(IconName::Square))
                    .tooltip("Stop the device")
                    .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
            )
        })
}
