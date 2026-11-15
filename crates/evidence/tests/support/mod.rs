//! Shared by the long simulation suites.
use std::thread;

/// The seed for `text`: four splitmix64 outputs from its FNV-1a hash, so each scenario and
/// batch has its own.
pub fn seed(text: &str) -> [u8; 32] {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut out = [0u8; 32];
    for chunk in out.chunks_mut(8) {
        h = h.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = h;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        chunk.copy_from_slice(&(z ^ (z >> 31)).to_le_bytes());
    }
    out
}

/// `f` of each item, each on its own thread, in the order of `items`.
pub fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    thread::scope(|scope| {
        let handles: Vec<_> = items.iter().map(|x| scope.spawn(|| f(x))).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    })
}
