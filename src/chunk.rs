//! Chunk payloads (FORMAT.md, "Chunk payload").

use crate::bits::{self, W, read_u32, read_u64};
use crate::format::ChunkEntry;
use crate::template::{Part, Template, Val, write_dec, write_hex};
use crate::{Error, Result};

pub const ENC_CONST: u8 = 0;
pub const ENC_SEQ: u8 = 1;
pub const ENC_FOR: u8 = 2;
pub const ENC_PLAIN: u8 = 3;
pub const ENC_RUNS: u8 = 4;

const CHUNK_HEADER: usize = 8;
pub const FIELD_DESC: usize = 32;

/// Field values of a chunk, one column per template field.
pub enum Column<'a> {
    Int(Vec<u64>),
    Str(Vec<&'a [u8]>),
}

/// How one field of a chunk is stored.
enum FieldPlan {
    Const(u64),
    Seq(u64, u64),
    For { min: u64, bits: u8 },
    /// Column `slot` of run table `group`, arithmetic with `step` within each run.
    Runs { step: u64, group: usize, slot: usize },
    Plain,
}

/// The encoding of every field of a chunk, plus the run tables (their breakpoints) of RUNS fields.
struct Plan {
    fields: Vec<FieldPlan>,
    /// Breakpoints (`starts`) and number of fields (`width`) of each run table.
    groups: Vec<(Vec<u32>, usize)>,
}

/// The positions where a new run starts if `vals` is split into runs `v, v + step, v + 2 * step, ...`,
/// or None if there would be more than `limit` runs.
fn breakpoints(vals: &[u64], step: u64, limit: usize) -> Option<Vec<u32>> {
    let mut starts = vec![0u32];
    for j in 1..vals.len() {
        if vals[j] != vals[j - 1].wrapping_add(step) {
            if starts.len() == limit {
                return None;
            }
            starts.push(j as u32);
        }
    }
    Some(starts)
}

/// Bytes of a run table with `n` runs and `width` fields.
fn runs_len(n: usize, width: usize) -> usize {
    (4 * n).next_multiple_of(8) + 8 * n * width
}

fn plan(columns: &[Column], count: usize) -> Plan {
    let mut fields = Vec::with_capacity(columns.len());
    // RUNS candidates: (field, step, breakpoints).
    let mut runs: Vec<(usize, u64, Vec<u32>)> = Vec::new();
    for (k, col) in columns.iter().enumerate() {
        let Column::Int(vals) = col else {
            fields.push(FieldPlan::Plain);
            continue;
        };
        debug_assert_eq!(vals.len(), count);
        let v0 = vals[0];
        if vals.iter().all(|&v| v == v0) {
            fields.push(FieldPlan::Const(v0));
            continue;
        }
        let step = vals[1].wrapping_sub(v0);
        if vals.iter().enumerate().all(|(j, &v)| v == v0.wrapping_add((j as u64).wrapping_mul(step))) {
            fields.push(FieldPlan::Seq(v0, step));
            continue;
        }
        let (min, max) = vals.iter().fold((u64::MAX, 0), |(a, b), &v| (a.min(v), b.max(v)));
        let bits = bits::bitlen(max - min);
        fields.push(FieldPlan::For { min, bits });
        // Runs only replace bit packing when they take at most half its space, which also bounds the
        // number of runs a lookup searches.
        let limit = bits::packed_len(count, bits as usize) / 2 / runs_len(1, 1);
        if limit < 2 {
            continue;
        }
        // Candidate steps: 0 (piecewise constant) and the most common difference (majority vote).
        let (mut cand, mut votes) = (0u64, 0usize);
        for w in vals.windows(2) {
            let d = w[1].wrapping_sub(w[0]);
            if votes == 0 {
                cand = d;
            }
            votes = if d == cand { votes + 1 } else { votes - 1 };
        }
        let mut best: Option<(u64, Vec<u32>)> = None;
        for s in [0, cand] {
            if best.as_ref().is_some_and(|b| b.0 == s) {
                continue;
            }
            let lim = best.as_ref().map_or(limit, |b| b.1.len() - 1);
            if let Some(b) = breakpoints(vals, s, lim) {
                best = Some((s, b));
            }
        }
        if let Some((s, b)) = best {
            runs.push((k, s, b));
        }
    }
    let mut groups = Vec::new();
    if !runs.is_empty() {
        // One shared table over the union of the breakpoints, if that is no bigger than separate tables
        // (as when fields change only where the last one restarts).
        let mut union: Vec<u32> = runs.iter().flat_map(|r| r.2.iter().copied()).collect();
        union.sort_unstable();
        union.dedup();
        let separate: usize = runs.iter().map(|r| runs_len(r.2.len(), 1)).sum();
        if runs.len() > 1 && runs_len(union.len(), runs.len()) <= separate {
            for (slot, (k, step, _)) in runs.iter().enumerate() {
                fields[*k] = FieldPlan::Runs { step: *step, group: 0, slot };
            }
            groups.push((union, runs.len()));
        } else {
            for (k, step, b) in runs {
                fields[k] = FieldPlan::Runs { step, group: groups.len(), slot: 0 };
                groups.push((b, 1));
            }
        }
    }
    Plan { fields, groups }
}

