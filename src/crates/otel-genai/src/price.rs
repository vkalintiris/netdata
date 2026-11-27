//! What a model call cost, estimated from its token counts and a table of published prices.
//!
//! No provider reports a price with its answer, and a cost an instrumentation library writes on a span is that
//! library's own estimate, so every call is priced here, from one table: `genai-prices`, a snapshot of which is
//! part of this binary. A call whose model the table does not know has no price; it is never priced at zero.
//!
//! Prices change and models appear every week, so the host replaces the snapshot with the table's current
//! data ([`Prices::replace`]), which it downloads from [`REMOTE_DATA_URL`]; this crate makes no requests.
//!
//! A call is looked up by the model that answered, then by the model that was requested, each first under the
//! provider the producer names and then under any provider: behind an OpenAI-compatible proxy the provider a
//! library names is the proxy's dialect, not who serves the model. Last, a name of the form `provider/model`, as
//! LiteLLM and the frameworks built on it write it, is read as that provider's model.

use crate::Call;
use genai_prices::{Calculator, PriceRequest, Usage, UsageKey};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, UNIX_EPOCH};

/// Where the table's current data is published.
pub const REMOTE_DATA_URL: &str = genai_prices::REMOTE_DATA_URL;

/// The data given to [`Prices::replace`] is not a price table.
#[derive(Debug, thiserror::Error)]
#[error("not a valid price table: {0}")]
pub struct InvalidTable(String);

/// The names a lookup remembers; past this many the memory starts over.
const MAX_REMEMBERED_NAMES: usize = 4096;

/// What the table says of a call that reports tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Price {
    /// US dollars.
    Usd(f64),
    /// The table does not know the call's model.
    Unknown,
}

/// The provider and model names of a call, as its producer reports them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Names {
    provider: Option<String>,
    request_model: Option<String>,
    response_model: Option<String>,
}

/// The provider, if any, and the model that a call's names resolve to in the table.
type Resolved = Option<(Option<String>, String)>;

pub struct Prices {
    /// The table that is part of this binary.
    bundled: &'static Calculator,
    /// The table that replaced it, if any.
    current: RwLock<Option<Calculator>>,
    resolved: Mutex<HashMap<Names, Resolved>>,
}

impl Default for Prices {
    fn default() -> Self {
        Self::new()
    }
}

impl Prices {
    /// The table that is part of this binary. Decodes it, which takes tens of milliseconds, once per process.
    pub fn new() -> Self {
        Self {
            bundled: Calculator::bundled(),
            current: RwLock::new(None),
            resolved: Mutex::new(HashMap::new()),
        }
    }

    /// Price later calls from `data`, the table's current data.
    ///
    /// # Errors
    ///
    /// `data` is not a price table; the table in use stays.
    pub fn replace(&self, data: &[u8]) -> Result<(), InvalidTable> {
        let calculator =
            Calculator::from_json(data).map_err(|error| InvalidTable(error.to_string()))?;
        *self.current.write().unwrap() = Some(calculator);
        // A name may resolve differently in the new table.
        self.resolved.lock().unwrap().clear();
        Ok(())
    }

    /// The price of `call` at the rates of the time it ended. `None` for a call that reports no tokens: there is
    /// nothing to price.
    pub fn price(&self, call: &Call) -> Option<Price> {
        let tokens = call.tokens;
        if tokens.input.is_none() && tokens.output.is_none() {
            return None;
        }
        let mut usage = Usage::new();
        for (key, count) in [
            (UsageKey::INPUT_TOKENS, tokens.input),
            (UsageKey::OUTPUT_TOKENS, tokens.output),
            (UsageKey::CACHE_READ_TOKENS, tokens.cache_read),
            (UsageKey::CACHE_WRITE_TOKENS, tokens.cache_write),
            (UsageKey::OUTPUT_REASONING_TOKENS, tokens.reasoning),
        ] {
            if let Some(count) = count {
                usage.insert(key, count as f64);
            }
        }
        let names = Names {
            provider: call.provider.clone(),
            request_model: call.request_model.clone(),
            response_model: call.response_model.clone(),
        };
        let ended = UNIX_EPOCH + Duration::from_nanos(call.end_time_unix_nano);

        let remembered = self.resolved.lock().unwrap().get(&names).cloned();
        let is_new = remembered.is_none();
        let attempts = match remembered {
            Some(None) => return Some(Price::Unknown),
            Some(Some(resolved)) => vec![resolved],
            None => attempts(&names),
        };

        let current = self.current.read().unwrap();
        let calculator = current.as_ref().unwrap_or(self.bundled);
        let mut price = Price::Unknown;
        let mut resolved = None;
        for (provider, model) in attempts {
            let mut request = PriceRequest::new(&model, &usage).with_timestamp(ended);
            if let Some(provider) = &provider {
                request = request.with_provider_id(provider);
            }
            if let Ok(calculation) = calculator.calculate(&request) {
                price = Price::Usd(calculation.total_price);
                resolved = Some((provider, model));
                break;
            }
        }
        drop(current);
        if is_new {
            let mut memory = self.resolved.lock().unwrap();
            if memory.len() >= MAX_REMEMBERED_NAMES {
                memory.clear();
            }
            memory.insert(names, resolved);
        }
        Some(price)
    }
}

/// The provider id under which the table reads a model name of the form `provider/model`.
const PREFIXED: &str = "litellm";

