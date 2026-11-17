//! Token use added up over a session, per model, from the rounds the
//! Activity pane records (subagent rounds included).
use crate::activity::{ActivityEvent, ActivityKind};
use crate::providers::TokenUsage;
use std::collections::BTreeMap;

/// Prices in US dollars per million tokens, as the user writes them in
/// `models.toml`. Socium ships no prices: they change, and a wrong number
/// shown as a cost is worse than none.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
}

impl Price {
    fn cost(&self, usage: &TokenUsage) -> f64 {
        let per_token = |tokens: u32, price: f64| tokens as f64 * price / 1_000_000.0;
        per_token(usage.input_tokens, self.input)
            + per_token(usage.output_tokens, self.output)
            + per_token(
                usage.cache_read_input_tokens,
                self.cache_read.unwrap_or(self.input),
            )
            + per_token(
                usage.cache_creation_input_tokens,
                self.cache_write.unwrap_or(self.input),
            )
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionUsage {
    /// Totals keyed by model ID.
    pub by_model: BTreeMap<String, TokenUsage>,
    /// Model calls whose provider reported no usage: the totals and the
    /// cost are then at least what they show.
    unreported: usize,
    /// Which model each open round runs, keyed by agent and round.
    open_rounds: BTreeMap<(Option<String>, usize), String>,
}

impl SessionUsage {
    pub(crate) fn observe(&mut self, event: &ActivityEvent) {
        let key = (event.agent.clone(), event.round);
        match &event.kind {
            ActivityKind::RoundStarted { model } => {
                self.open_rounds.insert(key, model.clone());
            }
            ActivityKind::RoundFinished { usage, .. } => {
                let model = self.open_rounds.remove(&key).unwrap_or_default();
                self.record(model, usage.as_ref());
            }
            ActivityKind::CompactionFinished { model, usage, .. } if !model.is_empty() => {
                self.record(model.clone(), usage.as_ref());
            }
            _ => {}
        }
    }

    /// One model call: its usage added to its model, or counted as
    /// unreported. Every call goes through here, once.
    fn record(&mut self, model: String, usage: Option<&TokenUsage>) {
        let Some(usage) = usage else {
            self.unreported += 1;
            return;
        };
        let total = self.by_model.entry(model).or_default();
        total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
        total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
        total.cache_read_input_tokens = total
            .cache_read_input_tokens
            .saturating_add(usage.cache_read_input_tokens);
        total.cache_creation_input_tokens = total
            .cache_creation_input_tokens
            .saturating_add(usage.cache_creation_input_tokens);
    }

    /// Calls that reported no usage.
    pub fn unreported(&self) -> usize {
        self.unreported
    }

    pub fn total(&self) -> TokenUsage {
        let mut total = TokenUsage::default();
        for usage in self.by_model.values() {
            total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
            total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
            total.cache_read_input_tokens = total
                .cache_read_input_tokens
                .saturating_add(usage.cache_read_input_tokens);
            total.cache_creation_input_tokens = total
                .cache_creation_input_tokens
                .saturating_add(usage.cache_creation_input_tokens);
        }
        total
    }

    /// The cost at the user's prices, or `None` when a model that was used
    /// has no price.
    pub fn cost(&self, price_of: impl Fn(&str) -> Option<Price>) -> Option<f64> {
        // An empty float sum is -0.0, which would show as "$-0.00".
        if self.by_model.is_empty() {
            return None;
        }
        self.by_model
            .iter()
            .map(|(model, usage)| price_of(model).map(|price| price.cost(usage)))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::ActivityClock;

    fn usage(input: u32, output: u32, cache_read: u32) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: cache_read,
            ..TokenUsage::default()
        }
    }

    #[test]
    fn rounds_add_up_per_model_including_subagents() {
        let clock = ActivityClock::start(0);
        let mut session = SessionUsage::default();
        let started = |model: &str| ActivityKind::RoundStarted {
            model: model.into(),
        };
        let finished = |usage| ActivityKind::RoundFinished {
            stop_reason: None,
            usage: Some(usage),
            ttft_ms: None,
            elapsed_ms: 0,
        };
        session.observe(&clock.event(1, started("opus")));
        let mut child = clock.event(1, started("haiku"));
        child.agent = Some("call-1".into());
        session.observe(&child);
        let mut child = clock.event(1, finished(usage(10, 1, 0)));
        child.agent = Some("call-1".into());
        session.observe(&child);
        session.observe(&clock.event(1, finished(usage(100, 10, 900))));

        assert_eq!(session.by_model["opus"], usage(100, 10, 900));
        assert_eq!(session.by_model["haiku"], usage(10, 1, 0));
        assert_eq!(session.total(), usage(110, 11, 900));

        let price = Price {
            input: 4.0,
            output: 20.0,
            cache_read: Some(0.2),
            cache_write: None,
        };
        let cost = session
            .cost(|model| (model == "opus").then_some(price))
            .is_none();
        assert!(cost, "a model without a price leaves the cost unknown");
        let cost = session.cost(|_| Some(price)).unwrap();
        let expected = (110.0 * 4.0 + 11.0 * 20.0 + 900.0 * 0.2) / 1_000_000.0;
        assert!((cost - expected).abs() < 1e-12, "{cost}");
    }

    #[test]
    fn no_cost_before_any_model_is_called() {
        let price = Price {
            input: 4.0,
            output: 20.0,
            cache_read: None,
            cache_write: None,
        };
        assert_eq!(SessionUsage::default().cost(|_| Some(price)), None);
    }

    #[test]
    fn compaction_calls_count_and_unreported_usage_is_not_zero() {
        let clock = ActivityClock::start(0);
        let mut session = SessionUsage::default();
        session.observe(&clock.event(
            1,
            ActivityKind::RoundStarted {
                model: "opus".into(),
            },
        ));
        // The provider sent no usage for this round.
        session.observe(&clock.event(
            1,
            ActivityKind::RoundFinished {
                stop_reason: None,
                usage: None,
                ttft_ms: None,
                elapsed_ms: 0,
            },
        ));
        session.observe(&clock.event(
            0,
            ActivityKind::CompactionFinished {
                covered_messages: 10,
                used_llm: true,
                model: "haiku".into(),
                usage: Some(usage(1_000, 200, 0)),
            },
        ));
        // A fallback that never reached a model costs nothing.
        session.observe(&clock.event(
            0,
            ActivityKind::CompactionFinished {
                covered_messages: 12,
                used_llm: false,
                model: String::new(),
                usage: None,
            },
        ));

        assert_eq!(session.by_model["haiku"], usage(1_000, 200, 0));
        assert_eq!(session.total(), usage(1_000, 200, 0));
        assert_eq!(session.unreported(), 1);
    }
}
