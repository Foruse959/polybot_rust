//! Trading Guards — Safety systems that prevent losses
//!
//! From TheOverLordEA/polymarket-hft-engine:
//! 1. De-Peg Killswitch: Halt if Binance/Coinbase diverge >0.15%
//! 2. Volatility Desert: Skip flat markets where spread > expected profit
//! 3. Drift Reconciliation: Sync on-chain state every 15s
//!
//! These run BEFORE any trade decision and can veto entries.

use std::time::Instant;
use tracing::{warn, info};

/// De-Peg Killswitch — halt trading if oracle prices diverge
/// 
/// If Binance BTC price and Coinbase BTC price diverge by >0.15%,
/// it likely means one exchange has stale data or there's a flash crash.
/// Trading on stale oracle data = guaranteed loss.
pub struct DePegGuard {
    pub binance_price: f64,
    pub coinbase_price: f64,
    pub max_divergence_pct: f64,
    pub is_halted: bool,
    pub halt_reason: String,
    pub last_check: Instant,
}

impl DePegGuard {
    pub fn new() -> Self {
        Self {
            binance_price: 0.0,
            coinbase_price: 0.0,
            max_divergence_pct: 0.15, // 0.15% = killswitch threshold
            is_halted: false,
            halt_reason: String::new(),
            last_check: Instant::now(),
        }
    }

    /// Update oracle prices and check for divergence
    pub fn update(&mut self, binance: f64, coinbase: f64) {
        self.binance_price = binance;
        self.coinbase_price = coinbase;
        self.last_check = Instant::now();

        if binance <= 0.0 || coinbase <= 0.0 {
            return; // No data yet
        }

        let mid = (binance + coinbase) / 2.0;
        let divergence_pct = ((binance - coinbase).abs() / mid) * 100.0;

        if divergence_pct > self.max_divergence_pct {
            if !self.is_halted {
                warn!("🚨 DE-PEG KILLSWITCH: Binance={:.2} Coinbase={:.2} divergence={:.3}% > {:.3}%",
                    binance, coinbase, divergence_pct, self.max_divergence_pct);
                self.is_halted = true;
                self.halt_reason = format!(
                    "Oracle de-peg: {:.3}% divergence (max={:.3}%)",
                    divergence_pct, self.max_divergence_pct
                );
            }
        } else if self.is_halted {
            info!("✅ De-peg resolved: divergence={:.3}% < {:.3}%",
                divergence_pct, self.max_divergence_pct);
            self.is_halted = false;
            self.halt_reason.clear();
        }
    }

    /// Can we trade? Returns false if killswitch is active
    #[inline(always)]
    pub fn can_trade(&self) -> bool {
        !self.is_halted
    }
}

/// Volatility Desert Guard — skip flat markets where spread eats all profit
///
/// The #1 loss source: buying at 0.505, BTC doesn't move, selling at 0.50.
/// Loss = spread cost (1¢) × shares every single time.
///
/// This guard checks: is the expected profit > spread cost?
/// If not → skip the trade entirely.
pub struct VolatilityDesertGuard {
    /// Minimum expected edge (in cents) above spread cost to enter
    pub min_edge_above_spread: f64,
    /// Trades skipped by this guard
    pub trades_skipped: u64,
    /// Trades allowed
    pub trades_allowed: u64,
}

impl VolatilityDesertGuard {
    pub fn new() -> Self {
        Self {
            min_edge_above_spread: 0.01, // Need at least 1¢ edge above spread
            trades_skipped: 0,
            trades_allowed: 0,
        }
    }

    /// Should we enter this trade?
    /// 
    /// Parameters:
    /// - spread: current bid-ask spread (e.g., 0.02 = 2¢)
    /// - expected_wr: expected win rate (e.g., 0.869 for volume sniper)
    /// - entry_price: price we'd enter at
    ///
    /// Logic: Expected profit = (WR × TP) - ((1-WR) × SL) - spread_cost
    /// If expected profit < 0 → skip (volatility desert)
    #[inline(always)]
    pub fn should_enter(&mut self, spread: f64, expected_wr: f64, entry_price: f64) -> bool {
        // Cost of entering and exiting = 2 × half-spread (cross bid-ask twice)
        // But with maker entry (0% fee): cost = only exit spread
        let spread_cost = spread / 2.0; // One-way spread cost

        // Expected profit per $1 risked:
        // Win: average gain on 5min BTC UP = ~15-20% of entry
        // Loss: average loss = ~8-12% of entry (SL triggers)
        let avg_win = entry_price * 0.15; // Conservative 15% win
        let avg_loss = entry_price * 0.08; // Conservative 8% loss

        let expected_profit = (expected_wr * avg_win) - ((1.0 - expected_wr) * avg_loss) - spread_cost;

        if expected_profit < self.min_edge_above_spread {
            self.trades_skipped += 1;
            false
        } else {
            self.trades_allowed += 1;
            true
        }
    }

    /// Get skip ratio for monitoring
    pub fn skip_ratio(&self) -> f64 {
        let total = self.trades_skipped + self.trades_allowed;
        if total == 0 { return 0.0; }
        self.trades_skipped as f64 / total as f64
    }
}

/// Drift Reconciliation — sync local state with on-chain reality
///
/// Problem: Bot thinks it has X shares, but on-chain has Y shares.
/// This happens after crashes, restarts, or manual trades.
///
/// Solution: Every 15s, query the Polymarket Data API for actual positions
/// and reconcile with local state.
pub struct DriftReconciler {
    pub last_sync: Instant,
    pub sync_interval_secs: u64,
    pub mismatches_found: u64,
}

impl DriftReconciler {
    pub fn new() -> Self {
        Self {
            last_sync: Instant::now(),
            sync_interval_secs: 15,
            mismatches_found: 0,
        }
    }

    /// Should we sync now?
    #[inline(always)]
    pub fn should_sync(&self) -> bool {
        self.last_sync.elapsed().as_secs() >= self.sync_interval_secs
    }

    /// Mark sync as complete
    pub fn mark_synced(&mut self) {
        self.last_sync = Instant::now();
    }

    /// Record a mismatch (for monitoring)
    pub fn record_mismatch(&mut self) {
        self.mismatches_found += 1;
        warn!("⚠️ Drift detected: local state != on-chain (total: {})", self.mismatches_found);
    }
}
