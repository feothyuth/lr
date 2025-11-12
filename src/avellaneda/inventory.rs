use super::types::{FillEvent, FillSide, InventorySnapshot};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct InventoryState {
    pub base_balance: f64,
    pub quote_balance: f64,
    pub mid_price: f64,
    pub target_pct: f64,
    pub max_position: f64,
    pub last_update: Instant,
}

impl InventoryState {
    pub fn new(target_pct: f64, max_position: f64) -> Self {
        Self {
            base_balance: 0.0,
            quote_balance: 0.0,
            mid_price: 0.0,
            target_pct,
            max_position,
            last_update: Instant::now(),
        }
    }

    pub fn update_balances(&mut self, base_balance: f64, quote_balance: f64, mid_price: f64) {
        self.base_balance = base_balance;
        self.quote_balance = quote_balance;
        self.mid_price = mid_price;
        self.last_update = Instant::now();
    }

    pub fn apply_fill(&mut self, fill: &FillEvent) {
        match fill.side {
            FillSide::Bid => {
                self.base_balance += fill.size;
                self.quote_balance -= fill.price * fill.size;
            }
            FillSide::Ask => {
                self.base_balance -= fill.size;
                self.quote_balance += fill.price * fill.size;
            }
        }
        self.last_update = fill.timestamp;
    }

    pub fn target_base(&self) -> f64 {
        let total_value = self.total_value();
        if total_value <= f64::EPSILON || self.mid_price <= f64::EPSILON {
            return 0.0;
        }
        let target_value = total_value * self.target_pct;
        target_value / self.mid_price
    }

    pub fn total_value(&self) -> f64 {
        self.base_balance * self.mid_price + self.quote_balance
    }

    pub fn total_base(&self) -> f64 {
        let total_value = self.total_value();
        if self.mid_price <= f64::EPSILON {
            return 0.0;
        }
        total_value / self.mid_price
    }

    pub fn normalized_inventory(&self) -> f64 {
        let total_base = self.total_base();
        if total_base <= f64::EPSILON {
            return 0.0;
        }
        (self.base_balance - self.target_base()) / total_base
    }

    pub fn close_to_limit(&self, tolerance: f64) -> bool {
        self.base_balance.abs() >= (self.max_position * tolerance)
    }

    pub fn snapshot(&self) -> InventorySnapshot {
        InventorySnapshot {
            base_balance: self.base_balance,
            quote_balance: self.quote_balance,
            mid_price: self.mid_price,
            normalized_inventory: self.normalized_inventory(),
            max_position: self.max_position,
        }
    }
}
