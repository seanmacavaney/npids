//! Chunk payloads (FORMAT.md, "Chunk payload").

use crate::bits::{self, W, read_u32, read_u64};
use crate::format::ChunkEntry;
use crate::template::{Part, Template, Val, write_dec, write_hex};
use crate::{Error, Result};

pub const ENC_CONST: u8 = 0;
pub const ENC_SEQ: u8 = 1;
pub const ENC_FOR: u8 = 2;
pub const ENC_PLAIN: u8 = 3;

const CHUNK_HEADER: usize = 8;
const FIELD_DESC: usize = 32;

/// Field values of a chunk, one column per template field.
pub enum Column<'a> {
    Int(Vec<u64>),
    Str(Vec<&'a [u8]>),
}

struct IntEnc {
    enc: u8,
    bits: u8,
    p0: u64,
    p1: u64,
}

fn choose_int(vals: &[u64]) -> IntEnc {
    let v0 = vals[0];
    if vals.iter().all(|&v| v == v0) {
        return IntEnc { enc: ENC_CONST, bits: 0, p0: v0, p1: 0 };
    }
    let step = vals[1].wrapping_sub(v0);
    if vals.iter().enumerate().all(|(j, &v)| v == v0.wrapping_add((j as u64).wrapping_mul(step))) {
        return IntEnc { enc: ENC_SEQ, bits: 0, p0: v0, p1: step };
    }
    let (min, max) = vals.iter().fold((u64::MAX, 0), |(a, b), &v| (a.min(v), b.max(v)));
    IntEnc { enc: ENC_FOR, bits: bits::bitlen(max - min), p0: min, p1: 0 }
}

/// Encoded size in bytes of a chunk with these columns (used to choose a template).
pub fn cost(columns: &[Column], count: usize) -> usize {
    let mut c = CHUNK_HEADER + FIELD_DESC * columns.len();
    for col in columns {
        c += match col {
            Column::Int(v) => {
                let e = choose_int(v);
                if e.enc == ENC_FOR { bits::packed_len(count, e.bits as usize) } else { 0 }
            }
            Column::Str(v) => (4 * (count + 1) + v.iter().map(|s| s.len()).sum::<usize>()).next_multiple_of(8),
        };
    }
    c
}

pub fn encode(columns: &[Column], count: usize) -> Vec<u8> {
    assert!(count > 0);
    let encs: Vec<Option<IntEnc>> = columns
        .iter()
        .map(|c| match c {
            Column::Int(v) => {
                debug_assert_eq!(v.len(), count);
                Some(choose_int(v))
            }
            Column::Str(_) => None,
        })
        .collect();
    let rel = |k: usize| -> Vec<u64> {
        let (Column::Int(v), Some(e)) = (&columns[k], &encs[k]) else { unreachable!() };
        v.iter().map(|&x| x - e.p0).collect()
    };
    let for_fields: Vec<usize> = (0..columns.len()).filter(|&k| encs[k].as_ref().is_some_and(|e| e.enc == ENC_FOR)).collect();

    let mut head = W::default();
    head.u32(count as u32).u16(columns.len() as u16).u16(0);
    let mut data = W::default();
    let data_start = CHUNK_HEADER + FIELD_DESC * columns.len();
    // With two or more FOR fields, store them row by row in one stream (FORMAT.md, "Chunk payload").
    let mut shared: Option<(usize, usize, usize, Vec<usize>)> = None; // (off, len, stride, bit_offs)
    if for_fields.len() >= 2 {
        let rels: Vec<Vec<u64>> = for_fields.iter().map(|&k| rel(k)).collect();
        let cols: Vec<(&[u64], u8)> = for_fields.iter().zip(&rels).map(|(&k, r)| (r.as_slice(), encs[k].as_ref().unwrap().bits)).collect();
        let (stream, stride) = bits::pack_rows(&cols, count);
        let mut offs = Vec::new();
        let mut o = 0;
        for c in &cols {
            offs.push(o);
            o += c.1 as usize;
        }
        shared = Some((data_start, stream.len(), stride, offs));
        data.bytes(&stream);
    }
    for (k, col) in columns.iter().enumerate() {
        let off = data_start + data.len();
        let (enc, b, p0, p1, stride, bit_off, d_off, d_len) = match (col, &encs[k]) {
            (Column::Int(_), Some(e)) if e.enc == ENC_FOR => match &shared {
                Some((s_off, s_len, stride, offs)) => {
                    let i = for_fields.iter().position(|&f| f == k).unwrap();
                    (e.enc, e.bits, e.p0, e.p1, *stride as u16, offs[i] as u16, *s_off, *s_len)
                }
                None => {
                    data.bytes(&bits::pack(&rel(k), e.bits));
                    (e.enc, e.bits, e.p0, e.p1, 0, 0, off, data_start + data.len() - off)
                }
            },
            (Column::Int(_), Some(e)) => (e.enc, e.bits, e.p0, e.p1, 0, 0, off, 0),
            (Column::Str(v), _) => {
                debug_assert_eq!(v.len(), count);
                let mut o = 0u32;
                data.u32(0);
                for s in v {
                    o += s.len() as u32;
                    data.u32(o);
                }
                for s in v {
                    data.bytes(s);
                }
                data.pad8();
                (ENC_PLAIN, 0, 0, 0, 0, 0, off, data_start + data.len() - off)
            }
            _ => unreachable!(),
        };
        head.u8(enc).u8(b).u16(stride).u16(bit_off).u16(0).u64(p0).u64(p1).u32(d_off as u32).u32(d_len as u32);
    }
    head.bytes(&data.0);
    head.0
}

