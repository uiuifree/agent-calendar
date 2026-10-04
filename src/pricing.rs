/// API 単価での換算（USD / 100 万トークン）。サブスクで使っている分も、API で払った場合の目安として出す。
/// 出典は claude-api スキルの料金表（2026-09-25 時点）。キャッシュ書き込みは 5 分保持が入力の 1.25 倍、
/// 1 時間保持が 2 倍。fast mode は記録に 1 件も無く、キャッシュ分の単価を確かめていないので換算しない
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
}

const PRICES: &[(&str, Price)] = &[
    (
        "claude-fable-5-1",
        Price {
            input: 10.0,
            output: 50.0,
            cache_read: 0.25,
        },
    ),
    (
        "claude-fable-5",
        Price {
            input: 10.0,
            output: 50.0,
            cache_read: 1.0,
        },
    ),
    (
        "claude-opus-5-5",
        Price {
            input: 4.0,
            output: 20.0,
            cache_read: 0.20,
        },
    ),
    (
        "claude-opus-5",
        Price {
            input: 5.0,
            output: 25.0,
            cache_read: 0.50,
        },
    ),
    (
        "claude-opus-4-8",
        Price {
            input: 5.0,
            output: 25.0,
            cache_read: 0.50,
        },
    ),
    (
        "claude-sonnet-5-5",
        Price {
            input: 2.0,
            output: 10.0,
            cache_read: 0.20,
        },
    ),
    (
        "claude-sonnet-5",
        Price {
            input: 2.0,
            output: 10.0,
            cache_read: 0.20,
        },
    ),
    (
        "claude-haiku-4-5",
        Price {
            input: 1.0,
            output: 5.0,
            cache_read: 0.10,
        },
    ),
];

/// `claude-haiku-4-5-20251001` のような日付（8 桁）つきの ID も拾う。
/// 前方一致なので、長い ID（opus-5-5）を短い ID（opus-5）より先に並べてある
pub fn price(model: &str) -> Option<Price> {
    PRICES
        .iter()
        .find(|(id, _)| {
            model == *id
                || model.strip_prefix(id).is_some_and(|rest| {
                    rest.len() == 9
                        && rest.starts_with('-')
                        && rest[1..].chars().all(|c| c.is_ascii_digit())
                })
        })
        .map(|(_, p)| *p)
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Tokens {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_5m: i64,
    pub cache_1h: i64,
}

impl Tokens {
    pub fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_5m += o.cache_5m;
        self.cache_1h += o.cache_1h;
    }
}

/// 単価の分からないモデル・fast mode は None
pub fn cost(model: &str, fast: bool, t: &Tokens) -> Option<f64> {
    if fast {
        return None;
    }
    let p = price(model)?;
    let m = 1e-6;
    Some(
        t.input as f64 * p.input * m
            + t.output as f64 * p.output * m
            + t.cache_read as f64 * p.cache_read * m
            + t.cache_5m as f64 * p.input * 1.25 * m
            + t.cache_1h as f64 * p.input * 2.0 * m,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_a_real_bill() {
        // 要約の試し呼び出しで claude -p が返した total_cost_usd = 0.0164408（sonnet-5-5、書き込みは全部 1 時間保持）
        let t = Tokens {
            input: 2,
            output: 282,
            cache_read: 3144,
            cache_5m: 0,
            cache_1h: 3247,
        };
        let c = cost("claude-sonnet-5-5", false, &t).unwrap();
        assert!((c - 0.0164408).abs() < 1e-9, "{c}");
    }

    #[test]
    fn five_minute_writes_are_1_25x() {
        let t = Tokens {
            cache_5m: 1_000_000,
            ..Default::default()
        };
        assert_eq!(cost("claude-opus-5-5", false, &t), Some(5.0));
    }

    #[test]
    fn model_matching() {
        assert_eq!(
            price("claude-haiku-4-5-20251001"),
            price("claude-haiku-4-5")
        );
        assert_eq!(price("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(price("claude-opus-5").unwrap().input, 5.0);
        assert_eq!(price("claude-fable-5-1").unwrap().cache_read, 0.25);
        assert_eq!(price("claude-fable-5").unwrap().cache_read, 1.0);
        assert!(price("<synthetic>").is_none());
        assert!(price("claude-opus-5-9").is_none());
        assert!(cost("claude-opus-5-5", true, &Tokens::default()).is_none());
    }

    #[test]
    fn add_tokens() {
        let mut a = Tokens {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_5m: 4,
            cache_1h: 5,
        };
        a.add(&a.clone());
        assert_eq!(
            a,
            Tokens {
                input: 2,
                output: 4,
                cache_read: 6,
                cache_5m: 8,
                cache_1h: 10
            }
        );
    }
}
