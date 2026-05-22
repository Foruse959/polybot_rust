//! Polymarket Trading Bot — Rust Edition
//!
//! Architecture mirrors the Python bot (polymarket-bot-v2):
//!   main.rs (dashboard.py) → scan loop
//!   data/gamma.rs (gamma_client.py) → market discovery
//!   data/clob.rs (clob_client.py) → order placement (FOK/GTC)
//!   data/ws.rs (clob_ws.py + parallel_ws.py) → real-time WS feeds
//!   strategies/volume_sniper.rs (btc_volume_sniper.py) → 86.9% WR strategy
//!   trading/executor.rs (autonomous_executor.py) → position management
//!   trading/risk.rs (v2_risk_manager.py) → tier system + sizing
//!
//! Target: sub-1ms signal→order latency (vs Python's ~500ms)

mod config;
mod data;
mod strategies;
mod trading;

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};

use crate::config::Config;
use crate::data::gamma::{GammaClient, Market};
use crate::data::clob::ClobClient;
use crate::data::ws::WsManager;
use crate::strategies::StrategyEngine;
use crate::trading::executor::Executor;
use crate::trading::risk::RiskManager;

/// Shared application state
pub struct AppState {
    pub config: Config,
    pub clob: ClobClient,
    pub gamma: GammaClient,
    pub ws: WsManager,
    pub risk: RiskManager,
    pub executor: Executor,
    pub strategies: StrategyEngine,
    pub scan_round: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("polybot_rust=info")
        .with_target(false)
        .init();

    info!("═══════════════════════════════════════════════════════");
    info!("  Polymarket Bot v0.1.0 — Rust Edition (sub-ms latency)");
    info!("═══════════════════════════════════════════════════════");

    // Load config from .env
    let config = Config::from_env()?;
    info!("Config loaded: mode={}, coins={:?}", config.trading_mode, config.enabled_coins);

    // Initialize components
    let clob = ClobClient::new(&config).await?;
    info!("CLOB client initialized (sig_type={})", config.signature_type);

    let gamma = GammaClient::new();
    let ws = WsManager::new(&config);
    let risk = RiskManager::new(config.starting_balance);
    let mut executor = Executor::new();
    let strategies = StrategyEngine::new();

    info!("All components initialized. Starting scan loop...");
    info!("");

    // Start WebSocket connections in background
    let ws_handle = tokio::spawn(async move {
        if let Err(e) = ws.start().await {
            error!("WS manager error: {}", e);
        }
    });

    // Main scan loop
    let mut scan_round: u64 = 0;
    loop {
        scan_round += 1;
        let scan_start = std::time::Instant::now();

        // 1. Discover markets from Gamma API
        let markets: Vec<Market> = match gamma.discover_markets(&config).await {
            Ok(m) => m,
            Err(e) => {
                warn!("Market discovery failed: {}", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        if markets.is_empty() {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            continue;
        }

        // 2. Generate signals from all strategies (parallel)
        let signals = strategies.generate_signals(&markets, &clob).await;

        // 3. Execute top signals
        for signal in signals.iter().take(3) {
            match executor.execute_signal(signal, &clob, &risk).await {
                Ok(Some(position)) => {
                    info!("✅ FILLED: {} {} @ {:.3} ({})",
                        position.coin, position.direction,
                        position.entry_price, signal.strategy);
                }
                Ok(None) => {} // Signal skipped (dedup, risk, etc.)
                Err(e) => {
                    warn!("Order failed: {}", e);
                }
            }
        }

        // 4. Monitor open positions
        executor.monitor_positions(&clob).await;

        let elapsed = scan_start.elapsed();
        if scan_round % 10 == 0 {
            info!("Scan #{} | {}ms | {} markets | {} signals",
                scan_round, elapsed.as_millis(), markets.len(), signals.len());
        }

        // 1-second scan cadence (same as optimized Python bot)
        let sleep_duration = tokio::time::Duration::from_millis(1000)
            .saturating_sub(elapsed);
        tokio::time::sleep(sleep_duration).await;
    }
}
