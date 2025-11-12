use super::types::{QuoteContext, QuoteOrder, QuotePair, SafetyBounds, StrategyParams};

#[derive(Clone, Debug)]
pub struct QuoteComputation {
    pub pair: QuotePair,
    pub context: QuoteContext,
}

pub fn reservation_price(mid: f64, q: f64, gamma: f64, sigma: f64, time_left: f64) -> f64 {
    let variance_horizon = (sigma * sigma) * time_left;
    mid - q * gamma * variance_horizon
}

pub fn optimal_spread(gamma: f64, sigma: f64, time_left: f64, kappa: f64) -> f64 {
    let risk_term = gamma * (sigma * sigma) * time_left;
    let liquidity_term = (2.0 / gamma) * (1.0 + gamma / kappa).ln();
    (risk_term + liquidity_term).max(0.0)
}

pub fn spread_to_bps(spread: f64, mid: f64) -> f64 {
    if mid.abs() <= f64::EPSILON {
        return 0.0;
    }
    (spread / mid) * 10_000.0
}

fn min_required_spread_bps(bounds: &SafetyBounds) -> f64 {
    let mut min_required_bps = bounds.min_spread_bps.max(0.0);
    if bounds.min_edge_bps_total > 0.0 {
        let fee_total = 2.0 * bounds.maker_fee_bps;
        let floor = (bounds.min_edge_bps_total - fee_total).max(0.0);
        min_required_bps = min_required_bps.max(floor);
    }
    min_required_bps
}

pub fn clamp_spread(spread: f64, mid: f64, bounds: &SafetyBounds) -> f64 {
    if mid.abs() <= f64::EPSILON {
        return spread;
    }
    let spread_bps = spread_to_bps(spread, mid);
    let clamped_bps = spread_bps
        .max(min_required_spread_bps(bounds))
        .min(bounds.max_spread_bps);
    clamped_bps * mid / 10_000.0
}

pub fn compute_quote(
    mid: f64,
    q: f64,
    sigma: f64,
    params: &StrategyParams,
    bounds: &SafetyBounds,
) -> Option<QuoteComputation> {
    if mid <= 0.0 {
        return None;
    }
    let time_left = params.time_left_seconds();
    let reservation = reservation_price(mid, q, params.gamma, sigma, time_left);
    let raw_spread = optimal_spread(params.gamma, sigma, time_left, params.kappa);
    let raw_spread_bps = spread_to_bps(raw_spread, mid);

    let min_required_bps = min_required_spread_bps(bounds);
    let effective_spread_bps = raw_spread_bps
        .max(min_required_bps)
        .min(bounds.max_spread_bps);
    let effective_spread = mid * effective_spread_bps / 10_000.0;
    if effective_spread <= 0.0 {
        return None;
    }

    let half = effective_spread / 2.0;
    let mut bid_price = reservation - half;
    let mut ask_price = reservation + half;

    let fee_fraction = bounds.maker_fee_bps / 10_000.0;
    if fee_fraction > 0.0 {
        let fee_value = mid * fee_fraction;
        bid_price -= fee_value;
        ask_price += fee_value;
    }

    if ask_price <= bid_price {
        ask_price = reservation + effective_spread.abs();
        bid_price = reservation - effective_spread.abs();
        if ask_price <= bid_price {
            return None;
        }
    }

    let bid = QuoteOrder::new(bid_price, params.order_size, "avellaneda_bid");
    let ask = QuoteOrder::new(ask_price, params.order_size, "avellaneda_ask");
    let pair = QuotePair::new(bid, ask, reservation, effective_spread);

    let context = QuoteContext {
        reservation_price: reservation,
        raw_spread,
        raw_spread_bps,
        effective_spread,
        effective_spread_bps,
        min_required_spread_bps: min_required_bps,
        maker_fee_bps: bounds.maker_fee_bps,
    };

    Some(QuoteComputation { pair, context })
}

pub fn generate_quotes(
    mid: f64,
    q: f64,
    sigma: f64,
    params: &StrategyParams,
    bounds: &SafetyBounds,
) -> Option<QuotePair> {
    compute_quote(mid, q, sigma, params, bounds).map(|qc| qc.pair)
}
