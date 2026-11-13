//! Title bar chrome, tabs, pane tree and pane rendering.

use super::*;

impl Workspace {
    // ── Rendering ───────────────────────────────────────────────────────────

    // The chrome reads each pane's `title`/`running` snapshot, never the
    // views themselves: see `Pane::title`.
    pub(super) fn tab_running(&self, tab: &Tab) -> bool {
        tab.tree
            .leaves()
            .iter()
            .any(|id| self.panes.get(id).is_some_and(|p| p.running))
    }

    /// What the agents of `space` do. A workspace not shown yet since launch
    /// has no panes, so it counts as idle.
    pub(super) fn space_activity(&self, space: &Space) -> Activity {
        let mut activity = Activity::default();
        for id in space.tabs.iter().flat_map(|tab| tab.tree.leaves()) {
            match self.panes.get(&id) {
                Some(pane) if pane.needs_you => activity.needs_you += 1,
                Some(pane) if pane.running => activity.working += 1,
                _ => {}
            }
        }
        activity
    }

    /// What the agents of the other workspaces do, for the workspace chip.
    fn elsewhere_activity(&self) -> Activity {
        let mut total = Activity::default();
        for (index, space) in self.spaces.iter().enumerate() {
            if index != self.current {
                let activity = self.space_activity(space);
                total.working += activity.working;
                total.needs_you += activity.needs_you;
            }
        }
        total
    }

    pub(super) fn pane_title(&self, id: PaneId) -> SharedString {
        self.panes
            .get(&id)
            .map(|p| p.title.clone())
            .unwrap_or_default()
    }

