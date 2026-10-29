//! Budget meter: cumulative cost, alert fractions, and the fail-safe pause decision.
//! A dimension is `'c'` cpu, `'w'` wall or `'m'` money. A cap of 0 means no cap.
pub use crate::spec::model::Budget;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, VecDeque};
use std::fmt;
use std::time::Instant;

/// Fractions of each cap that raise an alert, once each.
pub const ALERT_FRACTIONS: [f64; 3] = [0.50, 0.80, 0.95];

/// A row is not started when `(spent + next row) * SAFETY_MARGIN` reaches a cap.
pub const SAFETY_MARGIN: f64 = 1.0;

/// Cumulative cost of a study.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Cost {
    pub cpu_seconds: f64,
    pub wall_seconds: f64,
    pub usd: f64,
}

impl Cost {
    /// Each dimension's larger value.
    pub fn larger(&self, other: &Cost) -> Cost {
        Cost {
            cpu_seconds: self.cpu_seconds.max(other.cpu_seconds),
            wall_seconds: self.wall_seconds.max(other.wall_seconds),
            usd: self.usd.max(other.usd),
        }
    }

    fn plus(&self, other: &Cost) -> Cost {
        Cost {
            cpu_seconds: self.cpu_seconds + other.cpu_seconds,
            wall_seconds: self.wall_seconds + other.wall_seconds,
            usd: self.usd + other.usd,
        }
    }

    fn is_sound(&self) -> bool {
        [self.cpu_seconds, self.wall_seconds, self.usd]
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0)
    }
}

/// `(dimension, spent, cap)` in the cap's own unit: hours for cpu and wall, dollars for money.
fn dims(c: &Cost, b: &Budget) -> [(char, f64, f64); 3] {
    [
        ('c', c.cpu_seconds / 3600.0, b.cpu_hours),
        ('w', c.wall_seconds / 3600.0, b.wall_hours),
        ('m', c.usd, b.money_usd),
    ]
}

pub fn dimension_name(dim: char) -> &'static str {
    match dim {
        'c' => "cpu",
        'w' => "wall",
        _ => "money",
    }
}

/// Why a cost reading cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeterError(pub String);

impl fmt::Display for MeterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MeterError {}

pub trait CostMeter {
    /// Cumulative cost now. An error pauses the run.
    fn sample(&self) -> Result<Cost, MeterError>;
    /// A row finished this session and used `cpu_seconds` of cpu.
    fn add_row(&self, _cpu_seconds: f64) {}
}

/// Earlier spend, plus wall time on a monotonic clock since `start`, plus this session's rows.
pub struct HostMeter {
    pub start: Instant,
    pub prior: Cost,
    pub rate_usd_per_hour: f64,
    rows_cpu: Cell<f64>,
}

impl HostMeter {
    /// `prior` is the spend already in the signed row records.
    pub fn new(prior: Cost, rate_usd_per_hour: f64) -> HostMeter {
        HostMeter {
            start: Instant::now(),
            prior,
            rate_usd_per_hour,
            rows_cpu: Cell::new(0.0),
        }
    }
}

impl CostMeter for HostMeter {
    fn sample(&self) -> Result<Cost, MeterError> {
        let wall_seconds = self.prior.wall_seconds + self.start.elapsed().as_secs_f64();
        Ok(Cost {
            cpu_seconds: self.prior.cpu_seconds + self.rows_cpu.get(),
            wall_seconds,
            usd: wall_seconds / 3600.0 * self.rate_usd_per_hour,
        })
    }

    fn add_row(&self, cpu_seconds: f64) {
        self.rows_cpu.set(self.rows_cpu.get() + cpu_seconds);
    }
}

/// Scripted readings for tests. Each sample takes the next one; the last repeats.
pub struct FakeMeter {
    script: RefCell<VecDeque<Result<Cost, MeterError>>>,
}

