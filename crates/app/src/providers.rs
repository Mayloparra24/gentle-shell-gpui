//! Providers: sign in to model providers (OAuth or API key) without a terminal.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::{AuthEvent, AuthPrompt, LoginHelper, LoginMessage, ProviderInfo};

use crate::ui;

pub enum ProvidersEvent {
    Close,
    /// Credentials changed; sessions should reload their models.
    Changed,
}

struct Flow {
    provider: String,
    log: Vec<String>,
    auth_url: Option<(String, Option<String>)>,
    device: Option<(String, String)>,
    prompt: Option<(u64, AuthPrompt)>,
    finished: Option<Result<(), String>>,
}

pub struct ProvidersView {
    helper: Option<LoginHelper>,
    error: Option<String>,
    providers: Vec<ProviderInfo>,
    filter: Entity<InputState>,
    answer: Entity<InputState>,
    secret: Entity<InputState>,
    flow: Option<Flow>,
    _io: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ProvidersEvent> for ProvidersView {}

impl ProvidersView {
    pub fn new(cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search providers"));
        let answer = cx.new(|cx| InputState::new(window, cx));
        let secret = cx.new(|cx| InputState::new(window, cx).masked(true));
        let submit_on_enter = |this: &mut Self,
                               _: &Entity<InputState>,
                               event: &InputEvent,
                               window: &mut Window,
                               cx: &mut Context<Self>| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit_answer(window, cx);
            }
        };
        let subscriptions = vec![
            cx.subscribe_in(&secret, window, submit_on_enter),
            cx.subscribe_in(&filter, window, |_, _, _: &InputEvent, _, cx| cx.notify()),
            cx.subscribe_in(&answer, window, submit_on_enter),
        ];
        let mut this = Self {
            helper: None,
            error: None,
            providers: Vec::new(),
            filter,
            answer,
            secret,
            flow: None,
            _io: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.start(cwd, cx);
        this
    }

