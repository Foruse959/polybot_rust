//! Pair Arbitrage Strategy — Risk-Free Profit When UP + DOWN < $1
//!
//! From atlantico-academy/polymarket-arbitrage-trading-bot-pack:
//! In a binary market, UP + DOWN always pays $1 (one wins, one loses).
//! If you buy UP at $0.45 and DOWN at $0.50, total cost = $0.95.
//! One of them pays $1, so you lock in $0.05 per share (minus fees).
//!
//! This is RISK-FREE arbitrage — guaranteed profit regardless of outcome.
//!
//! How it works:
//! 1. Fetch both UP and DOWN orderbooks simultaneously
//! 2. If best_ask(UP) + best_ask(DOWN) < 1.0 - fee_buffer:
//!    → BUY both sides, lock in spread as profit
//! 3. First leg: FOK at best_ask (take liquidity)
//! 4. If first leg fills → immediately place second leg
//! 5. If both fill: guaranteed profit = $1 - total_cost
//!
//! Key insight: This works because Polymarket makers sometimes misprice
//! the two sides, creating momentary arbitrage windows of 1-5¢.

use crate::data::gamma::Market;
use crate::data::clob::ClobClient;
use super::Signal;

/// Minimum profit per share to execute (after fees)
/// Taker fee = 3.125% on each leg, so need at least 6.25% spread
/// But with GTC maker (0% fee), we only pay on one leg = 3.125%
const MIN_PROFIT_BPS: f64 = 400.0; // 4% minimum (covers 3.125% fee + 0.875% profit)
const MIN_PROFIT_MAKER: f64 = 0.02; // 2¢ profit per share when using maker orders

/// Analyze market for pair arbitrage opportunity
pub async fn analyze(market: &Market, clob: &ClobClient) -> Option<Signal> {
    // Need both tokens
    if market.up_token_id.is_empty() || market.down_token_id.is_empty() {
        return None;
    }

    // Need enough time (at least 60s for both legs to fill)
    if market.seconds_remaining < 60.0 {
        return None;
    }

    // Fetch BOTH orderbooks concurrently (parallel = faster)
    let (up_book, down_book) = tokio::join!(
        clob.get_orderbook(&market.up_token_id),
        clob.get_orderbook(&market.down_token_id)
    );

    let up_book = up_book.ok()?;
    let down_book = down_book.ok()?;

    // Check if combined ask < $1 (arbitrage exists)
    let combined_ask = up_book.best_ask + down_book.best_ask;
    let profit_per_share = 1.0 - combined_ask;

    // Need minimum profit after fees
    // Strategy: BUY UP as maker (0% fee), BUY DOWN as FOK (3.125% fee)
    // Total fee = DOWN_cost × 3.125%
    let fee_cost = down_book.best_ask * 0.03125;
    let net_profit = profit_per_share - fee_cost;

    if net_profit < MIN_PROFIT_MAKER {
        return None;
    }

    // Check liquidity on both sides (need at least $2 depth each)
    if up_book.ask_depth < 2.0 || down_book.ask_depth < 2.0 {
        return None;
    }

    // Determine which side to buy first (cheaper side first for speed)
    let (first_token, first_price, second_token, second_price, first_dir) = 
        if up_book.best_ask <= down_book.best_ask {
            (&market.up_token_id, up_book.best_ask, &market.down_token_id, down_book.best_ask, "UP")
        } else {
            (&market.down_token_id, down_book.best_ask, &market.up_token_id, up_book.best_ask, "DOWN")
        };

    let confidence = 0.95; // Near-certain profit (only risk = one leg doesn't fill)
    let profit_pct = net_profit / combined_ask * 100.0;

    Some(Signal {
        strategy: "pair_arb".into(),
        coin: market.coin.clone(),
        direction: first_dir.into(), // First leg direction
        token_id: first_token.clone(),
        market_id: market.market_id.clone(),
        entry_price: first_price,
        confidence,
        order_type: "taker".into(), // Speed matters — need both legs fast
        rationale: format!(
            "PAIR ARB: UP@{:.3}+DOWN@{:.3}={:.3} < $1. Profit={:.3} ({:.1}%). Net after fee={:.3}",
            up_book.best_ask, down_book.best_ask, combined_ask,
            profit_per_share, profit_pct, net_profit
        ),
        agreement_count: 1,
        conviction_tier: "MAXIMUM".into(), // Risk-free = maximum conviction
        seconds_remaining: market.seconds_remaining,
    })
}
