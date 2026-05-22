//! Gamma API Client — Market Discovery
//!
//! Mirrors data/gamma_client.py. Discovers active crypto UP/DOWN markets
//! from the Polymarket Gamma API.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use crate::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Market {
    pub market_id: String,
    pub condition_id: String,
    pub question: String,
    pub coin: String,
    pub timeframe: u32,
    pub up_token_id: String,
    pub down_token_id: String,
    pub seconds_remaining: f64,
    pub volume: f64,
    pub end_time: String,
}

pub struct GammaClient {
    client: reqwest::Client,
    base_url: String,
}

impl GammaClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
            base_url: "https://gamma-api.polymarket.com".into(),
        }
    }

    /// Discover active crypto UP/DOWN markets matching our config
    pub async fn discover_markets(&self, config: &Config) -> Result<Vec<Market>> {
        let mut markets = Vec::new();

        for coin in &config.enabled_coins {
            for &tf in &config.enabled_timeframes {
                let url = format!(
                    "{}/markets?closed=false&tag=crypto-{}-up-down&limit=10",
                    self.base_url,
                    coin.to_lowercase()
                );

                let resp = self.client.get(&url).send().await?;
                if !resp.status().is_success() {
                    continue;
                }

                let raw_markets: Vec<serde_json::Value> = resp.json().await?;

                for raw in raw_markets {
                    if let Some(market) = self.parse_market(&raw, coin, tf) {
                        // Filter: only markets with matching timeframe and enough time left
                        if market.seconds_remaining > 30.0
                            && market.seconds_remaining < (tf as f64 * 60.0 + 60.0)
                        {
                            markets.push(market);
                        }
                    }
                }
            }
        }

        Ok(markets)
    }

    fn parse_market(&self, raw: &serde_json::Value, coin: &str, timeframe: u32) -> Option<Market> {
        let question = raw.get("question")?.as_str()?.to_string();
        let condition_id = raw.get("conditionId")?.as_str()?.to_string();

        // Parse tokens
        let tokens = raw.get("tokens")?.as_array()?;
        if tokens.len() < 2 {
            return None;
        }

        let up_token = tokens.iter().find(|t| {
            t.get("outcome").and_then(|o| o.as_str()) == Some("Yes")
        })?;
        let down_token = tokens.iter().find(|t| {
            t.get("outcome").and_then(|o| o.as_str()) == Some("No")
        })?;

        let up_token_id = up_token.get("token_id")?.as_str()?.to_string();
        let down_token_id = down_token.get("token_id")?.as_str()?.to_string();

        // Parse timing
        let end_time = raw.get("endDateIso")
            .or_else(|| raw.get("end_date_iso"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let seconds_remaining = raw.get("secondsRemaining")
            .or_else(|| raw.get("seconds_remaining"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let volume = raw.get("volume")
            .and_then(|v| v.as_str().and_then(|s| s.parse::<f64>().ok())
                .or_else(|| v.as_f64()))
            .unwrap_or(0.0);

        Some(Market {
            market_id: condition_id.clone(),
            condition_id,
            question,
            coin: coin.to_string(),
            timeframe,
            up_token_id,
            down_token_id,
            seconds_remaining,
            volume,
            end_time,
        })
    }
}
