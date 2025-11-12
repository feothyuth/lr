//! Avellaneda-Stoikov market making strategy components.
//!
//! This module exposes the high-level `AvellanedaStrategy` orchestrator together with
//! supporting building blocks (configuration, inventory tracking, volatility estimation,
//! spread optimisation, execution helpers, and market-data adapters).

pub mod config;
pub mod execution;
pub mod inventory;
pub mod market_data;
pub mod participation;
pub mod spreads;
pub mod strategy;
pub mod types;
pub mod volatility;

pub use config::AvellanedaConfig;
pub use strategy::AvellanedaStrategy;
pub use types::{QuoteContext, QuoteOrder, QuotePair, SafetyBounds, StrategyEvent, StrategyParams};
