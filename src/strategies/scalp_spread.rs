//! Scalp Spread Strategy — Market-Making Spread Capture
//!
//! NEW STRATEGY designed for Rust's sub-100ms speed:
//!
//! Concept: Place GTC BUY at best_bid + 1 tick, then immediately
//! queue a SELL at best_ask - 1 tick. Capture the spread.
//!
//! Example:
//!   Book: bid=0.48, ask=0.52 (spread = 4¢)
//!   BUY at 0.49 (inside bid, likely to fill when someone market-sells)
//!   SELL at 0.51 (inside ask, likely to fill when someone market-buys)
//!   Profit: 0.51 - 0.49 = 2¢ per share (minus 0 fee for maker!)
//!
//! Why this works in Rust but NOT Python:
//! - Need to detect spread opportunities and place orders in <100ms
//! - Python's 1s scan loop misses 90% of spread opportunities
//! - Rust's 100ms loop catches them while spread is still wide
//!
//! Risk: If price moves against us before second leg fills, we have
//! a directional position (handled by normal TP/SL system).
//!
//! Requirements:
//! - Spread > 3¢ (need room for both legs inside)
//! - Both sides have depth > $2 (liquidity exists)
//! - Market has > 120s remaining (time for both legs to fill)
//! - NOT during last 60s (books thin out)

use crate::data::gamma::Market;
use crate::data::clob::ClobClient;
use super::Signal;

/// Minimum spread to attempt scalp (in absolute price terms)
const MIN_SPREAD: f64 = 0.03; // 3¢ minimum spread

/// Minimum profit per share after crossing both sides
const MIN_PROFIT_PER_SHARE: f64 = 0.015; // 1.5¢ minimum

/// Minimum depth on each side
const MIN_DEPTH: f64 = 2.0; // $2 minimum depth

pub async fn analyze(market: &Market, clob: &ClobClient) -> Option<Signal> {
    // Need enough time for BOTH legs to fill
    if market.seconds_remaining < 120.0 {
        return None;
    }

    // Don't scalp in the last 2 minutes (books thin)
    let tf_secs = market.timeframe as f64 * 60.0;
    if market.seconds_remaining < tf_secs * 0.40 {
        return None;
    }

    // Get UP token orderbook
    let book = clob.get_orderbook(&market.up_token_id).await.ok()?;

    // Check spread is wide enough
    if book.spread < MIN_SPREAD {
        return None;
    }

    // Check liquidity on both sides
    if book.bid_depth < MIN_DEPTH || book.ask_depth < MIN_DEPTH {
        return None;
    }

    // Calculate scalp prices (inside the spread)
    let buy_price = book.best_bid + 0.01; // 1 tick above best bid
    let sell_price = book.best_ask - 0.01; // 1 tick below best ask
    let profit_per_share = sell_price - buy_price;

    // Need minimum profit
    if profit_per_share < MIN_PROFIT_PER_SHARE {
        return None;
    }

    // Don't scalp if price is extreme (near 0 or 1 = about to resolve)
    if book.mid_price < 0.15 || book.mid_price > 0.85 {
        return None;
    }

    // Confidence: higher spread = more confident the scalp works
    let confidence = (0.60 + (profit_per_share * 5.0)).min(0.80);

    Some(Signal {
        strategy: "scalp_spread".into(),
        coin: market.coin.clone(),
        direction: "UP".into(), // First leg is BUY
        token_id: market.up_token_id.clone(),
        market_id: market.market_id.clone(),
        entry_price: buy_price,
        confidence,
        order_type: "maker".into(), // MUST be maker (0% fee is essential for scalping)
        rationale: format!(
            "Scalp spread: BUY@{:.3} SELL@{:.3} profit={:.3}/sh ({:.1}%). Spread={:.3}",
            buy_price, sell_price, profit_per_share,
            profit_per_share / buy_price * 100.0, book.spread
        ),
        agreement_count: 1,
        conviction_tier: "SINGLE".into(),
        seconds_remaining: market.seconds_remaining,
    })
}
