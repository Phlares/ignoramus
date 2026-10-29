//! RFC 9162 section 2.1 Merkle tree: root, inclusion and consistency proofs.
use sha2::{Digest, Sha256};

pub type Hash = [u8; 32];

fn sha(parts: &[&[u8]]) -> Hash {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// SHA-256(0x00 || data).
pub fn leaf_hash(data: &[u8]) -> Hash {
    sha(&[&[0x00], data])
}

/// SHA-256(0x01 || left || right).
pub fn node_hash(left: &Hash, right: &Hash) -> Hash {
    sha(&[&[0x01], left, right])
}

/// Largest power of two strictly below `n` (`n` >= 2).
fn split(n: usize) -> usize {
    1 << (usize::BITS - 1 - (n - 1).leading_zeros())
}

/// MTH: the tree hash of `leaves`. The empty tree hashes the empty string.
pub fn root(leaves: &[Vec<u8>]) -> Hash {
    match leaves {
        [] => sha(&[]),
        [one] => leaf_hash(one),
        _ => {
            let (l, r) = leaves.split_at(split(leaves.len()));
            node_hash(&root(l), &root(r))
        }
    }
}

/// PATH(index, leaves). Empty when `index` is out of range.
pub fn inclusion_proof(leaves: &[Vec<u8>], index: usize) -> Vec<Hash> {
    if index >= leaves.len() {
        return Vec::new();
    }
    path(leaves, index)
}

fn path(leaves: &[Vec<u8>], m: usize) -> Vec<Hash> {
    if leaves.len() == 1 {
        return Vec::new();
    }
    let (l, r) = leaves.split_at(split(leaves.len()));
    let mut p;
    if m < l.len() {
        p = path(l, m);
        p.push(root(r));
    } else {
        p = path(r, m - l.len());
        p.push(root(l));
    }
    p
}

/// PROOF(old_size, leaves). Empty when `old_size` is 0 or above the size.
pub fn consistency_proof(leaves: &[Vec<u8>], old_size: usize) -> Vec<Hash> {
    if old_size == 0 || old_size > leaves.len() {
        return Vec::new();
    }
    subproof(leaves, old_size, true)
}

fn subproof(leaves: &[Vec<u8>], m: usize, whole: bool) -> Vec<Hash> {
    let n = leaves.len();
    if m == n {
        return if whole {
            Vec::new()
        } else {
            vec![root(leaves)]
        };
    }
    let (l, r) = leaves.split_at(split(n));
    let mut p;
    if m <= l.len() {
        p = subproof(l, m, whole);
        p.push(root(r));
    } else {
        p = subproof(r, m - l.len(), false);
        p.push(root(l));
    }
    p
}

/// RFC 9162 inclusion check (the section 2.1.3 algorithm). False on any malformed input.
pub fn verify_inclusion(leaf: &[u8], index: u64, size: u64, proof: &[Hash], root: &Hash) -> bool {
    if index >= size {
        return false;
    }
    let (mut f, mut s) = (index, size - 1);
    let mut r = leaf_hash(leaf);
    for p in proof {
        if s == 0 {
            return false;
        }
        if f & 1 == 1 || f == s {
            r = node_hash(p, &r);
            while f & 1 == 0 && f != 0 {
                f >>= 1;
                s >>= 1;
            }
        } else {
            r = node_hash(&r, p);
        }
        f >>= 1;
        s >>= 1;
    }
    s == 0 && r == *root
}

/// RFC 9162 consistency check (the section 2.1.4 algorithm). False on any malformed input.
pub fn verify_consistency(
    old_size: u64,
    new_size: u64,
    old_root: &Hash,
    new_root: &Hash,
    proof: &[Hash],
) -> bool {
    if old_size == 0 || old_size > new_size {
        return false;
    }
    if old_size == new_size {
        return proof.is_empty() && old_root == new_root;
    }
    let mut nodes: Vec<Hash> = Vec::with_capacity(proof.len() + 1);
    if old_size.is_power_of_two() {
        nodes.push(*old_root);
    }
    nodes.extend_from_slice(proof);
    let Some((first, rest)) = nodes.split_first() else {
        return false;
    };
    let (mut f, mut s) = (old_size - 1, new_size - 1);
    while f & 1 == 1 {
        f >>= 1;
        s >>= 1;
    }
    let (mut fr, mut sr) = (*first, *first);
    for c in rest {
        if s == 0 {
            return false;
        }
        if f & 1 == 1 || f == s {
            fr = node_hash(c, &fr);
            sr = node_hash(c, &sr);
            while f & 1 == 0 && f != 0 {
                f >>= 1;
                s >>= 1;
            }
        } else {
            sr = node_hash(&sr, c);
        }
        f >>= 1;
        s >>= 1;
    }
    s == 0 && fr == *old_root && sr == *new_root
}