#[derive(Clone, Debug)]
pub struct FieldCol {
    pub enc: u8,
    pub bits: u8,
    /// Row stride in bits and this field's offset in a row (FOR only).
    pub stride: usize,
    pub bit_off: usize,
    pub p0: u64,
    pub p1: u64,
    /// Absolute offset of the column data in the file.
    pub data: usize,
    pub data_len: usize,
}

/// A parsed chunk header, with column offsets resolved to absolute file offsets.
#[derive(Clone, Debug)]
pub struct ChunkInfo {
    pub count: usize,
    pub template_id: u32,
    pub fields: Vec<FieldCol>,
    /// Whether any field reads column data (FOR or PLAIN), and whether any is a string.
    pub has_data: bool,
    pub has_strings: bool,
}

impl ChunkInfo {
    pub fn read(file: &[u8], e: &ChunkEntry, tmpl: &Template) -> Result<ChunkInfo> {
        let corrupt = |m: &str| Error::Corrupt(format!("chunk at {}: {m}", e.offset));
        let start = e.offset as usize;
        let end = start.checked_add(e.len as usize).ok_or_else(|| corrupt("bad length"))?;
        let c = file.get(start..end).ok_or_else(|| corrupt("out of bounds"))?;
        if c.len() < CHUNK_HEADER {
            return Err(corrupt("truncated"));
        }
        let count = read_u32(c, 0) as usize;
        let n_fields = bits::read_u16(c, 4) as usize;
        if count != e.count as usize {
            return Err(corrupt("count mismatch"));
        }
        if n_fields != tmpl.n_fields() {
            return Err(corrupt("field count does not match template"));
        }
        if c.len() < CHUNK_HEADER + FIELD_DESC * n_fields {
            return Err(corrupt("truncated"));
        }
        let mut fields = Vec::with_capacity(n_fields);
        for (k, part) in tmpl.fields().enumerate() {
            let d = &c[CHUNK_HEADER + FIELD_DESC * k..];
            let stride = bits::read_u16(d, 2) as usize;
            let f = FieldCol {
                enc: d[0],
                bits: d[1],
                stride: if stride == 0 { d[1] as usize } else { stride },
                bit_off: bits::read_u16(d, 4) as usize,
                p0: read_u64(d, 8),
                p1: read_u64(d, 16),
                data: start + read_u32(d, 24) as usize,
                data_len: read_u32(d, 28) as usize,
            };
            if f.data + f.data_len > end {
                return Err(corrupt("column out of bounds"));
            }
            let ok = match (f.enc, part) {
                (ENC_CONST | ENC_SEQ, p) => p.is_int(),
                (ENC_FOR, p) => {
                    p.is_int()
                        && f.bits <= 64
                        && f.bit_off + f.bits as usize <= f.stride
                        && f.data_len >= bits::packed_len(count, f.stride)
                }
                (ENC_PLAIN, Part::Str) => {
                    f.data_len >= 4 * (count + 1)
                        && 4 * (count + 1) + read_u32(file, f.data + 4 * count) as usize <= f.data_len
                }
                (ENC_CONST..=ENC_PLAIN, _) => false,
                (enc, _) => return Err(Error::Unsupported(format!("unknown column encoding {enc}"))),
            };
            if !ok {
                return Err(corrupt("invalid column"));
            }
            fields.push(f);
        }
        let has_data = fields.iter().any(|f| f.enc == ENC_FOR || f.enc == ENC_PLAIN);
        let has_strings = fields.iter().any(|f| f.enc == ENC_PLAIN);
        Ok(ChunkInfo { count, template_id: e.template_id, fields, has_data, has_strings })
    }