    fn start(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        let spawned = pi_rpc::resolve_backend(&cwd)
            .and_then(|spec| LoginHelper::spawn(&spec, &pi_rpc::resolve_homes()));
        match spawned {
            Ok((helper, rx)) => {
                let _ = helper.list();
                self.helper = Some(helper);
                self._io = cx.spawn(async move |this, cx| {
                    while let Ok(message) = rx.recv().await {
                        if this
                            .update(cx, |this, cx| this.handle(message, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                });
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    fn handle(&mut self, message: LoginMessage, cx: &mut Context<Self>) {
        match message {
            LoginMessage::Ready => {}
            LoginMessage::Providers(list) => self.providers = list,
            LoginMessage::Prompt { id, prompt } => {
                if let Some(flow) = &mut self.flow {
                    flow.prompt = Some((id, prompt));
                }
            }
            LoginMessage::PromptDismissed(id) => {
                if let Some(flow) = &mut self.flow
                    && flow.prompt.as_ref().is_some_and(|(p, _)| *p == id)
                {
                    flow.prompt = None;
                }
            }
            LoginMessage::Event(event) => {
                if let Some(flow) = &mut self.flow {
                    match event {
                        AuthEvent::AuthUrl { url, instructions } => {
                            cx.open_url(&url);
                            flow.auth_url = Some((url, instructions));
                        }
                        AuthEvent::DeviceCode {
                            user_code,
                            verification_uri,
                        } => {
                            cx.write_to_clipboard(ClipboardItem::new_string(user_code.clone()));
                            cx.open_url(&verification_uri);
                            flow.device = Some((user_code, verification_uri));
                        }
                        AuthEvent::Info { message } | AuthEvent::Progress { message } => {
                            flow.log.push(message)
                        }
                        AuthEvent::Other => {}
                    }
                }
            }
            LoginMessage::Done => {
                if let Some(flow) = &mut self.flow {
                    flow.finished = Some(Ok(()));
                    flow.prompt = None;
                }
                if let Some(helper) = &self.helper {
                    let _ = helper.list();
                }
                cx.emit(ProvidersEvent::Changed);
            }
            LoginMessage::Error(message) => match &mut self.flow {
                Some(flow) => {
                    flow.finished = Some(Err(message));
                    flow.prompt = None;
                }
                None => self.error = Some(message),
            },
            LoginMessage::Exited => {
                self.helper = None;
                if self.error.is_none() {
                    self.error = Some("The sign-in helper stopped.".into());
                }
            }
        }
        cx.notify();
    }

    fn begin(
        &mut self,
        provider: &ProviderInfo,
        oauth: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(helper) = &self.helper else { return };
        if let Err(error) = helper.login(&provider.id, oauth) {
            self.error = Some(format!("{error:#}"));
            return;
        }
        self.flow = Some(Flow {
            provider: provider.name.clone(),
            log: Vec::new(),
            auth_url: None,
            device: None,
            prompt: None,
            finished: None,
        });
        self.clear_inputs(window, cx);
        cx.notify();
    }

    fn clear_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [&self.answer, &self.secret] {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
    }

    fn active_input(&self) -> &Entity<InputState> {
        match self.flow.as_ref().and_then(|f| f.prompt.as_ref()) {
            Some((_, AuthPrompt::Secret { .. })) => &self.secret,
            _ => &self.answer,
        }
    }

    fn logout(&mut self, provider: &ProviderInfo, cx: &mut Context<Self>) {
        if let Some(helper) = &self.helper {
            let _ = helper.logout(&provider.id);
        }
        cx.notify();
    }

    fn submit_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.active_input().read(cx).value().trim().to_string();
        self.respond(value, window, cx);
    }

    fn respond(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(helper), Some(flow)) = (&self.helper, &mut self.flow) else {
            return;
        };
        let Some((id, _)) = flow.prompt.take() else {
            return;
        };
        let _ = helper.answer(id, &value);
        self.clear_inputs(window, cx);
        cx.notify();
    }

    fn cancel_flow(&mut self, cx: &mut Context<Self>) {
        if let (Some(helper), Some(flow)) = (&self.helper, &self.flow)
            && flow.finished.is_none()
        {
            if let Some((id, _)) = &flow.prompt {
                let _ = helper.cancel_prompt(*id);
            }
            let _ = helper.cancel();
        }
        self.flow = None;
        cx.notify();
    }

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.filter.read(cx).value().to_lowercase();
        let matching = self.providers.iter().filter(|p| {
            query.is_empty() || p.name.to_lowercase().contains(&query) || p.id.contains(&query)
        });
        // Signed in first, then subscriptions (what most people sign in
        // with), then everything that only takes an API key.
        let mut groups: [(&str, Vec<&ProviderInfo>); 3] = [
            ("Signed in", Vec::new()),
            ("Subscriptions", Vec::new()),
            ("API keys", Vec::new()),
        ];
        for provider in matching {
            let group = if provider.configured {
                0
            } else if provider.oauth.is_some() {
                1
            } else {
                2
            };
            groups[group].1.push(provider);
        }
        groups[1]
            .1
            .sort_by_key(|p| !p.oauth.as_ref().is_some_and(|o| o.subscription));
        if groups.iter().all(|(_, list)| list.is_empty()) {
            return v_flex()
                .py_8()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(ui::muted())
                        .child("No providers match"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(ui::faint())
                        .child("Try another name or id."),
                );
        }
        let mut column = v_flex().gap_4();
        for (title, list) in groups {
            if list.is_empty() {
                continue;
            }
            let count = list.len();
            let mut rows = Vec::with_capacity(count);
            for (i, provider) in list.into_iter().enumerate() {
                rows.push(self.render_row(provider, i, cx).into_any_element());
            }
            column = column.child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .px_1()
                            .pb_1()
                            .gap_2()
                            .child(ui::section_label(title))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(ui::faint())
                                    .child(count.to_string()),
                            ),
                    )
                    .child(ui::card().p_1().gap_0p5().children(rows)),
            );
        }
        column
    }

