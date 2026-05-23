//! Gamma API Client — Market Discovery
//!
//! Uses SLUG-BASED lookup (same as Python bot):
//! Slug format: "{coin}-updown-{tf}m-{unix_timestamp}"
//! Example: "btc-updown-5m-1716451200"
//!
//! The timestamp is rounded to 5-min boundaries.
//! We check current + next 3 windows to find active markets.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use chrono::{Utc, Timelike};
use tracing::{info, debug, warn};
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
                .timeout(std::time::Duration::from_secs(5))
                .pool_max_idle_per_host(5)
                .build()
                .unwrap(),
            base_url: "https://gamma-api.polymarket.com".into(),
        }
    }

    /// Discover active crypto UP/DOWN markets using slug-based lookup
    /// Same method as Python bot: check multiple time windows in parallel
    pub async fn discover_markets(&self, config: &Config) -> Result<Vec<Market>> {
        let now = Utc::now().timestamp();
        let five_min: i64 = 300;
        let rounded_ts = (now / five_min) * five_min;

        let mut markets = Vec::new();

        // Check current + next 3 windows (same as Python bot)
        for offset in -1..4i64 {
            let ts = rounded_ts + (offset * five_min);
            if ts < now - 300 {
                continue;
            }

            for coin in &config.enabled_coins {
                for &tf in &config.enabled_timeframes {
                    let slug = format!("{}-updown-{}m-{}", coin.to_lowercase(), tf, ts);
                    debug!("Checking slug: {}", slug);

                    if let Some(market) = self.fetch_by_slug(&slug, coin, tf, now).await {
                        // Only include markets with time remaining
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

    /// Fetch a single market by slug from Gamma events API
    async fn fetch_by_slug(&self, slug: &str, coin: &str, timeframe: u32, now_ts: i64) -> Option<Market> {
        let url = format!("{}/events?slug={}", self.base_url, slug);
        debug!("Fetching: {}", url);

        let resp = self.client.get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }

        let data: serde_json::Value = resp.json().await.ok()?;

        // Response is array of events
        let events = if data.is_array() {
            data.as_array()?.clone()
        } else {
            data.get("events")?.as_array()?.clone()
        };

        let event = events.first()?;

        // Get the market within the event
        let event_markets = event.get("markets")?.as_array()?;
        let raw_market = event_markets.first()?;

        // Check if market is active and not closed
        let active = raw_market.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
        let closed = raw_market.get("closed").and_then(|v| v.as_bool()).unwrap_or(true);
        if !active || closed {
            return None;
        }

        // Parse condition ID
        let condition_id = raw_market.get("conditionId")?.as_str()?.to_string();

        // Parse question
        let question = raw_market.get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Parse tokens from clobTokenIds (JSON array as string)
        let clob_token_ids_str = raw_market.get("clobTokenIds")
            .and_then(|v| v.as_str())?;
        let token_ids: Vec<String> = serde_json::from_str(clob_token_ids_str).ok()?;
        if token_ids.len() < 2 {
            return None;
        }

        let up_token_id = token_ids[0].clone();   // YES = UP
        let down_token_id = token_ids[1].clone(); // NO = DOWN

        // Parse end time and calculate seconds remaining
        let end_date_str = raw_market.get("endDate")
            .or_else(|| raw_market.get("endDateIso"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let seconds_remaining = if let Ok(end_dt) = chrono::DateTime::parse_from_rfc3339(end_date_str) {
            (end_dt.timestamp() - now_ts) as f64
        } else {
            // Try parsing as date only
            0.0
        };

        // Parse volume
        let volume = raw_market.get("volume")
            .and_then(|v| {
                v.as_str().and_then(|s| s.parse::<f64>().ok())
                    .or_else(|| v.as_f64())
            })
            .unwrap_or(0.0);

        Some(Market {
            market_id: condition_id.clone(),
            condition_id,
            question,
            coin: coin.to_uppercase(),
            timeframe,
            up_token_id,
            down_token_id,
            seconds_remaining,
            volume,
            end_time: end_date_str.to_string(),
        })
    }
}