impl FakeMeter {
    pub fn new(readings: Vec<Result<Cost, MeterError>>) -> FakeMeter {
        FakeMeter {
            script: RefCell::new(readings.into()),
        }
    }
}

impl CostMeter for FakeMeter {
    fn sample(&self) -> Result<Cost, MeterError> {
        let mut s = self.script.borrow_mut();
        let next = if s.len() > 1 {
            s.pop_front()
        } else {
            s.front().cloned()
        };
        next.unwrap_or_else(|| Err(MeterError("no reading planted".into())))
    }
}

/// Each `(dimension, fraction)` that `now` has reached and `prev` had not.
pub fn crossed(prev: &Cost, now: &Cost, b: &Budget) -> Vec<(char, f64)> {
    let mut out = Vec::new();
    for ((dim, was, cap), (_, is, _)) in dims(prev, b).into_iter().zip(dims(now, b)) {
        if cap <= 0.0 {
            continue;
        }
        for f in ALERT_FRACTIONS {
            if was / cap < f && is / cap >= f {
                out.push((dim, f));
            }
        }
    }
    out
}

/// Remembers which alerts fired, so each fires once.
pub struct Alerts {
    fired: BTreeSet<(char, usize)>,
}

impl Alerts {
    /// Fractions that `spent` already crossed count as fired, without being reported.
    pub fn seeded(spent: &Cost, b: &Budget) -> Alerts {
        let mut a = Alerts {
            fired: BTreeSet::new(),
        };
        a.fresh(spent, b);
        a
    }

    /// The fractions `now` has reached that did not fire before.
    pub fn fresh(&mut self, now: &Cost, b: &Budget) -> Vec<(char, f64)> {
        crossed(&Cost::default(), now, b)
            .into_iter()
            .filter(|&(dim, f)| {
                let i = ALERT_FRACTIONS.iter().position(|&x| x == f).unwrap_or(0);
                self.fired.insert((dim, i))
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PauseReason {
    CapReached(char),
    WouldCross(char),
    MeterFault(String),
    InternalCheck(String),
}

impl fmt::Display for PauseReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PauseReason::CapReached(d) => write!(f, "the {} cap is reached", dimension_name(*d)),
            PauseReason::WouldCross(d) => {
                write!(f, "the next row would reach the {} cap", dimension_name(*d))
            }
            PauseReason::MeterFault(e) => write!(f, "the cost meter failed: {e}"),
            PauseReason::InternalCheck(e) => write!(f, "an internal check failed: {e}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Continue,
    Pause(PauseReason),
}

/// Pause on a meter fault, an unusable number, a reached cap, or a next row that would reach one.
pub fn decide(now: &Cost, next_row: &Cost, b: &Budget, meter: Result<(), MeterError>) -> Decision {
    if let Err(e) = meter {
        return Decision::Pause(PauseReason::MeterFault(e.0));
    }
    let caps_ok = [b.cpu_hours, b.wall_hours, b.money_usd]
        .iter()
        .all(|c| c.is_finite() && *c >= 0.0);
    if !now.is_sound() || !next_row.is_sound() || !caps_ok {
        return Decision::Pause(PauseReason::InternalCheck(
            "a cost or a cap is not a finite number of 0 or more".into(),
        ));
    }
    let after = now.plus(next_row);
    let capped = |c: &Cost| {
        dims(c, b)
            .into_iter()
            .filter(|d| d.2 > 0.0)
            .collect::<Vec<_>>()
    };
    if let Some((dim, ..)) = capped(now)
        .into_iter()
        .find(|&(_, spent, cap)| spent >= cap)
    {
        return Decision::Pause(PauseReason::CapReached(dim));
    }
    match capped(&after)
        .into_iter()
        .find(|&(_, spent, cap)| spent * SAFETY_MARGIN >= cap)
    {
        Some((dim, ..)) => Decision::Pause(PauseReason::WouldCross(dim)),
        None => Decision::Continue,
    }
}
