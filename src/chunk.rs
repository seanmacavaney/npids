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
                if e.enc == ENC_FOR { bits::packed_len(count, e.bits) } else { 0 }
            }
            Column::Str(v) => (4 * (count + 1) + v.iter().map(|s| s.len()).sum::<usize>()).next_multiple_of(8),
        };
    }
    c
}

pub fn encode(columns: &[Column], count: usize) -> Vec<u8> {
    assert!(count > 0);
    let mut head = W::default();
    head.u32(count as u32).u16(columns.len() as u16).u16(0);
    let mut data = W::default();
    let data_start = CHUNK_HEADER + FIELD_DESC * columns.len();
    for col in columns {
        let off = data_start + data.len();
        let (enc, b, p0, p1) = match col {
            Column::Int(v) => {
                debug_assert_eq!(v.len(), count);
                let e = choose_int(v);
                if e.enc == ENC_FOR {
                    let rel: Vec<u64> = v.iter().map(|&x| x - e.p0).collect();
                    data.bytes(&bits::pack(&rel, e.bits));
                }
                (e.enc, e.bits, e.p0, e.p1)
            }
            Column::Str(v) => {
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
                (ENC_PLAIN, 0, 0, 0)
            }
        };
        let len = data_start + data.len() - off;
        head.u8(enc).u8(b).zeros(6).u64(p0).u64(p1).u32(off as u32).u32(len as u32);
    }
    head.bytes(&data.0);
    head.0
}

#[derive(Clone, Debug)]
pub struct FieldCol {
    pub enc: u8,
    pub bits: u8,
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
            let f = FieldCol {
                enc: d[0],
                bits: d[1],
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
                (ENC_FOR, p) => p.is_int() && f.bits <= 64 && f.data_len >= bits::packed_len(count, f.bits),
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
        Ok(ChunkInfo { count, template_id: e.template_id, fields })
    }

    #[inline]
    pub fn int(&self, file: &[u8], k: usize, j: usize) -> u64 {
        let f = &self.fields[k];
        match f.enc {
            ENC_CONST => f.p0,
            ENC_SEQ => f.p0.wrapping_add((j as u64).wrapping_mul(f.p1)),
            _ => f.p0.wrapping_add(bits::get(&file[f.data..f.data + f.data_len], f.bits, j)),
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