    fn render_row(
        &self,
        provider: &ProviderInfo,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = provider.clone();
        let p2 = provider.clone();
        let p3 = provider.clone();
        let recommended =
            !provider.configured && provider.oauth.as_ref().is_some_and(|o| o.subscription);
        let status = provider
            .configured
            .then(|| match provider.source.as_deref() {
                Some("environment") => ("env", ui::info()),
                _ => ("signed in", ui::success()),
            });
        let detail = provider
            .oauth
            .as_ref()
            .map(|o| o.label.clone())
            .or_else(|| {
                provider
                    .source
                    .as_deref()
                    .filter(|s| provider.configured && *s != "stored" && *s != "environment")
                    .map(|s| format!("via {s}"))
            });
        let row = h_flex()
            .px_2()
            .py_1p5()
            .gap_3()
            .rounded_lg()
            .hover(|s| s.bg(ui::surface_hover()))
            .child(badge(&provider.name, provider.configured))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .text_color(ui::fg())
                                    .child(provider.name.clone()),
                            )
                            .when_some(status, |d, (label, color)| {
                                d.child(ui::chip(label, color).flex_shrink_0())
                            }),
                    )
                    .when_some(detail, |d, detail| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(detail),
                        )
                    }),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_1()
                    .when(provider.oauth.is_some(), |d| {
                        d.child(
                            Button::new(SharedString::from(format!("oauth-{}", provider.id)))
                                .small()
                                .when(recommended, |b| b.primary())
                                .when(!recommended, |b| b.ghost())
                                .icon(Icon::new(IconName::LogIn))
                                .label("Sign in")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.begin(&p, true, window, cx)
                                })),
                        )
                    })
                    .when(
                        provider.api_key.as_ref().is_some_and(|k| k.interactive),
                        |d| {
                            d.child(
                                Button::new(SharedString::from(format!("key-{}", provider.id)))
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::KeyRound))
                                    .label("API key")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.begin(&p2, false, window, cx)
                                    })),
                            )
                        },
                    )
                    .when(
                        provider.configured && provider.source.as_deref() == Some("stored"),
                        |d| {
                            d.child(
                                Button::new(SharedString::from(format!("logout-{}", provider.id)))
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::LogOut))
                                    .tooltip("Sign out")
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.logout(&p3, cx)),
                                    ),
                            )
                        },
                    ),
            );
        ui::appear(
            SharedString::from(format!("provider-{}-{index}", provider.id)),
            row,
        )
    }

    fn render_flow(&self, flow: &Flow, cx: &mut Context<Self>) -> impl IntoElement {
        let mut column = v_flex().gap_3().child(
            h_flex()
                .gap_3()
                .child(badge(&flow.provider, flow.finished == Some(Ok(()))))
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().text_xs().text_color(ui::muted()).child("Sign in to"))
                        .child(
                            div()
                                .truncate()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child(flow.provider.clone()),
                        ),
                ),
        );
        if let Some((url, instructions)) = &flow.auth_url {
            let url = url.clone();
            column = column.child(
                ui::card()
                    .gap_3()
                    .p_4()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .child(Icon::new(IconName::Globe).small().text_color(ui::info()).mt_0p5())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(ui::muted())
                                    .child(instructions.clone().unwrap_or_else(|| {
                                        "Your browser opened the sign-in page. Finish there; if it does not return here, paste the code or redirect URL below.".into()
                                    })),
                            ),
                    )
                    .child(
                        h_flex().child(
                            Button::new("open-url")
                                .small()
                                .ghost()
                                .icon(Icon::new(IconName::ExternalLink))
                                .label("Open the sign-in page again")
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        ),
                    ),
            );
        }
        if let Some((code, uri)) = &flow.device {
            let uri = uri.clone();
            column = column.child(
                ui::card()
                    .gap_3()
                    .p_4()
                    .child(div().text_sm().text_color(ui::muted()).child(
                        "Enter this code on the verification page (copied to the clipboard):",
                    ))
                    .child(
                        ui::well_box().items_center().py_3().child(
                            div()
                                .text_2xl()
                                .font_family(ui::MONO)
                                .text_color(ui::champagne())
                                .child(code.clone()),
                        ),
                    )
                    .child(
                        h_flex().child(
                            Button::new("open-device")
                                .small()
                                .ghost()
                                .icon(Icon::new(IconName::ExternalLink))
                                .label("Open verification page")
                                .on_click(move |_, _, cx| cx.open_url(&uri)),
                        ),
                    ),
            );
        }
        let log: Vec<&String> = flow.log.iter().rev().take(3).rev().collect();
        if !log.is_empty() {
            column = column.child(
                ui::well_box().px_3().py_2().gap_0p5().children(
                    log.into_iter()
                        .map(|line| div().text_color(ui::muted()).child(line.clone())),
                ),
            );
        }
        if let Some((_, prompt)) = &flow.prompt {
            column = column.child(match prompt {
                AuthPrompt::Select { message, options } => ui::card()
                    .p_2()
                    .gap_0p5()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .text_sm()
                            .text_color(ui::fg())
                            .child(message.clone()),
                    )
                    .children(options.iter().enumerate().map(|(i, option)| {
                        let id = option.id.clone();
                        Button::new(("auth-option", i))
                            .small()
                            .ghost()
                            .w_full()
                            .justify_start()
                            .icon(Icon::new(IconName::ChevronRight))
                            .label(option.label.clone())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.respond(id.clone(), window, cx)
                            }))
                    }))
                    .into_any_element(),
                AuthPrompt::Text {
                    message,
                    placeholder,
                }
                | AuthPrompt::Secret {
                    message,
                    placeholder,
                }
                | AuthPrompt::ManualCode {
                    message,
                    placeholder,
                } => v_flex()
                    .gap_2()
                    .child(div().text_sm().text_color(ui::fg()).child(message.clone()))
                    .when_some(placeholder.clone(), |d, p| {
                        d.child(div().text_xs().text_color(ui::faint()).child(p))
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                ui::field()
                                    .flex_1()
                                    .min_w_0()
                                    .when(matches!(prompt, AuthPrompt::Secret { .. }), |d| {
                                        d.child(
                                            Icon::new(IconName::KeyRound)
                                                .small()
                                                .text_color(ui::faint()),
                                        )
                                    })
                                    .child(
                                        Input::new(self.active_input()).appearance(false).small(),
                                    ),
                            )
                            .child(
                                Button::new("auth-submit")
                                    .primary()
                                    .icon(Icon::new(IconName::ArrowRight))
                                    .label("Continue")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.submit_answer(window, cx)
                                    })),
                            ),
                    )
                    .into_any_element(),
            });
        }
        match &flow.finished {
            None if flow.prompt.is_none() => {
                column = column.child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(ui::muted())
                        .child(Spinner::new().small().color(ui::accent()))
                        .child("Waiting for the provider…"),
                )
            }
            Some(Ok(())) => column = column.child(notice(
                IconName::CircleCheck,
                ui::success(),
                "Signed in. Tabs without a conversation reload their models now; open a new tab for the others.".into(),
            )),
            Some(Err(error)) => {
                column = column.child(notice(IconName::CircleAlert, ui::danger(), error.clone()))
            }
            None => {}
        }
        column
    }
}

