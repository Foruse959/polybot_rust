//! Configuration — loads from .env file
//!
//! Mirrors config.py from the Python bot.

use anyhow::Result;

#[derive(Debug, Clone)]
pub struct Config {
    pub trading_mode: String,       // "live" or "paper"
    pub private_key: String,
    pub proxy_wallet: String,
    pub signature_type: u8,         // 3 for V2 accounts
    pub api_key: Option<String>,
    pub api_secret: Option<String>,
    pub passphrase: Option<String>,
    pub enabled_coins: Vec<String>,
    pub enabled_timeframes: Vec<u32>,
    pub starting_balance: f64,
    pub min_order_size: f64,        // $1.00 for FOK
    pub clob_url: String,
    pub gamma_url: String,
    pub chain_id: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();

        Ok(Config {
            trading_mode: std::env::var("TRADING_MODE").unwrap_or_else(|_| "paper".into()),
            private_key: std::env::var("POLY_PRIVATE_KEY").unwrap_or_default(),
            proxy_wallet: std::env::var("POLY_PROXY_WALLET").unwrap_or_default(),
            signature_type: std::env::var("POLY_SIGNATURE_TYPE")
                .unwrap_or_else(|_| "3".into())
                .parse()
                .unwrap_or(3),
            api_key: std::env::var("POLY_API_KEY").ok(),
            api_secret: std::env::var("POLY_API_SECRET").ok(),
            passphrase: std::env::var("POLY_PASSPHRASE").ok(),
            enabled_coins: std::env::var("ENABLED_COINS")
                .unwrap_or_else(|_| "BTC".into())
                .split(',')
                .map(|s| s.trim().to_uppercase())
                .collect(),
            enabled_timeframes: std::env::var("ENABLED_TIMEFRAMES")
                .unwrap_or_else(|_| "5".into())
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect(),
            starting_balance: std::env::var("STARTING_BALANCE")
                .unwrap_or_else(|_| "5.0".into())
                .parse()
                .unwrap_or(5.0),
            min_order_size: 1.0,
            clob_url: "https://clob-v2.polymarket.com".into(),
            gamma_url: "https://gamma-api.polymarket.com".into(),
            chain_id: 137, // Polygon
        })
    }

    pub fn is_live(&self) -> bool {
        self.trading_mode == "live"
    }
}
