//! Login-helper IO and sign-in flow state: spawning the helper, applying
//! its messages, and starting, answering or cancelling a flow.

use std::path::PathBuf;

use gpui_kit::component::input::InputState;
use gpui_kit::*;
use pi_rpc::{AuthEvent, AuthPrompt, LoginHelper, LoginMessage, ProviderInfo};

use super::{Flow, ProvidersEvent, ProvidersView};

impl ProvidersView {
    /// Resolving the launcher can wait on the login-shell PATH probe, and
    /// spawning starts Node: both run on the background executor.
    pub(super) fn start(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        self._io = cx.spawn(async move |this, cx| {
            let spawned = cx
                .background_executor()
                .spawn(async move {
                    pi_rpc::resolve_backend(&cwd)
                        .and_then(|spec| LoginHelper::spawn(&spec, &pi_rpc::resolve_homes()))
                })
                .await;
            let rx = this.update(cx, |this, cx| {
                cx.notify();
                match spawned {
                    Ok((helper, rx)) => {
                        let _ = helper.list();
                        this.helper = Some(helper);
                        Some(rx)
                    }
                    Err(error) => {
                        this.error = Some(format!("{error:#}"));
                        None
                    }
                }
            });
            let Ok(Some(rx)) = rx else { return };
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
                crate::features::settings::forget_claude_auth();
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

    pub(super) fn begin(
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

    pub(super) fn active_input(&self) -> &Entity<InputState> {
        match self.flow.as_ref().and_then(|f| f.prompt.as_ref()) {
            Some((_, AuthPrompt::Secret { .. })) => &self.secret,
            _ => &self.answer,
        }
    }

    pub(super) fn logout(&mut self, provider: &ProviderInfo, cx: &mut Context<Self>) {
        if let Some(helper) = &self.helper {
            let _ = helper.logout(&provider.id);
            crate::features::settings::forget_claude_auth();
        }
        cx.notify();
    }

    pub(super) fn submit_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.active_input().read(cx).value().trim().to_string();
        self.respond(value, window, cx);
    }

    pub(super) fn respond(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn cancel_flow(&mut self, cx: &mut Context<Self>) {
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
}