/// Rounded initial badge for a provider.
fn badge(name: &str, signed_in: bool) -> Div {
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let color = if signed_in {
        ui::success()
    } else {
        ui::petal()
    };
    div()
        .flex_shrink_0()
        .size(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_lg()
        .bg(color.opacity(0.12))
        .border_1()
        .border_color(color.opacity(0.25))
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(initial)
}

/// Inline status message with a leading icon.
fn notice(icon: IconName, color: Hsla, text: String) -> Div {
    h_flex()
        .items_start()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_lg()
        .bg(color.opacity(0.08))
        .border_1()
        .border_color(color.opacity(0.22))
        .child(Icon::new(icon).small().text_color(color).mt_0p5())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .text_color(color)
                .child(text),
        )
}

impl Render for ProvidersView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let listing = self.flow.is_none() && !(self.error.is_some() && self.helper.is_none());
        let body: AnyElement = if let Some(error) = &self.error
            && self.helper.is_none()
        {
            notice(IconName::CircleAlert, ui::danger(), error.clone()).into_any_element()
        } else if let Some(flow) = &self.flow {
            self.render_flow(flow, cx).into_any_element()
        } else if self.providers.is_empty() {
            h_flex()
                .py_8()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(ui::muted())
                .child(Spinner::new().small().color(ui::accent()))
                .child("Loading providers…")
                .into_any_element()
        } else {
            self.render_list(cx).into_any_element()
        };
        let flow_finished = self.flow.as_ref().map(|f| f.finished.is_some());
        let (backdrop, panel) = ui::screen(Some(640.));
        backdrop.occlude().child(ui::pop_in(
            "providers-screen",
            panel
                .w_full()
                .max_w(px(640.))
                .child(
                    ui::screen_header()
                        .child(
                            Icon::new(IconName::KeyRound)
                                .small()
                                .text_color(ui::muted()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child("Providers"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child("Credentials go to Gentle Shell's auth.json, like /login."),
                        )
                        .child(ui::kbd("esc"))
                        .child(
                            Button::new("providers-close")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::X))
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(ProvidersEvent::Close)),
                                ),
                        ),
                )
                .when(listing && !self.providers.is_empty(), |d| {
                    d.child(
                        div().px_4().pt_4().pb_1().child(
                            ui::field()
                                .child(Icon::new(IconName::Search).small().text_color(ui::faint()))
                                .child(Input::new(&self.filter).appearance(false).small()),
                        ),
                    )
                })
                .child(
                    div()
                        .id("providers-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_4()
                        .child(body),
                )
                .when_some(flow_finished, |d, finished| {
                    d.child(
                        h_flex()
                            .px_4()
                            .py_3()
                            .gap_2()
                            .border_t_1()
                            .border_color(ui::border())
                            .child(div().flex_1())
                            .child(
                                Button::new("flow-back")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(if finished {
                                        IconName::ArrowLeft
                                    } else {
                                        IconName::X
                                    }))
                                    .label(if finished { "Back" } else { "Cancel" })
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_flow(cx))),
                            ),
                    )
                }),
        ))
    }
}
