//! Trade Executor — Position Management
//!
//! Mirrors trading/autonomous_executor.py:
//! - Entry: GTC-first for maker strategies, FOK-first for taker
//! - Exit: Multi-tier cascade (FOK → GTC deep cuts)
//! - Dedup: prevent duplicate entries in same market
//! - TP/SL: Dynamic based on score, timeframe, confidence

use anyhow::Result;
use tracing::{info, warn};
use std::collections::HashMap;
use std::time::Instant;

use crate::data::clob::{ClobClient, OrderResult};
use crate::strategies::Signal;
use crate::trading::risk::RiskManager;

#[derive(Debug, Clone)]
pub struct Position {
    pub id: String,
    pub coin: String,
    pub direction: String,
    pub token_id: String,
    pub market_id: String,
    pub strategy: String,
    pub entry_price: f64,
    pub current_price: f64,
    pub shares: f64,
    pub size_pusd: f64,
    pub tp_pct: f64,
    pub sl_pct: f64,
    pub pnl_pct: f64,
    pub peak_pnl_pct: f64,
    pub opened_at: Instant,
    pub market_close_time: f64,
}

impl Position {
    pub fn update_price(&mut self, price: f64) {
        self.current_price = price;
        self.pnl_pct = if self.direction == "UP" {
            (price - self.entry_price) / self.entry_price * 100.0
        } else {
            (self.entry_price - price) / self.entry_price * 100.0
        };
        if self.pnl_pct > self.peak_pnl_pct {
            self.peak_pnl_pct = self.pnl_pct;
        }
    }

    pub fn should_exit(&self) -> Option<String> {
        if self.pnl_pct >= self.tp_pct {
            return Some(format!("profit_take ({:.1}%)", self.pnl_pct));
        }
        if self.pnl_pct <= -self.sl_pct {
            return Some(format!("stop_loss ({:.1}%)", self.pnl_pct));
        }
        if self.opened_at.elapsed().as_secs() > 330 {
            return Some("timeout".into());
        }
        None
    }
}

pub struct Executor {
    pub open_positions: HashMap<String, Position>,
    pub closed_trades: Vec<Position>,
    dedup: HashMap<String, Instant>,
}

impl Executor {
    pub fn new() -> Self {
        Self {
            open_positions: HashMap::new(),
            closed_trades: Vec::new(),
            dedup: HashMap::new(),
        }
    }

    /// Execute a signal — returns Position if filled, None if skipped
    pub async fn execute_signal(
        &mut self,
        signal: &Signal,
        clob: &ClobClient,
        risk: &RiskManager,
    ) -> Result<Option<Position>> {
        // Dedup check
        let key = format!("{}_{}_{}_{}", signal.coin, signal.direction, signal.market_id, signal.strategy);
        if let Some(last) = self.dedup.get(&key) {
            if last.elapsed().as_secs() < 15 {
                return Ok(None);
            }
        }
        self.dedup.insert(key, Instant::now());

        // Already have position in this market?
        if self.open_positions.values().any(|p| p.market_id == signal.market_id) {
            return Ok(None);
        }

        // Risk validation
        let size_pusd = risk.calculate_size(signal.confidence);
        if size_pusd < 1.0 {
            return Ok(None);
        }

        // Place order
        let result = if signal.order_type == "maker" && signal.seconds_remaining > 120.0 {
            // GTC-first (0% fee)
            clob.place_limit_order(&signal.token_id, "BUY", signal.entry_price, size_pusd).await?
        } else {
            // FOK-first (speed priority)
            clob.place_fast_order(&signal.token_id, "BUY", size_pusd, signal.entry_price).await?
        };

        if result.status == "MATCHED" || result.status == "LIVE" {
            let tp_pct = compute_tp(signal.confidence);
            let sl_pct = compute_sl(signal.confidence);

            let position = Position {
                id: result.order_id.clone(),
                coin: signal.coin.clone(),
                direction: signal.direction.clone(),
                token_id: signal.token_id.clone(),
                market_id: signal.market_id.clone(),
                strategy: signal.strategy.clone(),
                entry_price: result.price,
                current_price: result.price,
                shares: result.size,
                size_pusd,
                tp_pct,
                sl_pct,
                pnl_pct: 0.0,
                peak_pnl_pct: 0.0,
                opened_at: Instant::now(),
                market_close_time: signal.seconds_remaining,
            };

            self.open_positions.insert(position.id.clone(), position.clone());
            return Ok(Some(position));
        }

        Ok(None)
    }

    /// Monitor all open positions for TP/SL
    pub async fn monitor_positions(&mut self, clob: &ClobClient) {
        let mut to_close = Vec::new();

        for (id, pos) in &self.open_positions {
            if let Some(reason) = pos.should_exit() {
                to_close.push((id.clone(), reason));
            }
        }

        for (id, reason) in to_close {
            if let Some(mut pos) = self.open_positions.remove(&id) {
                // Place exit order
                match clob.place_sell(&pos.token_id, pos.shares, pos.current_price).await {
                    Ok(result) => {
                        let emoji = if pos.pnl_pct > 0.0 { "✅" } else { "❌" };
                        info!("{} EXIT: {} {} PnL={:+.1}% reason={}",
                            emoji, pos.coin, pos.direction, pos.pnl_pct, reason);
                    }
                    Err(e) => {
                        warn!("Exit failed for {} {}: {}", pos.coin, pos.direction, e);
                        // Re-queue
                        self.open_positions.insert(id, pos);
                    }
                }
            }
        }
    }
}

/// Compute take-profit % based on confidence
fn compute_tp(confidence: f64) -> f64 {
    if confidence >= 0.85 { 30.0 }
    else if confidence >= 0.75 { 18.0 }
    else { 12.0 }
}

/// Compute stop-loss % based on confidence
fn compute_sl(confidence: f64) -> f64 {
    if confidence >= 0.85 { 15.0 }
    else if confidence >= 0.75 { 9.0 }
    else { 6.0 }
}
