//! Choosing a template for a chunk. Not part of the format: any template that every ID satisfies is
//! valid; this picks the smallest encoding among a few candidates.

use crate::chunk::{self, Column};
use crate::template::{Part, Template, Val};

/// Pick the cheapest template that every ID round-trips through, and return its columns.
pub fn choose<'a>(ids: &[&'a [u8]]) -> (Template, Vec<Column<'a>>) {
    assert!(!ids.is_empty());
    let sample: Vec<&[u8]> = {
        let step = (ids.len() / 64).max(1);
        ids.iter().step_by(step).copied().collect()
    };
    let mut best: Option<(usize, Template, Vec<Column<'a>>)> = None;
    let mut seen: Vec<Vec<Part>> = Vec::new();
    for parts in candidates(ids) {
        if seen.contains(&parts) {
            continue;
        }
        seen.push(parts.clone());
        let Ok(t) = Template::new(parts) else { continue };
        if !matches_all(&t, &sample) {
            continue;
        }
        if let Some(cols) = columns(&t, ids) {
            let c = chunk::cost(&cols, ids.len());
            if best.as_ref().is_none_or(|b| c < b.0) {
                best = Some((c, t, cols));
            }
        }
    }
    let (_, t, cols) = best.expect("{str} always matches");
    (t, cols)
}

fn matches_all(t: &Template, ids: &[&[u8]]) -> bool {
    let (mut vals, mut scratch) = (Vec::new(), Vec::new());
    ids.iter().all(|id| t.parse_exact(id, &mut vals, &mut scratch))
}

/// Parse every ID into columns, or None if any ID does not round-trip.
pub fn columns<'a>(t: &Template, ids: &[&'a [u8]]) -> Option<Vec<Column<'a>>> {
    let mut cols: Vec<Column<'a>> = t
        .fields()
        .map(|p| if p.is_int() { Column::Int(Vec::with_capacity(ids.len())) } else { Column::Str(Vec::with_capacity(ids.len())) })
        .collect();
    let (mut vals, mut scratch) = (Vec::new(), Vec::new());
    for id in ids {
        if !t.parse_exact(id, &mut vals, &mut scratch) {
            return None;
        }
        for (col, v) in cols.iter_mut().zip(&vals) {
            match (col, v) {
                (Column::Int(c), Val::Int(x)) => c.push(*x),
                (Column::Str(c), Val::Str(s)) => c.push(s),
                _ => unreachable!(),
            }
        }
    }
    Some(cols)
}

fn candidates(ids: &[&[u8]]) -> Vec<Vec<Part>> {
    let first = ids[0];
    let mut out = Vec::new();
    out.push(digit_parts(first, false));
    out.push(digit_parts(first, true));
    out.push(hex_parts(first, false));
    out.push(hex_parts(first, true));
    out.push(affix_parts(ids));
    out.push(vec![Part::Str]);
    out
}

/// Runs of `pred` bytes in `id`, as (start, end).
fn runs(id: &[u8], pred: impl Fn(u8) -> bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < id.len() {
        if pred(id[i]) {
            let s = i;
            while i < id.len() && pred(id[i]) {
                i += 1;
            }
            out.push((s, i));
        } else {
            i += 1;
        }
    }
    out
}

/// Build parts from `id`, turning each selected run into fields via `field` and the rest into literals.
fn with_fields(id: &[u8], selected: &[(usize, usize)], field: impl Fn(&[u8]) -> Vec<Part>) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut pos = 0;
    for &(s, e) in selected {
        if s > pos {
            parts.push(Part::Lit(id[pos..s].to_vec()));
        }
        parts.extend(field(&id[s..e]));
        pos = e;
    }
    if pos < id.len() {
        parts.push(Part::Lit(id[pos..].to_vec()));
    }
    parts
}

/// Decimal fields for digit runs (all runs, or only the last one).
fn digit_parts(id: &[u8], last_only: bool) -> Vec<Part> {
    let mut rs = runs(id, |b| b.is_ascii_digit());
    rs.retain(|&(s, e)| e - s <= 19);
    if last_only && rs.len() > 1 {
        rs.drain(..rs.len() - 1);
    }
    with_fields(id, &rs, |run| {
        if run.len() > 1 && run[0] == b'0' { vec![Part::DecW(run.len() as u8)] } else { vec![Part::Dec] }
    })
}

/// Hex fields for runs of at least 4 hex digits, split into fields of at most 16 digits.
fn hex_parts(id: &[u8], upper: bool) -> Vec<Part> {
    let is_hex = |b: u8| b.is_ascii_digit() || if upper { (b'A'..=b'F').contains(&b) } else { (b'a'..=b'f').contains(&b) };
    let mut rs = runs(id, is_hex);
    rs.retain(|&(s, e)| e - s >= 4);
    with_fields(id, &rs, |run| {
        let mut fs = Vec::new();
        let mut n = run.len();
        while n > 0 {
            let w = n.min(16);
            fs.push(if upper { Part::HexU(w as u8) } else { Part::HexL(w as u8) });
            n -= w;
        }
        fs
    })
}

/// Longest common prefix and suffix around a STR field.
fn affix_parts(ids: &[&[u8]]) -> Vec<Part> {
    let first = ids[0];
    let min_len = ids.iter().map(|i| i.len()).min().unwrap();
    let mut p = first.len().min(min_len);
    let mut s = p;
    for id in ids {
        p = first[..p].iter().zip(id.iter()).take_while(|(a, b)| a == b).count();
        s = first.iter().rev().zip(id.iter().rev()).take(s).take_while(|(a, b)| a == b).count();
    }
    if p + s > min_len {
        s = min_len - p;
    }
    let mut parts = Vec::new();
    if p > 0 {
        parts.push(Part::Lit(first[..p].to_vec()));
    }
    parts.push(Part::Str);
    if s > 0 {
        parts.push(Part::Lit(first[first.len() - s..].to_vec()));
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(ids: &[&str]) -> String {
        let ids: Vec<&[u8]> = ids.iter().map(|s| s.as_bytes()).collect();
        choose(&ids).0.to_string()
    }

    #[test]
    fn picks_sensible_templates() {
        assert_eq!(pick(&["0", "1", "2", "3"]), "{dec}");
        assert_eq!(pick(&["D1555982", "D301595", "D1359209"]), "D{dec}");
        let mut x = 0x1234_5678_9abc_def0u64;
        let uuids: Vec<String> = (0..500)
            .map(|_| {
                let mut h = || {
                    x = crate::phf::fmix64(x.wrapping_add(1));
                    x
                };
                let (a, b) = (h(), h());
                format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", a >> 32, a & 0xffff, (a >> 16) & 0xffff, b >> 48, b & 0xffff_ffff_ffff)
            })
            .collect();
        let uuids: Vec<&str> = uuids.iter().map(|s| s.as_str()).collect();
        assert_eq!(pick(&uuids), "{hex:8}-{hex:4}-{hex:4}-{hex:4}-{hex:12}");
        assert_eq!(pick(&["doc-00001", "doc-00002"]), "doc-{dec:5}");
        assert_eq!(pick(&["FBIS3-1", "FR940104-0-00001", "LA010189-0001"]), "{str}1");
        assert_eq!(pick(&["<a>x.txt", "<a>yy.txt"]), "<a>{str}.txt");
        let digests: Vec<String> = (0..500u64).map(|i| format!("{:016x}{:016x}", crate::phf::fmix64(i), crate::phf::fmix64(!i))).collect();
        let digests: Vec<&str> = digests.iter().map(|s| s.as_str()).collect();
        assert_eq!(pick(&digests), "{hex:16}{hex:16}");
    }
}
