//! Perfect hash inverse units (FORMAT.md, "Kind 2: PHF"). PTHash-style hash-and-displace.

use crate::bits::{self, W, read_u64};
use crate::{Error, Result};

const ALPHA: f64 = 0.98;
const LAMBDA: f64 = 4.5;
const MAX_PILOT: u64 = 1 << 24;
const PAYLOAD_HEADER: usize = 56;
pub const DEFAULT_SEED: u64 = 0x9e37_79b9_7f4a_7c15;

#[inline]
pub fn fmix64(mut k: u64) -> u64 {
    k ^= k >> 33;
    k = k.wrapping_mul(0xff51_afd7_ed55_8ccd);
    k ^= k >> 33;
    k = k.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    k ^= k >> 33;
    k
}

/// `hash64` from FORMAT.md: one code path for every length.
#[inline]
pub fn hash(id: &[u8], seed: u64) -> u64 {
    let mut h = seed ^ (id.len() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut blocks = id.chunks_exact(8);
    for b in &mut blocks {
        h = (h ^ u64::from_le_bytes(b.try_into().unwrap())).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        h ^= h >> 31;
    }
    let rest = blocks.remainder();
    if !rest.is_empty() {
        let mut last = [0u8; 8];
        last[..rest.len()].copy_from_slice(rest);
        h = (h ^ u64::from_le_bytes(last)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        h ^= h >> 31;
    }
    fmix64(h)
}

#[inline]
fn bucket(h: u64, b: u64, b1: u64) -> u64 {
    let hi = h >> 32;
    if b1 > 0 && (h & 0xFFFF_FFFF) < 0x9999_9999 { (hi * b1) >> 32 } else { b1 + ((hi * (b - b1)) >> 32) }
}

#[inline]
fn pos(h: u64, pilot: u64, m: u64) -> u64 {
    ((fmix64(h ^ fmix64(pilot)) as u128 * m as u128) >> 64) as u64
}

/// Build a PHF payload for `keys` = (hash, index - lo), whose hashes must be distinct.
/// `range` is `hi - lo`. Returns None if the pilot search fails (the caller retries with another seed).
pub fn build(keys: &[(u64, u64)], range: u64, seed: u64) -> Option<Vec<u8>> {
    let n = keys.len() as u64;
    let m = n.max((n as f64 / ALPHA).ceil() as u64).max(1);
    let b = ((n as f64 / LAMBDA).ceil() as u64).clamp(1, u32::MAX as u64 - 1);
    let b1 = b * 3 / 10;

    // Group keys by bucket (counting sort).
    let bk: Vec<u64> = keys.iter().map(|&(h, _)| bucket(h, b, b1)).collect();
    let mut start = vec![0usize; b as usize + 1];
    for &x in &bk {
        start[x as usize + 1] += 1;
    }
    for i in 0..b as usize {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut grouped = vec![(0u64, 0u64); keys.len()];
    for (k, &x) in keys.iter().zip(&bk) {
        grouped[fill[x as usize]] = *k;
        fill[x as usize] += 1;
    }
    // Largest buckets first.
    let mut order: Vec<u32> = (0..b as u32).filter(|&i| start[i as usize + 1] > start[i as usize]).collect();
    order.sort_unstable_by_key(|&i| std::cmp::Reverse(start[i as usize + 1] - start[i as usize]));

    let slot_bits = bits::bitlen(range);
    let empty = if slot_bits == 64 { u64::MAX } else { (1u64 << slot_bits) - 1 };
    let mut taken = vec![0u64; (m as usize).div_ceil(64)];
    let mut slots = vec![empty; m as usize];
    let mut pilots = vec![0u64; b as usize];
    let mut ps: Vec<u64> = Vec::new();
    for &bi in &order {
        let ks = &grouped[start[bi as usize]..start[bi as usize + 1]];
        let mut found = false;
        'pilot: for p in 0..MAX_PILOT {
            ps.clear();
            for &(h, _) in ks {
                let x = pos(h, p, m);
                if taken[(x / 64) as usize] >> (x % 64) & 1 == 1 || ps.contains(&x) {
                    continue 'pilot;
                }
                ps.push(x);
            }
            for (&x, &(_, local)) in ps.iter().zip(ks) {
                taken[(x / 64) as usize] |= 1 << (x % 64);
                slots[x as usize] = local;
            }
            pilots[bi as usize] = p;
            found = true;
            break;
        }
        if !found {
            return None;
        }
    }

    let pilot_bits = bits::bitlen(pilots.iter().copied().max().unwrap_or(0));
    let packed_pilots = bits::pack(&pilots, pilot_bits);
    let packed_slots = bits::pack(&slots, slot_bits);
    let mut w = W::default();
    w.u64(n).u64(m).u64(b).u64(seed).u8(pilot_bits).u8(slot_bits).zeros(6);
    w.u64(PAYLOAD_HEADER as u64).u64((PAYLOAD_HEADER + packed_pilots.len()) as u64);
    debug_assert_eq!(w.len(), PAYLOAD_HEADER);
    w.bytes(&packed_pilots).bytes(&packed_slots);
    Some(w.0)
}

/// A PHF unit in a mapped file.
#[derive(Clone, Debug)]
pub struct PhfView {
    m: u64,
    b: u64,
    b1: u64,
    seed: u64,
    pilot_bits: u8,
    slot_bits: u8,
    empty: u64,
    pilots: (usize, usize),
    slots: (usize, usize),
}

impl PhfView {
    pub fn read(file: &[u8], offset: u64, len: u64) -> Result<PhfView> {
        let corrupt = || Error::Corrupt(format!("PHF unit at {offset}"));
        let start = offset as usize;
        let p = file.get(start..start + len as usize).ok_or_else(corrupt)?;
        if p.len() < PAYLOAD_HEADER {
            return Err(corrupt());
        }
        let (m, b, seed) = (read_u64(p, 8), read_u64(p, 16), read_u64(p, 24));
        let (pilot_bits, slot_bits) = (p[32], p[33]);
        let (pilots_off, slots_off) = (read_u64(p, 40) as usize, read_u64(p, 48) as usize);
        let pilots_len = bits::packed_len(b as usize, pilot_bits as usize);
        let slots_len = bits::packed_len(m as usize, slot_bits as usize);
        if b == 0 || m == 0 || pilot_bits > 64 || slot_bits > 64
            || pilots_off + pilots_len > p.len() || slots_off + slots_len > p.len()
        {
            return Err(corrupt());
        }
        Ok(PhfView {
            m,
            b,
            b1: b * 3 / 10,
            seed,
            pilot_bits,
            slot_bits,
            empty: if slot_bits == 64 { u64::MAX } else { (1u64 << slot_bits) - 1 },
            pilots: (start + pilots_off, pilots_len),
            slots: (start + slots_off, slots_len),
        })
    }

    /// The candidate local index for `id`, or None if `id` is certainly not in the unit.
    #[inline]
    pub fn candidate(&self, file: &[u8], id: &[u8]) -> Option<u64> {
        let h = hash(id, self.seed);
        let bi = bucket(h, self.b, self.b1);
        let pilot = bits::get(&file[self.pilots.0..self.pilots.0 + self.pilots.1], self.pilot_bits, bi as usize);
        let x = pos(h, pilot, self.m);
        let v = bits::get(&file[self.slots.0..self.slots.0 + self.slots.1], self.slot_bits, x as usize);
        if v == self.empty { None } else { Some(v) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perfect_on_many_sizes() {
        for n in [1u64, 2, 3, 10, 100, 1000, 54321] {
            let keys: Vec<(u64, u64)> = (0..n).map(|i| (hash(format!("id{i}").as_bytes(), DEFAULT_SEED), i)).collect();
            let payload = build(&keys, n, DEFAULT_SEED).unwrap();
            let view = PhfView::read(&payload, 0, payload.len() as u64).unwrap();
            for i in 0..n {
                assert_eq!(view.candidate(&payload, format!("id{i}").as_bytes()), Some(i));
            }
            let bits_per_key = (payload.len() * 8) as f64 / n as f64;
            if n >= 1000 {
                assert!(bits_per_key < bits::bitlen(n) as f64 + 6.0, "n={n} {bits_per_key}");
            }
        }
    }
}
