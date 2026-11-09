//! Usage: native `/gentle:usage` (provider subscription windows) and
//! `/gentle:stats` (local token and cost totals from session files).
//!
//! Parsing mirrors gentle-pi's `lib/shell-usage.ts` and `lib/stats-collector.ts`.
//! Tokens and API keys only ever travel from `auth.json` into the HTTP request
//! header; they are never stored in view state, logged, or rendered.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::*;

use gentle_core::usage::stats::{StatsRange, StatsSummary, aggregate_stats};

mod credentials;
mod fetch;
mod stats;
mod view;

use credentials::{ProviderCard, discover, read_auth};
use fetch::run_fetch;
use stats::{MAX_SESSION_BYTES, StatsData, load_sessions, local_offset_secs};

pub enum UsageEvent {
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Usage,
    Stats,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Metric {
    Tokens,
    Cost,
}

enum UsageState {
    Loading,
    Failed(String),
    Loaded {
        cards: Vec<ProviderCard>,
        others: Vec<String>,
    },
}

enum StatsState {
    Idle,
    Loading,
    Loaded {
        data: Arc<StatsData>,
        offset: i64,
        summary: StatsSummary,
    },
}

pub struct UsageView {
    tab: Tab,
    usage: UsageState,
    stats: StatsState,
    range: StatsRange,
    metric: Metric,
    _usage_task: Task<()>,
    _stats_task: Task<()>,
}

impl EventEmitter<UsageEvent> for UsageView {}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl UsageView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            tab: Tab::Usage,
            usage: UsageState::Loading,
            stats: StatsState::Idle,
            range: StatsRange::All,
            metric: Metric::Tokens,
            _usage_task: Task::ready(()),
            _stats_task: Task::ready(()),
        };
        this.refresh_usage(cx);
        this
    }

    /// Replacing the task cancels an in-flight refresh, so stale results never land.
    fn refresh_usage(&mut self, cx: &mut Context<Self>) {
        self.usage = UsageState::Loading;
        cx.notify();
        let auth_path = pi_rpc::resolve_homes().agent.join("auth.json");
        self._usage_task = cx.spawn(async move |this, cx| {
            let now = now_ms();
            let discovery = cx
                .background_executor()
                .spawn(async move { read_auth(&auth_path).map(|auth| discover(&auth, now)) })
                .await;
            let discovery = match discovery {
                Ok(discovery) => discovery,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.usage = UsageState::Failed(error);
                        cx.notify();
                    });
                    return;
                }
            };
            let mut pending = Vec::new();
            let mut cards = Vec::new();
            for (card, fetch) in discovery.cards {
                if let Some(fetch) = fetch {
                    let id = card.id.clone();
                    pending.push((
                        id,
                        cx.background_executor()
                            .spawn(async move { run_fetch(fetch, now) }),
                    ));
                }
                cards.push(card);
            }
            if this
                .update(cx, |this, cx| {
                    this.usage = UsageState::Loaded {
                        cards,
                        others: discovery.others,
                    };
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            for (id, task) in pending {
                let state = task.await;
                let updated = this.update(cx, |this, cx| {
                    if let UsageState::Loaded { cards, .. } = &mut this.usage
                        && let Some(card) = cards.iter_mut().find(|c| c.id == id)
                    {
                        card.state = state;
                    }
                    cx.notify();
                });
                if updated.is_err() {
                    return;
                }
            }
        });
    }

    fn refresh_stats(&mut self, cx: &mut Context<Self>) {
        self.stats = StatsState::Loading;
        cx.notify();
        let root = pi_rpc::resolve_homes().agent.join("sessions");
        let range = self.range;
        self._stats_task = cx.spawn(async move |this, cx| {
            let (data, offset, summary) = cx
                .background_executor()
                .spawn(async move {
                    let offset = local_offset_secs();
                    let data = load_sessions(&root, MAX_SESSION_BYTES);
                    let summary = aggregate_stats(&data.sessions, range, now_ms(), offset);
                    (Arc::new(data), offset, summary)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // The range may have changed while loading; re-aggregate if so.
                let summary = if this.range == range {
                    summary
                } else {
                    aggregate_stats(&data.sessions, this.range, now_ms(), offset)
                };
                this.stats = StatsState::Loaded {
                    data,
                    offset,
                    summary,
                };
                cx.notify();
            });
        });
    }

    /// Switch to the Local stats tab (the `/gentle:stats` command).
    pub fn show_stats(&mut self, cx: &mut Context<Self>) {
        self.set_tab(Tab::Stats, cx);
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        if tab == Tab::Stats && matches!(self.stats, StatsState::Idle) {
            self.refresh_stats(cx);
        }
        cx.notify();
    }

    fn set_range(&mut self, range: StatsRange, cx: &mut Context<Self>) {
        self.range = range;
        if let StatsState::Loaded {
            data,
            offset,
            summary,
        } = &mut self.stats
        {
            *summary = aggregate_stats(&data.sessions, range, now_ms(), *offset);
        }
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        match self.tab {
            Tab::Usage => self.refresh_usage(cx),
            Tab::Stats => self.refresh_stats(cx),
        }
    }
}
