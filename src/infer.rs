//! Choosing a template for a chunk. Not part of the format: any template that every ID satisfies is
//! valid; this picks the smallest encoding among a few candidates.

use crate::chunk::{self, Column};
use crate::template::{Part, Template, Val};

/// Pick a template that every ID round-trips through, and return its columns.
///
/// Among the candidates that cost at most a little more than the cheapest one, prefer `prev` (the
/// previous chunk's template), then the one with the most fields: neighbouring chunks that share a
/// template can share one derived inverse unit, which saves far more than a constant column costs.
pub fn choose<'a>(ids: &[&'a [u8]], prev: Option<&Template>) -> (Template, Vec<Column<'a>>) {
    assert!(!ids.is_empty());
    let mut sample: Vec<&[u8]> = {
        let step = (ids.len() / 64).max(1);
        ids.iter().step_by(step).copied().collect()
    };
    sample.push(ids[ids.len() - 1]);
    let mut found: Vec<(usize, Template, Vec<Column<'a>>)> = Vec::new();
    let mut seen: Vec<Vec<Part>> = Vec::new();
    let cands = prev.map(|t| t.parts().to_vec()).into_iter().chain(candidates(ids, &sample));
    for parts in cands {
        if seen.contains(&parts) {
            continue;
        }
        seen.push(parts.clone());
        let Ok(t) = Template::new(parts) else { continue };
        if !matches_all(&t, &sample) {
            continue;
        }
        if let Some(cols) = columns(&t, ids) {
            found.push((chunk::cost(&cols, ids.len()), t, cols));
        }
    }
    let min = found.iter().map(|f| f.0).min().expect("{str} always matches");
    let slack = min + min / 256 + 2 * chunk::FIELD_DESC;
    let pick = found
        .into_iter()
        .enumerate()
        .filter(|(_, f)| f.0 <= slack)
        .max_by_key(|(i, f)| (prev == Some(&f.1), f.2.len(), std::cmp::Reverse((f.0, *i))))
        .unwrap()
        .1;
    (pick.1, pick.2)
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

fn candidates(ids: &[&[u8]], sample: &[&[u8]]) -> Vec<Vec<Part>> {
    let first = ids[0];
    let mut out = Vec::new();
    out.extend(sampled_digit_parts(sample));
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
fn with_fields(id: &[u8], selected: &[(usize, usize)], mut field: impl FnMut(&[u8]) -> Vec<Part>) -> Vec<Part> {
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

/// Decimal fields for the digit runs of the first ID, fixed-width where the run has the same length
/// in every sampled ID (with the same number of runs) and a leading zero in at least one of them.
/// Unlike `digit_parts`, this doesn't depend on whether the first ID happens to have a leading zero.
fn sampled_digit_parts(sample: &[&[u8]]) -> Option<Vec<Part>> {
    let digit = |b: u8| b.is_ascii_digit();
    let first = sample[0];
    let rs = runs(first, digit);
    if rs.is_empty() || rs.iter().any(|&(s, e)| e - s > 19) {
        return None;
    }
    let mut fixed = vec![true; rs.len()];
    let mut zero = vec![false; rs.len()];
    for id in sample {
        let r = runs(id, digit);
        if r.len() != rs.len() {
            continue;
        }
        for (k, (&(s, e), &(s0, e0))) in r.iter().zip(&rs).enumerate() {
            fixed[k] &= e - s == e0 - s0;
            zero[k] |= e - s > 1 && id[s] == b'0';
        }
    }
    let mut k = 0;
    Some(with_fields(first, &rs, |run| {
        let p = if fixed[k] && zero[k] { Part::DecW(run.len() as u8) } else { Part::Dec };
        k += 1;
        vec![p]
    }))
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
        choose(&ids, None).0.to_string()
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
        // Fixed-width fields are detected even when the first ID has no leading zero.
        assert_eq!(pick(&["cw-07-12345", "cw-07-99999", "cw-08-00000", "cw-08-00001"]), "cw-{dec:2}-{dec:5}");
        // A constant field is kept as a field rather than a literal, so neighbouring chunks share it.
        let ids: Vec<String> = (0..1000).map(|i| format!("p_00_{}", i * 977)).collect();
        let ids: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        assert_eq!(pick(&ids), "p_{dec:2}_{dec}");
    }

    #[test]
    fn prefers_previous_template() {
        let prev = Template::new(vec![Part::Lit(b"x".to_vec()), Part::Dec]).unwrap();
        let ids: Vec<&[u8]> = vec![b"x1", b"x2", b"x3"];
        assert_eq!(choose(&ids, Some(&prev)).0, prev);
    }
}