/// Encoded size in bytes of a chunk with these columns (used to choose a template).
pub fn cost(columns: &[Column], count: usize) -> usize {
    let p = plan(columns, count);
    let mut c = CHUNK_HEADER + FIELD_DESC * columns.len();
    for (f, col) in p.fields.iter().zip(columns) {
        c += match (f, col) {
            (FieldPlan::For { bits, .. }, _) => bits::packed_len(count, *bits as usize),
            (FieldPlan::Plain, Column::Str(v)) => (4 * (count + 1) + v.iter().map(|s| s.len()).sum::<usize>()).next_multiple_of(8),
            _ => 0,
        };
    }
    c + p.groups.iter().map(|(s, w)| runs_len(s.len(), *w)).sum::<usize>()
}

pub fn encode(columns: &[Column], count: usize) -> Vec<u8> {
    assert!(count > 0);
    let p = plan(columns, count);
    let int = |k: usize| -> &[u64] {
        let Column::Int(v) = &columns[k] else { unreachable!() };
        v
    };
    let for_fields: Vec<usize> = (0..columns.len()).filter(|&k| matches!(p.fields[k], FieldPlan::For { .. })).collect();
    let rel = |k: usize| -> Vec<u64> {
        let FieldPlan::For { min, .. } = p.fields[k] else { unreachable!() };
        int(k).iter().map(|&x| x - min).collect()
    };

    let mut head = W::default();
    head.u32(count as u32).u16(columns.len() as u16).u16(0);
    let mut data = W::default();
    let data_start = CHUNK_HEADER + FIELD_DESC * columns.len();
    // With two or more FOR fields, store them row by row in one stream (FORMAT.md, "Chunk payload").
    let mut shared: Option<(usize, usize, usize, Vec<usize>)> = None; // (off, len, stride, bit_offs)
    if for_fields.len() >= 2 {
        let rels: Vec<Vec<u64>> = for_fields.iter().map(|&k| rel(k)).collect();
        let cols: Vec<(&[u64], u8)> = for_fields
            .iter()
            .zip(&rels)
            .map(|(&k, r)| match p.fields[k] {
                FieldPlan::For { bits, .. } => (r.as_slice(), bits),
                _ => unreachable!(),
            })
            .collect();
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
    // Run tables: breakpoints, then the first value of every field of the table, run by run.
    let mut tables = Vec::with_capacity(p.groups.len()); // (off, len)
    for (g, (starts, width)) in p.groups.iter().enumerate() {
        let off = data_start + data.len();
        for &s in starts {
            data.u32(s);
        }
        data.pad8();
        let mut members: Vec<(usize, usize)> = (0..columns.len())
            .filter_map(|k| match p.fields[k] {
                FieldPlan::Runs { group, slot, .. } if group == g => Some((slot, k)),
                _ => None,
            })
            .collect();
        members.sort_unstable();
        debug_assert_eq!(members.len(), *width);
        for &s in starts {
            for &(_, k) in &members {
                data.u64(int(k)[s as usize]);
            }
        }
        tables.push((off, data_start + data.len() - off));
    }
    for (k, col) in columns.iter().enumerate() {
        let off = data_start + data.len();
        let (enc, b, p0, p1, stride, bit_off, d_off, d_len) = match (&p.fields[k], col) {
            (FieldPlan::Const(v), _) => (ENC_CONST, 0, *v, 0, 0, 0, off, 0),
            (FieldPlan::Seq(v, s), _) => (ENC_SEQ, 0, *v, *s, 0, 0, off, 0),
            (FieldPlan::For { min, bits }, _) => match &shared {
                Some((s_off, s_len, stride, offs)) => {
                    let i = for_fields.iter().position(|&f| f == k).unwrap();
                    (ENC_FOR, *bits, *min, 0, *stride as u16, offs[i] as u16, *s_off, *s_len)
                }
                None => {
                    data.bytes(&bits::pack(&rel(k), *bits));
                    (ENC_FOR, *bits, *min, 0, 0, 0, off, data_start + data.len() - off)
                }
            },
            (FieldPlan::Runs { step, group, slot }, _) => {
                let (starts, width) = &p.groups[*group];
                let (t_off, t_len) = tables[*group];
                (ENC_RUNS, 0, *step, starts.len() as u64, *width as u16, *slot as u16, t_off, t_len)
            }
            (FieldPlan::Plain, Column::Str(v)) => {
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
    /// FOR: row stride in bits and this field's offset in a row. RUNS: the run table's number of
    /// fields and this field's column in it.
    pub stride: usize,
    pub bit_off: usize,
    pub p0: u64,
    pub p1: u64,
    /// Absolute offset of the column data in the file.
    pub data: usize,
    pub data_len: usize,
    /// RUNS only: absolute offset of the run table's values (after its breakpoints).
    pub vals: usize,
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
            let mut f = FieldCol {
                enc: d[0],
                bits: d[1],
                stride: if stride != 0 { stride } else if d[0] == ENC_RUNS { 1 } else { d[1] as usize },
                bit_off: bits::read_u16(d, 4) as usize,
                p0: read_u64(d, 8),
                p1: read_u64(d, 16),
                data: start + read_u32(d, 24) as usize,
                data_len: read_u32(d, 28) as usize,
                vals: 0,
            };
            if f.enc == ENC_RUNS && f.p1 <= count as u64 {
                f.vals = f.data + (4 * f.p1 as usize).next_multiple_of(8);
            }
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
                (ENC_RUNS, p) => p.is_int() && valid_runs(file, &f, count),
                (ENC_CONST..=ENC_RUNS, _) => false,
                (enc, _) => {
                    return Err(Error::Unsupported(format!(
                        "unknown column encoding {enc} (the file was written by a newer version of npids; please upgrade npids)"
                    )));
                }
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
        self.int_cached(file, k, j, &mut (usize::MAX, 0))
    }

    /// Like `int`; `cache` remembers the run found in a run table (by its offset), so fields that
    /// share a table search it once per position.
    #[inline]
    fn int_cached(&self, file: &[u8], k: usize, j: usize, cache: &mut (usize, usize)) -> u64 {
        let f = &self.fields[k];
        match f.enc {
            ENC_CONST => f.p0,
            ENC_SEQ => f.p0.wrapping_add((j as u64).wrapping_mul(f.p1)),
            ENC_RUNS => {
                if cache.0 != f.data {
                    *cache = (f.data, find_run(file, f.data, f.p1 as usize, j));
                }
                let r = cache.1;
                let start = read_u32(file, f.data + 4 * r) as u64;
                read_u64(file, f.vals + 8 * (r * f.stride + f.bit_off)).wrapping_add((j as u64 - start).wrapping_mul(f.p0))
            }
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

    /// The run of RUNS field `f` containing position `j`.
    #[inline]
    pub fn run_of(&self, file: &[u8], f: &FieldCol, j: usize) -> usize {
        find_run(file, f.data, f.p1 as usize, j)
    }

    /// The first position of run `r` of RUNS field `f`.
    #[inline]
    pub fn run_start(&self, file: &[u8], f: &FieldCol, r: usize) -> usize {
        read_u32(file, f.data + 4 * r) as usize
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
        let mut cache = (usize::MAX, 0);
        for (k, f) in self.fields.iter().enumerate() {
            out.push(if f.enc == ENC_PLAIN { Val::Str(self.str(file, k, j)) } else { Val::Int(self.int_cached(file, k, j, &mut cache)) });
        }
    }

    /// Append the ID at position `j` to `out`.
    pub fn decode(&self, file: &[u8], tmpl: &Template, j: usize, out: &mut Vec<u8>) {
        let mut k = 0;
        let mut cache = (usize::MAX, 0);
        for p in tmpl.parts() {
            match p {
                Part::Lit(l) => out.extend_from_slice(l),
                Part::Str => {
                    out.extend_from_slice(self.str(file, k, j));
                    k += 1;
                }
                Part::Dec(w) => {
                    write_dec(self.int_cached(file, k, j, &mut cache), *w as usize, out);
                    k += 1;
                }
                Part::DecW(w) => {
                    write_dec(self.int_cached(file, k, j, &mut cache), *w as usize, out);
                    k += 1;
                }
                Part::HexL(w) => {
                    write_hex(self.int_cached(file, k, j, &mut cache), *w as usize, false, out);
                    k += 1;
                }
                Part::HexU(w) => {
                    write_hex(self.int_cached(file, k, j, &mut cache), *w as usize, true, out);
                    k += 1;
                }
            }
        }
    }
}

/// The run containing position `j`: the last `r` with `starts[r] <= j`, for the `n` breakpoints at
/// byte `off` (`starts[0] = 0`).
#[inline]
fn find_run(file: &[u8], off: usize, n: usize, j: usize) -> usize {
    let (mut lo, mut len) = (0, n);
    while len > 1 {
        let half = len / 2;
        if read_u32(file, off + 4 * (lo + half)) as usize <= j {
            lo += half;
        }
        len -= half;
    }
    lo
}

/// Whether a RUNS field's table is in bounds and its breakpoints start at 0 and strictly increase
/// below `count`.
fn valid_runs(file: &[u8], f: &FieldCol, count: usize) -> bool {
    let n = f.p1;
    if n == 0 || n > count as u64 || f.bits != 0 || f.bit_off >= f.stride {
        return false;
    }
    let n = n as usize;
    let Some(need) = n.checked_mul(f.stride).and_then(|x| x.checked_mul(8)) else { return false };
    if f.data_len < (4 * n).next_multiple_of(8) + need {
        return false;
    }
    let mut prev = 0u32;
    for r in 0..n {
        let s = read_u32(file, f.data + 4 * r);
        if (r == 0 && s != 0) || (r > 0 && s <= prev) || s as usize >= count {
            return false;
        }
        prev = s;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode integer columns as one chunk of template `{dec}-{dec}-...` and read every value back.
    fn roundtrip(cols: &[Vec<u64>]) -> (Vec<u8>, ChunkInfo) {
        let count = cols[0].len();
        let mut parts = Vec::new();
        for k in 0..cols.len() {
            if k > 0 {
                parts.push(Part::Lit(b"-".to_vec()));
            }
            parts.push(Part::Dec(0));
        }
        let t = Template::new(parts).unwrap();
        let columns: Vec<Column> = cols.iter().map(|c| Column::Int(c.clone())).collect();
        let payload = encode(&columns, count);
        assert_eq!(payload.len(), cost(&columns, count));
        let e = ChunkEntry { offset: 0, len: payload.len() as u32, count: count as u32, template_id: 0, crc: 0 };
        let info = ChunkInfo::read(&payload, &e, &t).unwrap();
        for (k, c) in cols.iter().enumerate() {
            for (j, &v) in c.iter().enumerate() {
                assert_eq!(info.int(&payload, k, j), v, "field {k} position {j}");
            }
        }
        let mut out = Vec::new();
        for j in 0..count {
            out.clear();
            info.decode(&payload, &t, j, &mut out);
            let want: Vec<String> = cols.iter().map(|c| c[j].to_string()).collect();
            assert_eq!(out, want.join("-").as_bytes());
        }
        (payload, info)
    }

    /// Columns of `clueweb22-en{a}-{b}-{i}`: `i` counts up in runs of the given lengths.
    fn cw22(runs: &[u64]) -> Vec<Vec<u64>> {
        let mut cols = vec![Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        for (r, &len) in runs.iter().enumerate() {
            for i in 0..len {
                cols[0].push(22);
                cols[1].push(r as u64 / 3);
                cols[2].push(r as u64 % 3);
                cols[3].push(i);
            }
        }
        cols
    }

    #[test]
    fn runs_share_a_table() {
        let (payload, info) = roundtrip(&cw22(&[300, 1000, 17, 2000, 1]));
        let encs: Vec<u8> = info.fields.iter().map(|f| f.enc).collect();
        assert_eq!(encs, [ENC_CONST, ENC_RUNS, ENC_RUNS, ENC_RUNS]);
        assert!(info.fields[1..].iter().all(|f| f.data == info.fields[1].data && f.stride == 3 && f.p1 == 5));
        assert_eq!(info.fields.iter().map(|f| f.p0).collect::<Vec<_>>(), [22, 0, 0, 1]);
        assert!(payload.len() < 8 + 4 * 32 + 8 * 4 * 5);
    }

    #[test]
    fn runs_separate_tables() {
        // Breakpoints of the two fields don't nest: separate tables are smaller than their union.
        let a: Vec<u64> = (0..4000u64).map(|i| (i / 1000) * 5).collect();
        let b: Vec<u64> = (0..4000u64).map(|i| 1000 + (i + 300) % 700 * 2).collect();
        let (_, info) = roundtrip(&[a, b]);
        assert!(info.fields.iter().all(|f| f.enc == ENC_RUNS && f.stride == 1));
        assert_ne!(info.fields[0].data, info.fields[1].data);
        assert_eq!((info.fields[0].p0, info.fields[0].p1), (0, 4));
        assert_eq!((info.fields[1].p0, info.fields[1].p1), (2, 7));
    }

    #[test]
    fn runs_negative_step_and_wrapping() {
        let a: Vec<u64> = (0..3000u64).map(|i| 1_000_000 - (i % 600) * 3).collect();
        let (_, info) = roundtrip(&[a]);
        assert_eq!((info.fields[0].enc, info.fields[0].p0 as i64), (ENC_RUNS, -3));
        let b: Vec<u64> = (0..3000u64).map(|i| u64::MAX - 5 + (i % 1000)).collect();
        let (_, info) = roundtrip(&[b]);
        assert_eq!((info.fields[0].enc, info.fields[0].p0), (ENC_RUNS, 1));
    }

    #[test]
    fn many_runs_stay_bit_packed() {
        // Short runs would cost more than half of the bit packing.
        let a: Vec<u64> = (0..3000u64).map(|i| i % 3 + (i / 3) * 1000).collect();
        let (_, info) = roundtrip(&[a]);
        assert_eq!(info.fields[0].enc, ENC_FOR);
        let mut x = 7u64;
        let r: Vec<u64> = (0..3000).map(|_| { x = crate::phf::fmix64(x); x >> 40 }).collect();
        let (_, info) = roundtrip(&[r]);
        assert_eq!(info.fields[0].enc, ENC_FOR);
    }

    #[test]
    fn invalid_run_tables_are_corrupt() {
        let cols = cw22(&[300, 1000, 17]);
        let t = Template::new(vec![Part::Dec(0), Part::Lit(b"-".to_vec()), Part::Dec(0), Part::Lit(b"-".to_vec()), Part::Dec(0), Part::Lit(b"-".to_vec()), Part::Dec(0)]).unwrap();
        let columns: Vec<Column> = cols.iter().map(|c| Column::Int(c.clone())).collect();
        let payload = encode(&columns, 1317);
        let table = read_u32(&payload, CHUNK_HEADER + 2 * FIELD_DESC + 24) as usize; // field 2 (`r % 3`)
        let e = ChunkEntry { offset: 0, len: payload.len() as u32, count: 1317, template_id: 0, crc: 0 };
        for (pos, val) in [(table, 1u32), (table + 4, 0), (table + 8, 1317), (table + 8, 200)] {
            let mut p = payload.clone();
            p[pos..pos + 4].copy_from_slice(&val.to_le_bytes());
            assert!(matches!(ChunkInfo::read(&p, &e, &t), Err(Error::Corrupt(_))), "{pos} {val}");
        }
        let mut p = payload.clone();
        p[CHUNK_HEADER + 2 * FIELD_DESC + 17] = 200; // n_runs beyond count
        assert!(matches!(ChunkInfo::read(&p, &e, &t), Err(Error::Corrupt(_))));
    }
}
