//! Actions: looking at the machine again, running installs (one or the
//! "Install everything" queue) with a streamed log, and finishing.

use gpui_kit::*;

use super::checks::{install_plan, is_ok, push_lines};
use super::{OnboardingEvent, OnboardingView, Probe};
use crate::features::setup::{self, CheckId, Fix};

impl OnboardingView {
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.checking = true;
        self._probe = cx.spawn(async move |this, cx| {
            let probe = cx.background_executor().spawn(async { Probe::run() }).await;
            let _ = this.update(cx, |this, cx| this.apply(probe, cx));
        });
        cx.notify();
    }

    pub(super) fn apply(&mut self, probe: Probe, cx: &mut Context<Self>) {
        self.checking = false;
        self.platform = Some(probe.platform);
        self.first_run = probe.first_run;
        self.checks = Some(probe.checks);
        if std::mem::take(&mut self.announce) {
            cx.emit(OnboardingEvent::Installed);
        }
        self.next_queued(cx);
        cx.notify();
    }

    /// Start the next queued install that is still needed.
    fn next_queued(&mut self, cx: &mut Context<Self>) {
        if self.installing.is_some() {
            return;
        }
        while let Some(id) = self.queue.pop_front() {
            let needed = self.checks.as_deref().is_some_and(|checks| {
                checks
                    .iter()
                    .any(|c| c.id == id && c.fix == Fix::Auto && !is_ok(c))
            });
            if needed {
                self.install(id, cx);
                return;
            }
        }
    }

    pub(super) fn install_all(&mut self, cx: &mut Context<Self>) {
        if self.installing.is_some() {
            return;
        }
        let Some(checks) = &self.checks else { return };
        self.queue = install_plan(checks);
        self.failure = None;
        self.next_queued(cx);
    }

    /// Run `setup::install` on the background executor, streaming its output
    /// into the row, then look at the machine again.
    pub(super) fn install(&mut self, id: CheckId, cx: &mut Context<Self>) {
        if self.installing.is_some() {
            return;
        }
        self.installing = Some(id);
        self.log_for = Some(id);
        self.log.clear();
        self.failure = None;
        let (tx, rx) = async_channel::unbounded::<String>();
        self._job = cx.spawn(async move |this, cx| {
            let work = cx.background_executor().spawn(async move {
                let log = move |line: String| {
                    let _ = tx.send_blocking(line);
                };
                // `log` (and the sender) drops when this returns, which ends
                // the stream below.
                setup::install(id, &log)
            });
            while let Ok(first) = rx.recv().await {
                let mut chunk = first;
                while let Ok(more) = rx.try_recv() {
                    chunk.push('\n');
                    chunk.push_str(&more);
                }
                let alive = this.update(cx, |this, cx| {
                    push_lines(&mut this.log, &chunk);
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            let result = work.await;
            let _ = this.update(cx, |this, cx| this.finished(id, result, cx));
        });
        cx.notify();
    }

    fn finished(&mut self, id: CheckId, result: Result<(), String>, cx: &mut Context<Self>) {
        self.installing = None;
        match result {
            Ok(()) => self.announce = true,
            Err(message) => {
                self.failure = Some((id, message));
                self.queue.clear();
            }
        }
        self.refresh(cx);
    }

    pub(super) fn finish(&mut self, cx: &mut Context<Self>) {
        setup::mark_done();
        cx.emit(OnboardingEvent::Close);
    }
}