    pub(super) fn render_workspace_chip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.space().name.clone();
        let color = avatar_color(&name);
        let initial = name
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().collect::<String>())
            .unwrap_or_else(|| "·".into());
        // Agents in the other workspaces: a dot here, details in the switcher.
        let elsewhere = self.elsewhere_activity();
        let (tip, dot) = if elsewhere.needs_you > 0 {
            (
                "Switch workspace · an agent in another one needs you",
                Some(ui::warning()),
            )
        } else if elsewhere.working > 0 {
            (
                "Switch workspace · agents are working in another one",
                Some(ui::accent()),
            )
        } else {
            ("Switch workspace", None)
        };
        h_flex()
            .id("workspace-chip")
            .h(px(TITLE_ITEM))
            .pl(px(5.))
            .pr(px(6.))
            .gap(px(7.))
            .flex_none()
            // Same shape and hover as a tab, but no resting fill: it labels the
            // workspace (as in Tern) instead of competing with the active tab.
            .rounded(px(8.))
            .text_size(px(12.5))
            .cursor_pointer()
            .hover(|s| s.bg(ui::surface_hover().opacity(0.6)))
            .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
            // Keep clicks on the chip from turning into window drags: `occlude`
            // hides the title bar's drag area from Windows' hit test, and
            // stopping the mouse-down keeps TitleBar from starting a move.
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_palette(Category::Workspaces, window, cx)
            }))
            .child(
                div()
                    .size(px(18.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .bg(color.opacity(0.18))
                    .text_color(color)
                    .text_size(px(10.5))
                    .font_weight(FontWeight::BOLD)
                    .line_height(relative(1.))
                    .child(initial),
            )
            .child(
                div()
                    .max_w(px(160.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui::fg())
                    .child(name),
            )
            .when_some(dot, |d, color| {
                d.child(div().size(px(6.)).flex_none().rounded_full().bg(color))
            })
            .child(
                Icon::new(IconName::ChevronDown)
                    .xsmall()
                    .text_color(ui::faint()),
            )
    }

    pub(super) fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let space = self.space();
        let weak = cx.entity().downgrade();
        h_flex()
            .gap(px(2.))
            .min_w_0()
            .children(space.tabs.iter().enumerate().map(|(i, tab)| {
                let active = i == space.active;
                let running = self.tab_running(tab);
                let kind = self
                    .panes
                    .get(&tab.focused)
                    .map(|p| p.item.kind)
                    .unwrap_or(PaneKind::Agent);
                let count = tab.tree.len();
                let icon_color = if active { ui::petal() } else { ui::faint() };
                let item = h_flex()
                    .min_w_0()
                    .overflow_hidden()
                    .id(("tab", tab.id))
                    .group("tab")
                    .h(px(TITLE_ITEM))
                    .pl(px(10.))
                    .pr(px(4.))
                    .gap(px(7.))
                    .rounded(px(8.))
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .relative()
                    // The fill alone marks the active tab, like Tern; the petal
                    // icon carries the accent.
                    .when(active, |this| {
                        this.bg(ui::surface_hover().opacity(0.7))
                            .text_color(ui::fg())
                    })
                    .when(!active, |this| {
                        this.text_color(ui::muted())
                            .hover(|s| s.bg(ui::surface_hover().opacity(0.6)).text_color(ui::fg()))
                    })
                    // Keep clicks on tabs from turning into window drags (see the
                    // workspace chip).
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // A double-click on an agent tab renames its session.
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            this.start_tab_rename(i, window, cx);
                        } else {
                            this.activate(i, window, cx);
                        }
                    }))
                    .child(
                        div()
                            .size(px(14.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if running {
                                ui::pulse(div().size(px(7.)).rounded_full().bg(ui::accent()))
                                    .into_any_element()
                            } else if kind == PaneKind::Agent {
                                div()
                                    .text_size(px(13.))
                                    .line_height(relative(1.))
                                    .text_color(icon_color)
                                    .child("❀")
                                    .into_any_element()
                            } else {
                                Icon::new(kind.icon())
                                    .small()
                                    .text_color(icon_color)
                                    .into_any_element()
                            }),
                    )
                    .child(
                        match self.renaming.as_ref().filter(|r| r.pane == tab.focused) {
                            Some(rename) => div()
                                .w(px(160.))
                                .h(px(20.))
                                .px_1()
                                .flex()
                                .items_center()
                                .rounded(px(5.))
                                .bg(ui::well())
                                // Clicks in the field edit it rather than the tab.
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_action(cx.listener(Self::cancel_tab_rename))
                                .child(Input::new(&rename.input).appearance(false).xsmall())
                                .into_any_element(),
                            None => div()
                                .min_w_0()
                                .max_w(px(180.))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .when(active, |d| d.font_weight(FontWeight::MEDIUM))
                                .child(self.pane_title(tab.focused))
                                .into_any_element(),
                        },
                    )
                    .when(count > 1, |d| {
                        d.child(
                            div()
                                .h(px(16.))
                                .min_w(px(16.))
                                .px(px(4.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.))
                                .bg(ui::surface_hover().opacity(0.7))
                                .text_size(px(10.))
                                .line_height(relative(1.))
                                .text_color(ui::muted())
                                .child(count.to_string()),
                        )
                    })
                    .child(
                        div()
                            .id(("close-tab", tab.id))
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .text_color(ui::faint())
                            .invisible()
                            .group_hover("tab", |s| s.visible())
                            .hover(|s| s.bg(ui::surface_hover()).text_color(ui::fg()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(i, window, cx);
                            }))
                            .child(Icon::new(IconName::X).xsmall()),
                    );
                // Tabs shrink (titles end in …) so the title bar's buttons
                // keep their place in a narrow window.
                ui::appear(("tab-in", tab.id), div().flex().min_w_0().child(item))
            }))
            .child(
                div()
                    .flex_none()
                    .ml(px(2.))
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(title_button("new-tab", IconName::Plus).dropdown_menu(
                        move |menu, _, _| {
                            let item = |icon: IconName,
                                        label: &'static str,
                                        detail: &'static str,
                                        keys: &'static str| {
                                PopupMenuItem::element(move |_, _| {
                                    menu_row(icon, label, detail, keys)
                                })
                            };
                            let (agent, terminal) = (weak.clone(), weak.clone());
                            menu.min_w(px(260.))
                                .item(
                                    item(
                                        IconName::Bot,
                                        "Agent tab",
                                        "Gentle Shell in this folder",
                                        if cfg!(target_os = "macos") {
                                            "⌘T"
                                        } else {
                                            "Ctrl+T"
                                        },
                                    )
                                    .on_click(
                                        move |_, window, cx| {
                                            let _ = agent.update(cx, |this, cx| {
                                                this.on_new_tab(&NewTab, window, cx)
                                            });
                                        },
                                    ),
                                )
                                .item(
                                    item(
                                        IconName::SquareTerminal,
                                        "Terminal tab",
                                        "A shell in this folder",
                                        if cfg!(target_os = "macos") {
                                            "⌘⇧T"
                                        } else {
                                            "Ctrl+Shift+T"
                                        },
                                    )
                                    .on_click(
                                        move |_, window, cx| {
                                            let _ = terminal.update(cx, |this, cx| {
                                                this.on_new_terminal_tab(
                                                    &NewTerminalTab,
                                                    window,
                                                    cx,
                                                )
                                            });
                                        },
                                    ),
                                )
                        },
                    )),
            )
    }

    pub(super) fn render_tree(
        &self,
        tab: &Tab,
        node: &PaneTree<PaneId>,
        path: &mut Vec<bool>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            PaneTree::Leaf(id) => self.render_pane(tab, *id, window, cx),
            PaneTree::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let axis = *axis;
                let horizontal = axis == SplitAxis::Horizontal;
                path.push(false);
                let first = self.render_tree(tab, first, path, window, cx);
                path.pop();
                path.push(true);
                let second = self.render_tree(tab, second, path, window, cx);
                path.pop();
                let key = path
                    .iter()
                    .map(|b| if *b { '1' } else { '0' })
                    .collect::<String>();
                let drag = DividerDrag {
                    tab: tab.id,
                    path: path.clone(),
                };
                let (tab_id, here) = (tab.id, path.clone());
                let cell = |child: AnyElement, share: f32| {
                    div()
                        .flex()
                        // Zero basis, grow by share: the space left after the
                        // divider splits exactly at the ratio.
                        .flex_basis(px(0.))
                        .flex_grow(share)
                        .flex_shrink(1.)
                        .min_w_0()
                        .min_h_0()
                        .when(horizontal, |d| d.h_full())
                        .when(!horizontal, |d| d.w_full())
                        .child(child)
                };
                div()
                    .flex()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .when(horizontal, |d| d.flex_row())
                    .when(!horizontal, |d| d.flex_col())
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<DividerDrag>, _, cx| {
                            let drag = event.drag(cx);
                            if drag.tab != tab_id || drag.path != here {
                                return;
                            }
                            let bounds = event.bounds;
                            let position = event.event.position;
                            let share = if horizontal {
                                (position.x - bounds.left()) / bounds.size.width
                            } else {
                                (position.y - bounds.top()) / bounds.size.height
                            };
                            // Keep both sides at least the minimum pane size
                            // (when the split has room for that at all).
                            let (total, min) = if horizontal {
                                (f32::from(bounds.size.width), MIN_PANE_WIDTH)
                            } else {
                                (f32::from(bounds.size.height), MIN_PANE_HEIGHT)
                            };
                            let bound = min / total.max(1.);
                            let share = if bound < 0.5 {
                                share.clamp(bound, 1. - bound)
                            } else {
                                share
                            };
                            let path = here.clone();
                            if let Some(tab) = this.tab_mut()
                                && tab.id == tab_id
                                && tab.tree.set_ratio(&path, share)
                            {
                                this.schedule_save(cx);
                                cx.notify();
                            }
                        },
                    ))
                    .child(cell(first, *ratio))
                    .child(
                        div()
                            .id(SharedString::from(format!("divider-{}-{key}", tab.id)))
                            .group("divider")
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(horizontal, |d| d.w(px(6.)).h_full().cursor_col_resize())
                            .when(!horizontal, |d| d.h(px(6.)).w_full().cursor_row_resize())
                            .on_drag(drag, |_, _, _, cx| cx.new(|_| Invisible))
                            .child(
                                div()
                                    .rounded_full()
                                    .bg(ui::border_strong())
                                    .opacity(0.0)
                                    .group_hover("divider", |s| s.opacity(1.0).bg(ui::petal()))
                                    .when(horizontal, |d| d.w(px(2.)).h(px(40.)))
                                    .when(!horizontal, |d| d.h(px(2.)).w(px(40.))),
                            ),
                    )
                    .child(cell(second, 1.0 - *ratio))
                    .into_any_element()
            }
        }
    }

    pub(super) fn render_pane(
        &self,
        tab: &Tab,
        id: PaneId,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(pane) = self.panes.get(&id) else {
            return div().into_any_element();
        };
        let tiled = tab.tree.len() > 1;
        let focused = tiled && tab.focused == id;
        // The title bar's close button is hovered and this pane is what it
        // closes.
        let closing = self.close_hover && tab.focused == id;
        let title = pane.title.clone();
        let body = div()
            .id(("pane", id))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .rounded_xl()
            .bg(ui::panel())
            // Panes read as glass on their own; only a split's focused pane gets
            // an edge. The width stays so focus changes don't shift content.
            .border_1()
            .border_color(if closing {
                ui::danger()
            } else if focused {
                ui::petal().opacity(0.45)
            } else {
                gpui_kit::transparent_black()
            })
            .overflow_hidden()
            .capture_any_mouse_down(cx.listener(move |this, _, _, cx| {
                if let Some(tab) = this.tab_mut()
                    && tab.focused != id
                    && tab.tree.contains(&id)
                {
                    tab.focused = id;
                    cx.notify();
                }
            }))
            // Dragging a pane over this one: track which zone the pointer is
            // in (only while it's over this pane, and never over itself).
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<PaneDrag>, _, cx| {
                    let bounds = event.bounds;
                    let position = event.event.position;
                    let hint =
                        (bounds.contains(&position) && event.drag(cx).pane != id).then(|| {
                            let x = (position.x - bounds.left()) / bounds.size.width;
                            let y = (position.y - bounds.top()) / bounds.size.height;
                            (id, DropZone::at(x, y))
                        });
                    match hint {
                        Some(hint) if this.drop_hint != Some(hint) => {
                            this.drop_hint = Some(hint);
                            cx.notify();
                        }
                        // Only this pane's own hint is its to clear.
                        None if this.drop_hint.is_some_and(|(pane, _)| pane == id) => {
                            this.drop_hint = None;
                            cx.notify();
                        }
                        _ => {}
                    }
                }),
            )
            .on_drop(cx.listener(move |this, drag: &PaneDrag, window, cx| {
                let zone = this
                    .drop_hint
                    .take()
                    .filter(|(pane, _)| *pane == id)
                    .map_or(DropZone::Center, |(_, zone)| zone);
                this.move_pane(drag.pane, id, zone, window, cx);
            }))
            // The pink border alone marks focus: a straight accent line on top
            // can't follow the rounded corners now that panes have no header.
            // Cached: a notify elsewhere (another pane, the chrome) reuses this
            // pane's last frame instead of rebuilding it.
            .child(
                div().flex_1().min_h_0().child(
                    pane.item
                        .view
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                ),
            )
            .children(
                self.drop_hint
                    .filter(|(pane, _)| *pane == id && cx.has_active_drag())
                    .map(|(_, zone)| drop_overlay(zone)),
            )
            .when(closing, |d| {
                d.child(div().absolute().inset_0().bg(ui::danger().opacity(0.08)))
            });
        // The grip: a pill straddling the pane's top edge (outside the
        // clipped body); drag it onto another pane's edge to move the pane
        // there, or onto its middle to swap the two.
        let grip = h_flex()
            .absolute()
            // Inside the ~4 px gap under the title bar, not over the tabs.
            .top(px(-4.))
            .left_0()
            .right_0()
            .justify_center()
            .child(
                div()
                    .id(("pane-grip", id))
                    .w(px(34.))
                    .h(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(ui::surface())
                    .border_1()
                    .border_color(if focused {
                        ui::petal().opacity(0.45)
                    } else {
                        ui::border()
                    })
                    .text_color(ui::faint())
                    .cursor_grab()
                    .invisible()
                    .when(focused, |d| d.visible())
                    .group_hover(SharedString::from(format!("pane-wrap-{id}")), |s| {
                        s.visible()
                    })
                    .hover(|s| s.text_color(ui::fg()).border_color(ui::border_strong()))
                    .on_drag(
                        PaneDrag {
                            pane: id,
                            title: title.clone(),
                        },
                        |drag, _, _, cx| cx.new(|_| DragChip(drag.title.clone())),
                    )
                    .child(Icon::new(IconName::GripHorizontal).xsmall()),
            );
        ui::appear(
            ("pane-in", id),
            div()
                .group(SharedString::from(format!("pane-wrap-{id}")))
                .relative()
                .flex()
                .size_full()
                .child(body)
                .when(tiled, |d| d.child(grip)),
        )
        .into_any_element()
    }

    pub(super) fn render_body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        match self.tab() {
            Some(tab) => {
                let mut path = Vec::new();
                self.render_tree(tab, &tab.tree, &mut path, window, cx)
            }
            None => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .text_sm()
                .text_color(ui::muted())
                .child("No tabs in this workspace")
                .child(
                    Button::new("empty-new-tab")
                        .primary()
                        .small()
                        .icon(Icon::new(IconName::Plus))
                        .label("New agent tab")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_new_tab(&NewTab, window, cx)),
                        ),
                )
                .into_any_element(),
        }
    }
}

