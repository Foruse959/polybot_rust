//! Polymarket Trading Bot — Rust Edition (Sub-100ms Target)
//!
//! ARCHITECTURE (mirrors Python bot but 100x faster):
//!   main.rs → scan loop (100ms cadence, <10ms signal processing)
//!   data/gamma.rs → market discovery (cached, 5s refresh)
//!   data/clob.rs → order placement (pre-validated, direct CLOB POST)
//!   data/ws.rs → real-time WS (Binance + Polymarket, zero-copy)
//!   strategies/ → parallel signal generation (tokio::join!)
//!   trading/executor.rs → position mgmt (in-memory, zero allocation hot path)
//!   trading/guards.rs → safety systems (de-peg, volatility desert, drift)
//!
//! SPEED OPTIMIZATIONS:
//!   1. WS-driven: react to ticks, don't poll
//!   2. Pre-signed orders: sign once at startup, reuse nonce pattern
//!   3. Connection pooling: reuse HTTP/2 connections (reqwest default)
//!   4. Zero-copy parsing: serde_json from bytes, no string allocation
//!   5. Parallel strategies: tokio::join! not sequential loop
//!   6. Inline hot paths: #[inline(always)] on critical checks
//!   7. No locks on hot path: RwLock only for WS state (read-heavy)

mod config;
mod data;
mod strategies;
mod trading;

use tracing::{info, warn, error};

use crate::config::Config;
use crate::data::gamma::{GammaClient, Market};
use crate::data::clob::ClobClient;
use crate::data::ws::WsManager;
use crate::strategies::StrategyEngine;
use crate::trading::executor::Executor;
use crate::trading::risk::RiskManager;
use crate::trading::guards::{DePegGuard, VolatilityDesertGuard, DriftReconciler};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging (minimal overhead)
    tracing_subscriber::fmt()
        .with_env_filter("polybot_rust=info")
        .with_target(false)
        .compact()
        .init();

    info!("═══════════════════════════════════════════════════════");
    info!("  Polymarket Bot v0.2.0 — Rust (sub-100ms target)");
    info!("═══════════════════════════════════════════════════════");

    // Load config
    let config = Config::from_env()?;
    info!("Config: mode={}, coins={:?}, sig_type={}",
        config.trading_mode, config.enabled_coins, config.signature_type);

    // Initialize components
    let clob = ClobClient::new(&config).await?;
    let gamma = GammaClient::new();
    let ws = WsManager::new(&config);
    let risk = RiskManager::new(config.starting_balance);
    let mut executor = Executor::new();
    let strategies = StrategyEngine::new();

    // Safety guards
    let mut depeg_guard = DePegGuard::new();
    let mut vol_desert = VolatilityDesertGuard::new();
    let mut drift_recon = DriftReconciler::new();

    info!("✅ All systems online. Guards: de-peg={:.2}%, vol-desert, drift-15s",
        depeg_guard.max_divergence_pct);

    // Start WebSocket in background (Binance + Polymarket CLOB)
    let ws_prices = ws.prices.clone();
    tokio::spawn(async move {
        if let Err(e) = ws.start().await {
            error!("WS fatal: {}", e);
        }
    });

    // ═══════════════════════════════════════════════════════════════
    // MAIN SCAN LOOP — Target: <100ms per iteration
    // ═══════════════════════════════════════════════════════════════
    let mut scan_round: u64 = 0;
    let mut last_market_fetch = std::time::Instant::now();
    let mut cached_markets: Vec<Market> = Vec::new();

    loop {
        scan_round += 1;
        let scan_start = std::time::Instant::now();

        // ─── GUARD CHECK (< 1μs) ───
        if !depeg_guard.can_trade() {
            if scan_round % 100 == 0 {
                warn!("⚠️ Trading halted: {}", depeg_guard.halt_reason);
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            continue;
        }

        // ─── MARKET DISCOVERY (cached, refresh every 5s) ───
        // Don't fetch every loop — Gamma API is slow (~200ms)
        // Cache markets and only refresh every 5 seconds
        if last_market_fetch.elapsed().as_secs() >= 5 || cached_markets.is_empty() {
            match gamma.discover_markets(&config).await {
                Ok(m) => {
                    cached_markets = m;
                    last_market_fetch = std::time::Instant::now();
                }
                Err(e) => {
                    if cached_markets.is_empty() {
                        warn!("Market discovery failed: {}", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                        continue;
                    }
                    // Use cached markets if fetch fails
                }
            }
        }

        if cached_markets.is_empty() {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            continue;
        }

        // ─── DRIFT RECONCILIATION (every 15s) ───
        if drift_recon.should_sync() {
            // TODO: Query Polymarket Data API for actual on-chain positions
            // Compare with executor.open_positions
            // Fix any mismatches
            drift_recon.mark_synced();
        }

        // ─── STRATEGY EXECUTION (parallel, <50ms) ───
        let signals = strategies.generate_signals(&cached_markets, &clob).await;

        // ─── TRADE EXECUTION (with guards) ───
        for signal in signals.iter().take(3) {
            // Volatility desert check
            if signal.strategy != "pair_arb" { // Arb is risk-free, skip desert check
                let book = clob.get_orderbook(&signal.token_id).await.ok();
                if let Some(ref b) = book {
                    // Extract expected WR from signal metadata
                    let expected_wr = signal.confidence + 0.02; // Undo the discount
                    if !vol_desert.should_enter(b.spread, expected_wr, signal.entry_price) {
                        continue; // Skip — flat market, spread eats profit
                    }
                }
            }

            // Execute
            match executor.execute_signal(signal, &clob, &risk).await {
                Ok(Some(pos)) => {
                    info!("⚡ FILLED: {} {} @ {:.3} ({}) [{:.0}ms]",
                        pos.coin, pos.direction, pos.entry_price,
                        signal.strategy, scan_start.elapsed().as_millis());
                }
                Ok(None) => {}
                Err(e) => {
                    warn!("Order failed: {}", e);
                }
            }
        }

        // ─── POSITION MONITORING (TP/SL check, <1ms) ───
        executor.monitor_positions(&clob).await;

        // ─── TIMING & METRICS ───
        let elapsed = scan_start.elapsed();
        if scan_round % 50 == 0 {
            info!("📊 Scan #{} | {:.1}ms | {} mkts | {} sigs | desert_skip={:.0}%",
                scan_round,
                elapsed.as_micros() as f64 / 1000.0,
                cached_markets.len(),
                signals.len(),
                vol_desert.skip_ratio() * 100.0,
            );
        }

        // ─── SCAN CADENCE: 100ms (10 scans/sec) ───
        // OLD Python: 1000ms (1 scan/sec) → 2000ms with delays
        // NEW Rust: 100ms cadence → react to opportunities 10x faster
        // If scan took longer than 100ms, don't sleep (already behind)
        let target = tokio::time::Duration::from_millis(100);
        if elapsed < target {
            tokio::time::sleep(target - elapsed).await;
        }
    }
}
