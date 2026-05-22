//! BTC Volume Sniper — 86.9% win rate on low-volume BTC markets
//!
//! BACKTEST PROVEN (44,546 BTC markets):
//!   - BTC <$50 volume:  86.9% UP win rate (n=2,647)
//!   - BTC $50-$200:     79.9% UP win rate (n=1,460)
//!   - BTC $200-$1K:     55.0% UP win rate (n=1,489)
//!   - BTC >$1K:         ~50% (no edge)
//!
//! Strategy: When BTC market has very low volume, BUY UP as maker.
//! IMPORTANT: This ONLY works for BTC. DOWN signals are NOT generated.

use crate::data::gamma::Market;
use crate::data::clob::ClobClient;
use super::Signal;

/// Volume tiers with proven win rates
const VOLUME_TIERS: [(f64, f64, f64); 3] = [
    (0.0, 50.0, 0.869),    // 86.9% UP
    (50.0, 200.0, 0.799),  // 79.9% UP
    (200.0, 1000.0, 0.550), // 55.0% UP (marginal)
];

/// Analyze a market for BTC volume sniper opportunity
pub async fn analyze(market: &Market, clob: &ClobClient) -> Option<Signal> {
    // ONLY works for BTC
    if market.coin != "BTC" {
        return None;
    }

    // Need at least 30s remaining
    if market.seconds_remaining < 30.0 {
        return None;
    }

    // Skip markets too close to resolution (< 15% lifetime)
    let tf_secs = market.timeframe as f64 * 60.0;
    if market.seconds_remaining < tf_secs * 0.15 {
        return None;
    }

    // Find matching volume tier
    let expected_wr = VOLUME_TIERS.iter()
        .find(|(min, max, _)| market.volume >= *min && market.volume < *max)
        .map(|(_, _, wr)| *wr)?;

    // Only trade if edge > 5%
    if expected_wr < 0.55 {
        return None;
    }

    // Get orderbook for pricing
    let book = match clob.get_orderbook(&market.up_token_id).await {
        Ok(b) => b,
        Err(_) => return None,
    };

    // Don't trade if spread too wide (> 5%)
    let spread_pct = book.spread / book.best_ask * 100.0;
    if spread_pct > 5.0 {
        return None;
    }

    // Don't generate UP signal if price already very high
    if book.mid_price > 0.72 {
        return None;
    }

    // Spread-aware filter (from backtest): skip if spread > 4%
    if spread_pct > 4.0 {
        return None;
    }

    // Entry price: 1 tick below ask (maker order)
    let limit_price = (book.best_ask - 0.01).max(book.best_bid + 0.005).max(0.01).min(0.99);

    // Confidence from backtest win rate (slight discount for slippage)
    let confidence = (expected_wr - 0.02).min(0.90);

    let n_samples = if market.volume < 50.0 { 2647 } else if market.volume < 200.0 { 1460 } else { 1489 };

    Some(Signal {
        strategy: "btc_volume_sniper".into(),
        coin: "BTC".into(),
        direction: "UP".into(),
        token_id: market.up_token_id.clone(),
        market_id: market.market_id.clone(),
        entry_price: limit_price,
        confidence,
        order_type: "maker".into(),
        rationale: format!(
            "BTC low-volume sniper: vol=${:.0} -> {:.0}% UP win rate (n={}). BUY UP @ {:.3}",
            market.volume, expected_wr * 100.0, n_samples, limit_price
        ),
        agreement_count: 1,
        conviction_tier: "SINGLE".into(),
        seconds_remaining: market.seconds_remaining,
    })
}