/// The lookups to try for `names`, in order.
fn attempts(names: &Names) -> Vec<(Option<String>, String)> {
    let mut attempts = Vec::new();
    let models = [&names.response_model, &names.request_model];
    for provider in [names.provider.clone(), None, Some(PREFIXED.to_string())] {
        for model in models.into_iter().flatten() {
            let attempt = (provider.clone(), model.clone());
            if !attempts.contains(&attempt) {
                attempts.push(attempt);
            }
        }
    }
    attempts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tokens;

    /// A call that ended on 2026-11-01.
    fn call(
        provider: Option<&str>,
        request: Option<&str>,
        response: Option<&str>,
        tokens: Tokens,
    ) -> Call {
        Call {
            operation: "chat".into(),
            provider: provider.map(str::to_string),
            request_model: request.map(str::to_string),
            response_model: response.map(str::to_string),
            tokens,
            error: false,
            end_time_unix_nano: 1_793_491_200_000_000_000,
            duration_seconds: 1.0,
            time_to_first_chunk_seconds: None,
            response_id: None,
            gateway: false,
        }
    }

    fn million_each() -> Tokens {
        Tokens {
            input: Some(1_000_000),
            output: Some(1_000_000),
            ..Tokens::default()
        }
    }

    fn usd(price: Option<Price>) -> f64 {
        match price {
            Some(Price::Usd(usd)) => usd,
            other => panic!("not priced: {other:?}"),
        }
    }

    #[test]
    fn prices_a_model_its_provider_serves() {
        let prices = Prices::new();
        let direct = call(
            Some("openai"),
            Some("gpt-4o-mini"),
            Some("gpt-4o-mini-2024-07-18"),
            million_each(),
        );
        // 0.15 USD a million input tokens, 0.60 a million output tokens.
        assert!((usd(prices.price(&direct)) - 0.75).abs() < 1e-9);
        // The second lookup of the same names takes the remembered path.
        assert!((usd(prices.price(&direct)) - 0.75).abs() < 1e-9);
    }

    #[test]
    fn prices_cache_reads_as_part_of_the_input() {
        let prices = Prices::new();
        let plain = call(Some("openai"), Some("gpt-4o-mini"), None, million_each());
        let cached = call(
            Some("openai"),
            Some("gpt-4o-mini"),
            None,
            Tokens {
                cache_read: Some(1_000_000),
                ..million_each()
            },
        );
        // Every input token was read from the cache, at half the input rate.
        assert!((usd(prices.price(&cached)) - 0.675).abs() < 1e-9);
        assert!(usd(prices.price(&cached)) < usd(prices.price(&plain)));
    }

    #[test]
    fn falls_back_to_the_model_under_any_provider() {
        let prices = Prices::new();
        // An OpenAI-compatible proxy: the library names the dialect, another provider serves the model.
        let proxied = call(
            Some("openai"),
            Some("claude-sonnet-4-5"),
            None,
            million_each(),
        );
        let direct = call(
            Some("anthropic"),
            Some("claude-sonnet-4-5"),
            None,
            million_each(),
        );
        assert_eq!(prices.price(&proxied), prices.price(&direct));
        // A provider label that names the library.
        let library = call(
            Some("strands-agents"),
            Some("claude-sonnet-4-5"),
            None,
            million_each(),
        );
        assert_eq!(prices.price(&library), prices.price(&direct));
        // No provider at all.
        let bare = call(None, None, Some("claude-sonnet-4-5"), million_each());
        assert_eq!(prices.price(&bare), prices.price(&direct));
    }

    #[test]
    fn reads_a_model_prefixed_with_its_provider() {
        let prices = Prices::new();
        let direct = call(
            Some("anthropic"),
            Some("claude-sonnet-4-5"),
            None,
            million_each(),
        );
        let prefixed = call(
            Some("openai"),
            Some("anthropic/claude-sonnet-4-5"),
            None,
            million_each(),
        );
        assert_eq!(prices.price(&prefixed), prices.price(&direct));
        // The prefix names a provider that does not serve the model.
        let misplaced = call(
            Some("openai"),
            Some("openai/no-such-model"),
            None,
            million_each(),
        );
        assert_eq!(prices.price(&misplaced), Some(Price::Unknown));
    }

    #[test]
    fn a_model_the_table_does_not_know_has_no_price() {
        let prices = Prices::new();
        let unknown = call(
            Some("openai"),
            Some("no-such-model"),
            Some("no-such-model"),
            million_each(),
        );
        assert_eq!(prices.price(&unknown), Some(Price::Unknown));
        assert_eq!(prices.price(&unknown), Some(Price::Unknown));
        let unnamed = call(None, None, None, million_each());
        assert_eq!(prices.price(&unnamed), Some(Price::Unknown));
    }

    #[test]
    fn a_replaced_table_prices_later_calls() {
        let prices = Prices::new();
        let known = call(Some("openai"), Some("gpt-4o-mini"), None, million_each());
        let new = call(Some("acme"), Some("acme-1"), None, million_each());
        assert_eq!(prices.price(&new), Some(Price::Unknown));

        assert!(prices.replace(b"not a table").is_err());
        assert!((usd(prices.price(&known)) - 0.75).abs() < 1e-9);

        let table = r#"[{"id": "acme", "name": "Acme", "api_pattern": "acme", "models": [
            {"id": "acme-1", "match": {"equals": "acme-1"}, "prices": {"input_mtok": 2, "output_mtok": 4}}
        ]}]"#;
        prices.replace(table.as_bytes()).unwrap();
        // The name that had no price is looked up again, in the new table.
        assert!((usd(prices.price(&new)) - 6.0).abs() < 1e-9);
        // The new table is the whole table: it does not know the old one's models.
        assert_eq!(prices.price(&known), Some(Price::Unknown));
    }

    #[test]
    fn a_call_without_tokens_is_not_priced() {
        let prices = Prices::new();
        let failed = call(Some("openai"), Some("gpt-4o-mini"), None, Tokens::default());
        assert_eq!(prices.price(&failed), None);
    }
}