    #[inline]
    pub fn int(&self, file: &[u8], k: usize, j: usize) -> u64 {
        let f = &self.fields[k];
        match f.enc {
            ENC_CONST => f.p0,
            ENC_SEQ => f.p0.wrapping_add((j as u64).wrapping_mul(f.p1)),
            _ => f.p0.wrapping_add(bits::read_bits(file, f.data, j * f.stride + f.bit_off, f.bits)),
        }
    }

    #[inline]
    pub fn str<'a>(&self, file: &'a [u8], k: usize, j: usize) -> &'a [u8] {
        let f = &self.fields[k];
        let a = read_u32(file, f.data + 4 * j) as usize;
        let b = read_u32(file, f.data + 4 * (j + 1)) as usize;
        let blob = f.data + 4 * (self.count + 1);
        &file[blob + a..blob + b]
    }

    /// Load the first-level data of position `j` (a FOR row word, PLAIN offsets) and return something
    /// derived from it, so independent lookups can overlap their cache misses.
    #[inline]
    pub fn touch(&self, file: &[u8], j: usize) -> u64 {
        let mut x = 0;
        for f in &self.fields {
            match f.enc {
                ENC_FOR => {
                    x ^= read_u64(file, f.data + (j * f.stride + f.bit_off) / 64 * 8);
                    if f.stride == f.bits as usize {
                        continue;
                    }
                    break; // fields sharing a row are in the same place
                }
                ENC_PLAIN => x ^= read_u32(file, f.data + 4 * j) as u64,
                _ => {}
            }
        }
        x
    }

    /// Load the second-level data of position `j` (the first byte of PLAIN strings).
    #[inline]
    pub fn touch2(&self, file: &[u8], j: usize) -> u64 {
        let mut x = 0;
        for (k, f) in self.fields.iter().enumerate() {
            if f.enc == ENC_PLAIN {
                x ^= self.str(file, k, j).first().copied().unwrap_or(0) as u64;
            }
        }
        x
    }

    /// Field values at position `j`.
    pub fn key<'a>(&self, file: &'a [u8], j: usize, out: &mut Vec<Val<'a>>) {
        out.clear();
        for (k, f) in self.fields.iter().enumerate() {
            out.push(if f.enc == ENC_PLAIN { Val::Str(self.str(file, k, j)) } else { Val::Int(self.int(file, k, j)) });
        }
    }

    /// Append the ID at position `j` to `out`.
    pub fn decode(&self, file: &[u8], tmpl: &Template, j: usize, out: &mut Vec<u8>) {
        let mut k = 0;
        for p in tmpl.parts() {
            match p {
                Part::Lit(l) => out.extend_from_slice(l),
                Part::Str => {
                    out.extend_from_slice(self.str(file, k, j));
                    k += 1;
                }
                Part::Dec => {
                    write_dec(self.int(file, k, j), 0, out);
                    k += 1;
                }
                Part::DecW(w) => {
                    write_dec(self.int(file, k, j), *w as usize, out);
                    k += 1;
                }
                Part::HexL(w) => {
                    write_hex(self.int(file, k, j), *w as usize, false, out);
                    k += 1;
                }
                Part::HexU(w) => {
                    write_hex(self.int(file, k, j), *w as usize, true, out);
                    k += 1;
                }
            }
        }
    }
}
