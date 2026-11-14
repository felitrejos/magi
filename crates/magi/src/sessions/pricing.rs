pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub fast: bool,
}

struct Rates {
    input: f64,
    output: f64,
    cache_read: f64,
}

struct Price {
    id: &'static str,
    rates: Rates,
    fast_multiplier: Option<f64>,
    long_prompt: Option<(u64, Rates)>,
}

const fn rates(input: f64, output: f64, cache_read: f64) -> Rates {
    Rates {
        input,
        output,
        cache_read,
    }
}

const fn price(id: &'static str, rates: Rates, fast_multiplier: Option<f64>) -> Price {
    Price {
        id,
        rates,
        fast_multiplier,
        long_prompt: None,
    }
}

const PRICES: [Price; 21] = [
    price("claude-fable-5-2", rates(8.0, 40.0, 0.20), Some(2.0)),
    price("claude-mythos-5-2", rates(8.0, 40.0, 0.20), Some(2.0)),
    price("claude-fable-5-1", rates(10.0, 50.0, 0.25), None),
    price("claude-mythos-5-1", rates(10.0, 50.0, 0.25), None),
    price("claude-fable-5", rates(10.0, 50.0, 1.0), None),
    price("claude-mythos-5", rates(10.0, 50.0, 1.0), None),
    price("claude-opus-5-5", rates(4.0, 20.0, 0.20), Some(2.0)),
    price("claude-opus-5", rates(5.0, 25.0, 0.50), Some(2.0)),
    price("claude-opus-4-8", rates(5.0, 25.0, 0.50), Some(2.0)),
    price("claude-opus-4-7", rates(5.0, 25.0, 0.50), None),
    price("claude-opus-4-6", rates(5.0, 25.0, 0.50), None),
    price("claude-opus-4-5", rates(5.0, 25.0, 0.50), None),
    price("claude-opus-4-1", rates(15.0, 75.0, 1.50), None),
    price("claude-opus-4", rates(15.0, 75.0, 1.50), None),
    price("claude-sonnet-5-5", rates(2.0, 10.0, 0.10), None),
    price("claude-sonnet-5", rates(2.0, 10.0, 0.20), None),
    price("claude-sonnet-4-6", rates(3.0, 15.0, 0.30), None),
    price("claude-sonnet-4-5", rates(3.0, 15.0, 0.30), None),
    price("claude-sonnet-4", rates(3.0, 15.0, 0.30), None),
    Price {
        id: "claude-haiku-5-5",
        rates: rates(0.10, 0.50, 0.01),
        fast_multiplier: None,
        long_prompt: Some((100_000, rates(0.50, 2.50, 0.05))),
    },
    price("claude-haiku-4-5", rates(1.0, 5.0, 0.10), None),
];

const WRITE_5M_MULTIPLIER: f64 = 1.25;
const WRITE_1H_MULTIPLIER: f64 = 2.0;

pub fn cost_micros(model: &str, usage: &Usage) -> Option<u64> {
    let id = normalize(model);
    let price = PRICES.iter().find(|price| price.id == id)?;
    let fast = if usage.fast {
        price.fast_multiplier?
    } else {
        1.0
    };
    let prompt = usage.input + usage.cache_read + usage.cache_write_5m + usage.cache_write_1h;
    let rates = match &price.long_prompt {
        Some((threshold, long)) if prompt > *threshold => long,
        _ => &price.rates,
    };
    let dollars = (usage.input as f64 * rates.input
        + usage.output as f64 * rates.output
        + usage.cache_read as f64 * rates.cache_read
        + usage.cache_write_5m as f64 * rates.input * WRITE_5M_MULTIPLIER
        + usage.cache_write_1h as f64 * rates.input * WRITE_1H_MULTIPLIER)
        / 1_000_000.0;
    Some((dollars * fast * 1_000_000.0).round() as u64)
}

fn normalize(model: &str) -> &str {
    let model = model.split_once('[').map_or(model, |(id, _)| id);
    match model.rsplit_once('-') {
        Some((id, date)) if date.len() == 8 && date.bytes().all(|byte| byte.is_ascii_digit()) => id,
        _ => model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input,
            output,
            cache_read: 0,
            cache_write_5m: 0,
            cache_write_1h: 0,
            fast: false,
        }
    }

    #[test]
    fn prices_input_output_and_cache_per_model() {
        assert_eq!(
            cost_micros("claude-opus-5-5", &usage(1_000_000, 0)),
            Some(4_000_000)
        );
        assert_eq!(
            cost_micros("claude-opus-5-5", &usage(0, 1_000_000)),
            Some(20_000_000)
        );
        let cached = Usage {
            input: 0,
            output: 0,
            cache_read: 1_000_000,
            cache_write_5m: 1_000_000,
            cache_write_1h: 1_000_000,
            fast: false,
        };
        assert_eq!(
            cost_micros("claude-opus-5-5", &cached),
            Some(200_000 + 5_000_000 + 8_000_000)
        );
    }

    #[test]
    fn model_ids_match_exactly_after_suffixes_are_removed() {
        assert_eq!(
            cost_micros("claude-opus-5", &usage(1_000_000, 0)),
            Some(5_000_000)
        );
        assert_eq!(
            cost_micros("claude-opus-5-5[1m]", &usage(1_000_000, 0)),
            Some(4_000_000)
        );
        assert_eq!(
            cost_micros("claude-haiku-4-5-20251001", &usage(1_000_000, 0)),
            Some(1_000_000)
        );
        assert_eq!(
            cost_micros("claude-opus-4-20250514", &usage(1_000_000, 0)),
            Some(15_000_000)
        );
        assert_eq!(cost_micros("claude-opus-5-6", &usage(1, 1)), None);
        assert_eq!(cost_micros("<synthetic>", &usage(10, 10)), None);
    }

    #[test]
    fn haiku_long_prompts_use_the_higher_rate() {
        assert_eq!(
            cost_micros("claude-haiku-5-5", &usage(100_000, 0)),
            Some(10_000)
        );
        assert_eq!(
            cost_micros("claude-haiku-5-5", &usage(100_001, 0)),
            Some(50_001)
        );
    }

    #[test]
    fn fast_mode_is_priced_only_where_its_rate_is_known() {
        let fast = |model| {
            cost_micros(
                model,
                &Usage {
                    fast: true,
                    ..usage(1_000_000, 0)
                },
            )
        };
        assert_eq!(fast("claude-opus-5-5"), Some(8_000_000));
        assert_eq!(fast("claude-fable-5-2"), Some(16_000_000));
        assert_eq!(fast("claude-sonnet-5-5"), None);
    }
}
