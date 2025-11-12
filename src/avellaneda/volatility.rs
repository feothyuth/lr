use ringbuffer::{AllocRingBuffer, RingBuffer, RingBufferExt, RingBufferRead, RingBufferWrite};
use std::time::Instant;

const SIGMA_FLOOR: f64 = 1e-4;

#[derive(Clone, Debug)]
pub struct VolEstimator {
    lookback: usize,
    alpha: f64,
    returns: AllocRingBuffer<f64>,
    sigma: f64,
    last_mid: Option<f64>,
    last_update: Option<Instant>,
}

impl VolEstimator {
    pub fn new(lookback: usize, alpha: f64) -> Self {
        let capacity = lookback.next_power_of_two().max(2);
        Self {
            lookback,
            alpha,
            returns: AllocRingBuffer::with_capacity(capacity),
            sigma: SIGMA_FLOOR,
            last_mid: None,
            last_update: None,
        }
    }

    pub fn on_mid_price(&mut self, mid: f64, timestamp: Instant) {
        if let Some(prev) = self.last_mid {
            if prev > 0.0 && mid > 0.0 {
                let log_return = (mid / prev).ln();
                self.returns.push(log_return);
                // AllocRingBuffer grows dynamically, manually trim if needed
                if self.returns.len() > self.lookback {
                    let excess = self.returns.len() - self.lookback;
                    for _ in 0..excess {
                        let _ = self.returns.dequeue();
                    }
                }
                if self.returns.len() > 1 {
                    let mean = self.returns.iter().sum::<f64>() / (self.returns.len() as f64);
                    let variance = self.returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>()
                        / (self.returns.len() as f64);
                    let sample_sigma = variance.max(0.0).sqrt();
                    self.sigma = self.alpha * sample_sigma
                        + (1.0 - self.alpha) * self.sigma.max(SIGMA_FLOOR);
                }
            }
        }
        self.last_mid = Some(mid);
        self.last_update = Some(timestamp);
    }

    pub fn sigma(&self) -> f64 {
        self.sigma
    }

    pub fn sigma_per_second(&self, samples_per_second: f64) -> f64 {
        if samples_per_second <= 0.0 {
            return self.sigma;
        }
        self.sigma * samples_per_second.sqrt()
    }

    pub fn sigma_annualized(&self, samples_per_second: f64) -> f64 {
        if samples_per_second <= 0.0 {
            return self.sigma;
        }
        let annualization_factor = samples_per_second * 60.0 * 60.0 * 24.0 * 365.0;
        self.sigma * annualization_factor.sqrt()
    }

    pub fn last_update(&self) -> Option<Instant> {
        self.last_update
    }

    pub fn reset(&mut self) {
        self.returns.clear();
        self.sigma = SIGMA_FLOOR;
        self.last_mid = None;
        self.last_update = None;
    }

    /// Check if volatility estimator has enough samples for reliable estimates
    /// Returns true when buffer is at least 50% full
    pub fn is_warmed_up(&self) -> bool {
        self.returns.len() >= (self.lookback / 2)
    }
}
