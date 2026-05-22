//! Risk Manager — Balance Tiers & Position Sizing
//!
//! Mirrors trading/v2_risk_manager.py with 5-tier system:
//! SURVIVAL ($0-5), SEED ($5-15), COMFORT ($15-50),
//! AGGRESSIVE ($50-150), FULL_SEND ($150+)

use tracing::info;

#[derive(Debug, Clone)]
pub struct Tier {
    pub name: &'static str,
    pub min_balance: f64,
    pub max_balance: f64,
    pub min_confidence: f64,
    pub min_agreement: u32,
    pub max_positions: u32,
    pub bet_pct_min: f64,
    pub bet_pct_max: f64,
}

const TIERS: [Tier; 5] = [
    Tier { name: "SURVIVAL", min_balance: 0.0, max_balance: 5.0,
           min_confidence: 0.65, min_agreement: 2, max_positions: 2,
           bet_pct_min: 15.0, bet_pct_max: 25.0 },
    Tier { name: "SEED", min_balance: 5.0, max_balance: 15.0,
           min_confidence: 0.60, min_agreement: 2, max_positions: 4,
           bet_pct_min: 10.0, bet_pct_max: 18.0 },
    Tier { name: "COMFORT", min_balance: 15.0, max_balance: 50.0,
           min_confidence: 0.55, min_agreement: 1, max_positions: 6,
           bet_pct_min: 4.0, bet_pct_max: 8.0 },
    Tier { name: "AGGRESSIVE", min_balance: 50.0, max_balance: 150.0,
           min_confidence: 0.55, min_agreement: 1, max_positions: 10,
           bet_pct_min: 5.0, bet_pct_max: 10.0 },
    Tier { name: "FULL_SEND", min_balance: 150.0, max_balance: f64::MAX,
           min_confidence: 0.50, min_agreement: 1, max_positions: 20,
           bet_pct_min: 6.0, bet_pct_max: 12.0 },
];

pub struct RiskManager {
    pub balance: f64,
    pub peak_balance: f64,
    pub total_pnl: f64,
    pub wins: u32,
    pub losses: u32,
    pub open_positions: u32,
}

impl RiskManager {
    pub fn new(starting_balance: f64) -> Self {
        Self {
            balance: starting_balance,
            peak_balance: starting_balance,
            total_pnl: 0.0,
            wins: 0,
            losses: 0,
            open_positions: 0,
        }
    }

    pub fn get_tier(&self) -> &Tier {
        TIERS.iter()
            .rev()
            .find(|t| self.balance >= t.min_balance)
            .unwrap_or(&TIERS[0])
    }

    pub fn calculate_size(&self, confidence: f64) -> f64 {
        let tier = self.get_tier();

        // Scale bet size by confidence within tier range
        let conf_factor = (confidence - tier.min_confidence) / (1.0 - tier.min_confidence);
        let bet_pct = tier.bet_pct_min + (tier.bet_pct_max - tier.bet_pct_min) * conf_factor.max(0.0);
        let size = self.balance * bet_pct / 100.0;

        // Enforce $1.00 minimum
        size.max(1.0).min(self.balance * 0.5) // Never more than 50% of balance
    }

    pub fn can_trade(&self, confidence: f64, agreement: u32) -> bool {
        let tier = self.get_tier();
        confidence >= tier.min_confidence
            && agreement >= tier.min_agreement
            && self.open_positions < tier.max_positions
            && self.balance >= 1.0
    }

    pub fn drawdown_pct(&self) -> f64 {
        if self.peak_balance > 0.0 {
            (self.peak_balance - self.balance) / self.peak_balance * 100.0
        } else {
            0.0
        }
    }
}
