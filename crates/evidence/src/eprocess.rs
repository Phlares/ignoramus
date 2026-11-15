//! Betting e-process for paired outcomes (Waudby-Smith and Ramdas,
//! arXiv:2010.09686; Vovk and Wang, arXiv:1912.06116 for the two-sided mean).
use crate::{Error, Tail};

/// The largest bet fraction of [`EProcess::new`]. Every factor stays at or above 1/4, so wealth
/// never reaches 0.
pub const MAX_BET: f64 = 0.75;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Score {
    /// The nulls are stated for Upper. Lower reverses the inequality. Both needs equality
    /// (P(x > 0 | past) = P(x < 0 | past), symmetry, or E[x | past] = mean).
    ///
    /// s = sign(x). Null: P(x > 0 | past) <= P(x < 0 | past).
    Sign,
    /// s = clamp(x / scale, -1, 1). Null: x is symmetric about 0 given the past.
    Clip { scale: f64 },
    /// s = (x - mean) / max(hi - mean, mean - lo). Null: E[x | past] <= mean.
    Bounded { lo: f64, hi: f64, mean: f64 },
}

/// One wealth: the side it bets on (+1 or -1) and its largest bet fraction.
#[derive(Debug, Clone)]
struct Arm {
    sigma: f64,
    cap: f64,
    log_wealth: f64,
}

#[derive(Debug, Clone)]
pub struct EProcess {
    score: Score,
    stop: f64,
    arms: Vec<Arm>,
    s_sum: f64,
    q_sum: f64,
    peak_log: f64,
    stopped: bool,
    observed: usize,
}

impl EProcess {
    pub fn new(score: Score, tail: Tail, stop: f64) -> Result<EProcess, Error> {
        EProcess::with_caps(score, tail, stop, &[MAX_BET])
    }

    /// [`EProcess::new`] with the largest bet fractions `caps`: at least one, each strictly
    /// between 0 and 1. Any such cap fixed before the data keeps the test valid and changes
    /// its power. The value is the mean of the e-values the caps give, which is an e-value
    /// too.
    pub fn with_caps(score: Score, tail: Tail, stop: f64, caps: &[f64]) -> Result<EProcess, Error> {
        if caps.is_empty() || !caps.iter().all(|&cap| cap > 0.0 && cap < 1.0) {
            return Err(Error::Parameter("cap"));
        }
        if stop.is_nan() || stop <= 1.0 {
            return Err(Error::Parameter("stop"));
        }
        match score {
            Score::Sign => {}
            Score::Clip { scale } => {
                if !(scale.is_finite() && scale > 0.0) {
                    return Err(Error::Parameter("scale"));
                }
            }
            Score::Bounded { lo, hi, mean } => {
                if !(lo < mean && mean < hi && (hi - lo).is_finite()) {
                    return Err(Error::Parameter("bounds"));
                }
            }
        }
        let sides: &'static [f64] = match tail {
            Tail::Upper => &[1.0],
            Tail::Lower => &[-1.0],
            Tail::Both => &[1.0, -1.0],
        };
        let arm = |&sigma: &f64| {
            caps.iter().map(move |&cap| Arm {
                sigma,
                cap,
                log_wealth: 0.0,
            })
        };
        Ok(EProcess {
            score,
            stop,
            arms: sides.iter().flat_map(arm).collect(),
            s_sum: 0.0,
            q_sum: 0.0,
            peak_log: 0.0,
            stopped: false,
            observed: 0,
        })
    }

    pub fn observe(&mut self, x: f64) -> Result<(), Error> {
        if !x.is_finite() {
            return Err(Error::NotFinite);
        }
        let s = match self.score {
            Score::Sign => {
                if x > 0.0 {
                    1.0
                } else if x < 0.0 {
                    -1.0
                } else {
                    0.0
                }
            }
            Score::Clip { scale } => (x / scale).clamp(-1.0, 1.0),
            Score::Bounded { lo, hi, mean } => {
                if x < lo || x > hi {
                    return Err(Error::OutOfBounds);
                }
                (x - mean) / (hi - mean).max(mean - lo)
            }
        };
        if self.stopped {
            return Ok(());
        }
        for a in &mut self.arms {
            let bet = (a.sigma * self.s_sum / (1.0 + self.q_sum)).clamp(0.0, a.cap);
            a.log_wealth += (bet * (a.sigma * s)).ln_1p();
        }
        self.s_sum += s;
        self.q_sum += s * s;
        self.observed += 1;
        let now = self.log_value();
        self.peak_log = self.peak_log.max(now);
        self.stopped = self.stop.is_finite() && now.exp() >= self.stop;
        Ok(())
    }

    fn log_value(&self) -> f64 {
        let ws = || self.arms.iter().map(|a| a.log_wealth);
        let m = ws().fold(f64::NEG_INFINITY, f64::max);
        let sum: f64 = ws().map(|w| (w - m).exp()).sum();
        m + (sum / self.arms.len() as f64).ln()
    }

    pub fn value(&self) -> f64 {
        self.log_value().exp()
    }

    pub fn stopped(&self) -> bool {
        self.stopped
    }

    pub fn observed(&self) -> usize {
        self.observed
    }

    /// The largest value the process held, the start's 1 included.
    pub fn peak(&self) -> f64 {
        self.peak_log.exp()
    }

    /// `min(1, 1 / peak value)`. Valid at any stopping time (Ville's inequality).
    pub fn p_value(&self) -> f64 {
        (-self.peak_log).exp().min(1.0)
    }
}
