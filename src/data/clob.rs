//! CLOB API Client — Order Placement & Execution
//!
//! Mirrors data/clob_client.py. Handles:
//! - FOK (fill-or-kill) orders for taker entries
//! - GTC (good-til-cancelled) limit orders for maker entries (0% fee)
//! - SELL orders for exits
//! - Order status polling
//!
//! Key constraints:
//! - FOK minimum: $1.00 (price × shares >= 1.0)
//! - GTC minimum: 5 shares
//! - Tick size: 0.01
//! - Signature type: 3 (V2 browser wallet)

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use crate::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderResult {
    pub order_id: String,
    pub status: String,
    pub price: f64,
    pub size: f64,
    pub side: String,
    pub order_type: String,
}

#[derive(Debug, Clone)]
pub struct Orderbook {
    pub token_id: String,
    pub best_bid: f64,
    pub best_ask: f64,
    pub mid_price: f64,
    pub spread: f64,
    pub bid_depth: f64,
    pub ask_depth: f64,
}

pub struct ClobClient {
    client: reqwest::Client,
    base_url: String,
    _signature_type: u8,
    // TODO: Add proper HMAC signing with ethers
}

impl ClobClient {
    pub async fn new(config: &Config) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        Ok(Self {
            client,
            base_url: config.clob_url.clone(),
            _signature_type: config.signature_type,
        })
    }

    /// Get orderbook for a token
    pub async fn get_orderbook(&self, token_id: &str) -> Result<Orderbook> {
        let url = format!("{}/book?token_id={}", self.base_url, token_id);
        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            return Err(anyhow!("Orderbook fetch failed: {}", resp.status()));
        }

        let data: serde_json::Value = resp.json().await?;

        let bids = data.get("bids").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let asks = data.get("asks").and_then(|v| v.as_array()).cloned().unwrap_or_default();

        let best_bid = bids.first()
            .and_then(|b| b.get("price").and_then(|p| p.as_str().and_then(|s| s.parse::<f64>().ok())))
            .unwrap_or(0.01);
        let best_ask = asks.first()
            .and_then(|a| a.get("price").and_then(|p| p.as_str().and_then(|s| s.parse::<f64>().ok())))
            .unwrap_or(0.99);

        let mid_price = (best_bid + best_ask) / 2.0;
        let spread = best_ask - best_bid;

        let bid_depth: f64 = bids.iter().take(10)
            .filter_map(|b| {
                let p = b.get("price")?.as_str()?.parse::<f64>().ok()?;
                let s = b.get("size")?.as_str()?.parse::<f64>().ok()?;
                Some(p * s)
            })
            .sum();

        let ask_depth: f64 = asks.iter().take(10)
            .filter_map(|a| {
                let p = a.get("price")?.as_str()?.parse::<f64>().ok()?;
                let s = a.get("size")?.as_str()?.parse::<f64>().ok()?;
                Some(p * s)
            })
            .sum();

        Ok(Orderbook {
            token_id: token_id.to_string(),
            best_bid,
            best_ask,
            mid_price,
            spread,
            bid_depth,
            ask_depth,
        })
    }

    /// Get mid price for a token (fast)
    pub async fn get_mid_price(&self, token_id: &str) -> Result<f64> {
        let url = format!("{}/midpoint?token_id={}", self.base_url, token_id);
        let resp = self.client.get(&url).send().await?;
        if resp.status().is_success() {
            let data: serde_json::Value = resp.json().await?;
            if let Some(mid) = data.get("mid").and_then(|v| v.as_str().and_then(|s| s.parse::<f64>().ok())) {
                return Ok(mid);
            }
        }
        Err(anyhow!("Failed to get mid price"))
    }

    /// Place a fast order (FOK first, GTC fallback)
    /// Returns OrderResult on success, None if both fail
    pub async fn place_fast_order(
        &self,
        token_id: &str,
        side: &str,
        size_pusd: f64,
        price: f64,
    ) -> Result<OrderResult> {
        // Pre-validation (mirrors Python fix)
        let fok_price = price.max(0.01).min(0.99);
        let min_fok_shares = (1.0 / fok_price).ceil();
        let fok_cost = fok_price * min_fok_shares;

        let gtc_price = if side == "BUY" {
            (fok_price - 0.01).max(0.01)
        } else {
            (fok_price + 0.01).min(0.99)
        };
        let gtc_min_cost = gtc_price * 5.0;

        if size_pusd < fok_cost * 0.95 && size_pusd < gtc_min_cost * 0.95 {
            return Err(anyhow!(
                "Budget ${:.2} too low for both FOK (${:.2}) and GTC (${:.2})",
                size_pusd, fok_cost, gtc_min_cost
            ));
        }

        // TODO: Implement actual CLOB order signing and posting
        // For now, return a paper-mode simulated result
        info!("[CLOB] FOK {} {:.2}sh @ ${:.2} = ${:.2}",
            side, size_pusd / fok_price, fok_price, size_pusd);

        Ok(OrderResult {
            order_id: format!("paper-{:x}", rand_id()),
            status: "MATCHED".into(),
            price: fok_price,
            size: size_pusd / fok_price,
            side: side.into(),
            order_type: "FOK".into(),
        })
    }

    /// Place a GTC limit order (maker, 0% fee)
    pub async fn place_limit_order(
        &self,
        token_id: &str,
        side: &str,
        price: f64,
        size_pusd: f64,
    ) -> Result<OrderResult> {
        let shares = size_pusd / price;
        if shares < 5.0 {
            return Err(anyhow!("GTC needs >= 5 shares, have {:.1}", shares));
        }

        info!("[CLOB] GTC {} {:.2}sh @ ${:.2} (maker 0% fee)", side, shares, price);

        // TODO: Actual CLOB signing
        Ok(OrderResult {
            order_id: format!("paper-{:x}", rand_id()),
            status: "LIVE".into(),
            price,
            size: shares,
            side: side.into(),
            order_type: "GTC".into(),
        })
    }

    /// Place sell order for exit (exact shares)
    pub async fn place_sell(
        &self,
        token_id: &str,
        shares: f64,
        price: f64,
    ) -> Result<OrderResult> {
        if shares * price < 1.0 && shares * 0.99 < 1.0 {
            return Err(anyhow!("Position unsellable: {:.2}sh × ${:.2} < $1 min", shares, price));
        }

        let sell_price = if shares * price < 1.0 {
            // Bump price to meet minimum (FOK fills at market anyway)
            (1.0 / shares).min(0.99)
        } else {
            price
        };

        info!("[CLOB] FOK SELL {:.2}sh @ ${:.3}", shares, sell_price);

        Ok(OrderResult {
            order_id: format!("paper-{:x}", rand_id()),
            status: "MATCHED".into(),
            price: sell_price,
            size: shares,
            side: "SELL".into(),
            order_type: "FOK".into(),
        })
    }
}

fn rand_id() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
}