/// Height of the title bar's pills, tabs and buttons.
pub(super) const TITLE_ITEM: f32 = 28.;

/// A quiet square icon button for the title bar.
pub(super) fn title_button(id: &'static str, icon: IconName) -> Button {
    Button::new(id).ghost().xsmall().icon(Icon::new(icon))
}

/// A stable hue per workspace name, for its letter badge.
pub(super) fn avatar_color(name: &str) -> Hsla {
    let hash = name
        .bytes()
        .fold(5381u32, |h, b| h.wrapping_mul(33) ^ u32::from(b));
    hsla((hash % 360) as f32 / 360., 0.55, 0.65, 1.)
}

/// A two-line menu entry: icon, label, muted description, shortcut (none
/// when `keys` is empty).
pub(super) fn menu_row(
    icon: IconName,
    label: &'static str,
    detail: &'static str,
    keys: &'static str,
) -> impl IntoElement {
    h_flex()
        .gap_2p5()
        .py_0p5()
        .w_full()
        .child(
            div()
                .size(px(26.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .border_1()
                .border_color(ui::border_strong())
                .bg(ui::well())
                .text_color(ui::petal())
                .child(Icon::new(icon).small()),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_sm().text_color(ui::fg()).child(label))
                .child(div().text_xs().text_color(ui::muted()).child(detail)),
        )
        .when(!keys.is_empty(), |d| d.child(ui::kbd(keys)))
}

/// Where a dragged pane would land on the pane under the pointer: that half
/// for an edge, the whole pane (a swap) in the middle.
fn drop_overlay(zone: DropZone) -> impl IntoElement {
    let area = div()
        .absolute()
        .m(px(6.))
        .rounded_xl()
        .bg(ui::petal().opacity(0.12))
        .border_2()
        .border_color(ui::petal().opacity(0.6));
    let area = match zone {
        DropZone::Left => area.left_0().top_0().bottom_0().w(relative(0.5)),
        DropZone::Right => area.right_0().top_0().bottom_0().w(relative(0.5)),
        DropZone::Top => area.top_0().left_0().right_0().h(relative(0.5)),
        DropZone::Bottom => area.bottom_0().left_0().right_0().h(relative(0.5)),
        DropZone::Center => area.inset_0(),
    };
    ui::popover_in(("pane-drop", zone as usize), area)
}
