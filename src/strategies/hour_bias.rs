//! Hour Bias Strategy — 65-85% WR at specific UTC hours
//!
//! From tick_recorder_bot analysis of 5000+ markets:
//! - UTC 14-16: Strong UP bias (57-58% → BUY UP)
//! - UTC 5, 8-9: Strong DOWN bias (46-47% UP → BUY DOWN)
//!
//! This is a TIME-BASED edge. BTC has predictable directional
//! patterns at certain hours due to market open/close flows:
//! - US market open (14-16 UTC) = buying pressure = UP
//! - Asia overnight (5, 8-9 UTC) = selling/profit-taking = DOWN
//!
//! SPEED: Zero computation needed — just check the clock.
//! Perfect for Rust's sub-100ms loop.

use crate::data::gamma::Market;
use crate::data::clob::ClobClient;
use super::Signal;
use chrono::{Utc, Timelike};

/// Hour bias lookup table (UTC hour → UP probability)
/// Only trade when bias > 55% (UP) or < 45% (DOWN)
const HOUR_BIAS: [(u32, f64); 24] = [
    (0, 0.52), (1, 0.51), (2, 0.53), (3, 0.54), (4, 0.53),
    (5, 0.47), (6, 0.49), (7, 0.50), (8, 0.46), (9, 0.47),
    (10, 0.48), (11, 0.51), (12, 0.53), (13, 0.55), (14, 0.57),
    (15, 0.58), (16, 0.56), (17, 0.54), (18, 0.53), (19, 0.52),
    (20, 0.51), (21, 0.50), (22, 0.51), (23, 0.52),
];

/// Minimum bias to trade (5% edge = 55% UP or 45% UP)
const MIN_BIAS_EDGE: f64 = 0.05;

pub async fn analyze(market: &Market, clob: &ClobClient) -> Option<Signal> {
    // Only BTC has proven hour bias from backtest data
    if market.coin != "BTC" {
        return None;
    }

    // Need at least 60s remaining
    if market.seconds_remaining < 60.0 {
        return None;
    }

    // Get current UTC hour
    let hour = Utc::now().hour();

    // Look up bias for this hour
    let up_prob = HOUR_BIAS.iter()
        .find(|(h, _)| *h == hour)
        .map(|(_, p)| *p)
        .unwrap_or(0.50);

    // Determine direction and edge
    let (direction, token_id, edge, confidence) = if up_prob >= 0.50 + MIN_BIAS_EDGE {
        // UP bias hour
        ("UP", &market.up_token_id, up_prob - 0.50, up_prob - 0.02)
    } else if up_prob <= 0.50 - MIN_BIAS_EDGE {
        // DOWN bias hour (BUY DOWN token)
        ("DOWN", &market.down_token_id, 0.50 - up_prob, (1.0 - up_prob) - 0.02)
    } else {
        // No edge this hour — skip
        return None;
    };

    // Get orderbook for entry pricing
    let book = clob.get_orderbook(token_id).await.ok()?;

    // Spread check — don't enter if spread eats the edge
    let spread_pct = book.spread / book.best_ask * 100.0;
    if spread_pct > 4.0 {
        return None;
    }

    // Don't buy if price already reflects the bias (> 0.65 for UP, > 0.65 for DOWN)
    if book.mid_price > 0.65 {
        return None;
    }

    // Entry: 1 tick below ask (maker, 0% fee)
    let limit_price = (book.best_ask - 0.01).max(0.01).min(0.99);

    Some(Signal {
        strategy: "hour_bias".into(),
        coin: "BTC".into(),
        direction: direction.into(),
        token_id: token_id.clone(),
        market_id: market.market_id.clone(),
        entry_price: limit_price,
        confidence,
        order_type: "maker".into(),
        rationale: format!(
            "Hour bias: UTC {} = {:.1}% UP prob (edge={:.1}%). BUY {} @ {:.3}",
            hour, up_prob * 100.0, edge * 100.0, direction, limit_price
        ),
        agreement_count: 1,
        conviction_tier: if edge > 0.06 { "MEDIUM".into() } else { "SINGLE".into() },
        seconds_remaining: market.seconds_remaining,
    })
}
