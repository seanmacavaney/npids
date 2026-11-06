//! Bit packing (FORMAT.md, "Bit packing") and little-endian byte helpers.

/// Number of bits needed to represent `x` (`bitlen(0) == 0`).
#[inline]
pub fn bitlen(x: u64) -> u8 {
    (64 - x.leading_zeros()) as u8
}

/// Byte length of a packed array of `n` values of width `bits`.
#[inline]
pub fn packed_len(n: usize, bits: u8) -> usize {
    8 * ((n * bits as usize).div_ceil(64) + 1)
}

/// Pack `values` at width `bits`. Every value must fit in `bits` bits.
pub fn pack(values: &[u64], bits: u8) -> Vec<u8> {
    let mut words = vec![0u64; packed_len(values.len(), bits) / 8];
    if bits > 0 {
        let bits = bits as usize;
        for (j, &v) in values.iter().enumerate() {
            debug_assert!(bits == 64 || v >> bits == 0);
            let pos = j * bits;
            let (w, off) = (pos / 64, pos % 64);
            words[w] |= v << off;
            if off + bits > 64 {
                words[w + 1] |= v >> (64 - off);
            }
        }
    }
    let mut out = Vec::with_capacity(words.len() * 8);
    for w in words {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out
}

/// Read value `j` from a packed array.
#[inline]
pub fn get(data: &[u8], bits: u8, j: usize) -> u64 {
    if bits == 0 {
        return 0;
    }
    let bits = bits as usize;
    let pos = j * bits;
    let (w, off) = (pos / 64, pos % 64);
    let mut v = read_u64(data, w * 8) >> off;
    if off + bits > 64 {
        v |= read_u64(data, (w + 1) * 8) << (64 - off);
    }
    if bits == 64 { v } else { v & ((1u64 << bits) - 1) }
}

#[inline]
pub fn read_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

#[inline]
pub fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

#[inline]
pub fn read_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(b[off..off + 2].try_into().unwrap())
}

/// Little-endian byte writer.
#[derive(Default)]
pub struct W(pub Vec<u8>);

impl W {
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
    pub fn zeros(&mut self, n: usize) -> &mut Self {
        self.0.resize(self.0.len() + n, 0);
        self
    }
    pub fn pad8(&mut self) -> &mut Self {
        let n = self.0.len().next_multiple_of(8);
        self.0.resize(n, 0);
        self
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_widths() {
        let mut state = 0x1234_5678_9abc_def0u64;
        for bits in 0..=64u8 {
            let vals: Vec<u64> = (0..200)
                .map(|_| {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    if bits == 64 { state } else if bits == 0 { 0 } else { state & ((1u64 << bits) - 1) }
                })
                .collect();
            let packed = pack(&vals, bits);
            assert_eq!(packed.len(), packed_len(vals.len(), bits));
            for (j, &v) in vals.iter().enumerate() {
                assert_eq!(get(&packed, bits, j), v, "bits={bits} j={j}");
            }
        }
    }

    #[test]
    fn bitlen_values() {
        assert_eq!(bitlen(0), 0);
        assert_eq!(bitlen(1), 1);
        assert_eq!(bitlen(255), 8);
        assert_eq!(bitlen(256), 9);
        assert_eq!(bitlen(u64::MAX), 64);
    }
}
