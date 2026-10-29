//! Online false discovery rate control by e-LOND (Xu and Ramdas, arXiv:2311.06412).
//! Bounded gamma: Robertson, Wason and Ramdas, arXiv:2208.11418. Unbounded gamma:
//! Javanmard and Montanari 2018, eq. 31.
use crate::Error;

/// Published constant; the series sums to about 0.976.
const UNBOUNDED_C: f64 = 0.07720838;

/// Weight series that splits alpha over the slots of a stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gamma {
    /// 1/m for slots 1 to m, then 0.
    Bounded { m: u64 },
    /// C ln(max(t, 2)) / (t exp(sqrt(ln t))).
    Unbounded,
}

impl Gamma {
    /// Weight of slot `t`, counted from 1. Slot 0 has weight 0.
    pub fn at(&self, t: u64) -> f64 {
        if t == 0 {
            return 0.0;
        }
        match *self {
            Gamma::Bounded { m } => {
                if t <= m {
                    1.0 / m as f64
                } else {
                    0.0
                }
            }
            Gamma::Unbounded => {
                let t = t as f64;
                UNBOUNDED_C * t.max(2.0).ln() / (t * t.ln().sqrt().exp())
            }
        }
    }
}

/// One open test slot. Closing it consumes it.
#[derive(Debug)]
pub struct Slot {
    index: u64,
    level: f64,
}

impl Slot {
    pub fn index(&self) -> u64 {
        self.index
    }

    pub fn level(&self) -> f64 {
        self.level
    }
}

/// An online FDR stream. It is not `Clone`: a slot belongs to the stream that opened it.
///
/// FDR stays at or below alpha when each null slot's e-value has expected value at most
/// 1, counting any way that e-value depends on the slot's level. A fixed-size test meets
/// this under any dependence between trees. A stop that depends on the level, such as
/// stop at 1/level, meets it only when the tree's data are independent of the other
/// trees' results, because those results set the level.
#[derive(Debug)]
pub struct Stream {
    alpha: f64,
    gamma: Gamma,
    opened: u64,
    discoveries: u64,
}

impl Stream {
    pub fn new(alpha: f64, gamma: Gamma) -> Result<Stream, Error> {
        if !(alpha > 0.0 && alpha < 1.0) {
            return Err(Error::Parameter("alpha"));
        }
        if gamma == (Gamma::Bounded { m: 0 }) {
            return Err(Error::Parameter("m"));
        }
        Ok(Stream {
            alpha,
            gamma,
            opened: 0,
            discoveries: 0,
        })
    }

    /// The level counts only discoveries closed before the slot opened. Under the
    /// e-value condition on [`Stream`], FDR then stays at or below alpha in any
    /// closing order.
    pub fn open(&mut self) -> Slot {
        self.opened += 1;
        let level = self.alpha * self.gamma.at(self.opened) * (self.discoveries + 1) as f64;
        Slot {
            index: self.opened,
            level,
        }
    }

    /// `e` must meet the e-value condition on [`Stream`]. The slot is consumed even
    /// when `e` is invalid. `+inf` is allowed.
    pub fn close(&mut self, slot: Slot, e: f64) -> Result<bool, Error> {
        if e.is_nan() || e < 0.0 {
            return Err(Error::Parameter("e-value"));
        }
        let discovery = slot.level > 0.0 && e >= 1.0 / slot.level;
        self.discoveries += u64::from(discovery);
        Ok(discovery)
    }

    pub fn opened(&self) -> u64 {
        self.opened
    }

    pub fn discoveries(&self) -> u64 {
        self.discoveries
    }
}
