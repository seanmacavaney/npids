//! ID templates (FORMAT.md, "Templates").

use crate::{Error, Result};
use regex::bytes::Regex;
use std::fmt;

pub const KIND_LIT: u8 = 0;
pub const KIND_DEC: u8 = 1;
pub const KIND_DECW: u8 = 2;
pub const KIND_HEXL: u8 = 3;
pub const KIND_HEXU: u8 = 4;
pub const KIND_STR: u8 = 5;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Lit(Vec<u8>),
    Dec,
    DecW(u8),
    HexL(u8),
    HexU(u8),
    Str,
}

impl Part {
    pub fn is_field(&self) -> bool {
        !matches!(self, Part::Lit(_))
    }
    pub fn is_int(&self) -> bool {
        matches!(self, Part::Dec | Part::DecW(_) | Part::HexL(_) | Part::HexU(_))
    }
    /// Length in bytes if the part always matches a fixed number of bytes.
    pub fn fixed_len(&self) -> Option<usize> {
        match self {
            Part::Lit(l) => Some(l.len()),
            Part::DecW(w) | Part::HexL(w) | Part::HexU(w) => Some(*w as usize),
            Part::Dec | Part::Str => None,
        }
    }
}

/// A parsed field value. Keys compare lexicographically over these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Val<'a> {
    Int(u64),
    Str(&'a [u8]),
}

#[derive(Clone)]
pub struct Template {
    parts: Vec<Part>,
    re: Regex,
    /// True if the hand-written parser gives the same result as the regex (see `is_simple`).
    simple: bool,
    n_fields: usize,
}

impl fmt::Debug for Template {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Template({self})")
    }
}

impl PartialEq for Template {
    fn eq(&self, other: &Self) -> bool {
        self.parts == other.parts
    }
}
impl Eq for Template {}

impl Template {
    pub fn new(parts: Vec<Part>) -> Result<Template> {
        let mut n_str = 0;
        for p in &parts {
            match p {
                Part::DecW(w) if *w == 0 || *w > 20 => {
                    return Err(Error::Invalid(format!("DECW width {w} out of range")));
                }
                Part::HexL(w) | Part::HexU(w) if *w == 0 || *w > 16 => {
                    return Err(Error::Invalid(format!("HEX width {w} out of range")));
                }
                Part::Lit(l) if l.len() > u16::MAX as usize => {
                    return Err(Error::Invalid("literal too long".into()));
                }
                Part::Str => n_str += 1,
                _ => {}
            }
        }
        if n_str > 1 {
            return Err(Error::Invalid("a template has at most one STR part".into()));
        }
        let re = Regex::new(&regex_source(&parts)).map_err(|e| Error::Invalid(e.to_string()))?;
        let simple = is_simple(&parts);
        let n_fields = parts.iter().filter(|p| p.is_field()).count();
        Ok(Template { parts, re, simple, n_fields })
    }

    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    pub fn n_fields(&self) -> usize {
        self.n_fields
    }

    pub fn fields(&self) -> impl Iterator<Item = &Part> {
        self.parts.iter().filter(|p| p.is_field())
    }

    /// Parse `id` into `out` (cleared first). Returns false if the template does not match or a
    /// value overflows. Does not check that formatting gives back `id`; see `parse_exact`.
    pub fn parse<'a>(&self, id: &'a [u8], out: &mut Vec<Val<'a>>) -> bool {
        out.clear();
        if self.simple { self.parse_simple(id, out) } else { self.parse_regex(id, out) }
    }

    /// Parse and check that formatting gives back exactly `id` (the round-trip rule).
    pub fn parse_exact<'a>(&self, id: &'a [u8], out: &mut Vec<Val<'a>>, scratch: &mut Vec<u8>) -> bool {
        if !self.parse(id, out) {
            return false;
        }
        scratch.clear();
        self.format(out, scratch);
        scratch.as_slice() == id
    }

    pub fn parse_regex<'a>(&self, id: &'a [u8], out: &mut Vec<Val<'a>>) -> bool {
        out.clear();
        let mut locs = self.re.capture_locations();
        if self.re.captures_read(&mut locs, id).is_none() {
            return false;
        }
        let mut g = 1;
        for p in &self.parts {
            if !p.is_field() {
                continue;
            }
            let (s, e) = locs.get(g).unwrap();
            g += 1;
            let text = &id[s..e];
            let v = match p {
                Part::Str => Val::Str(text),
                Part::Dec | Part::DecW(_) => match parse_dec(text) {
                    Some(v) => Val::Int(v),
                    None => return false,
                },
                Part::HexL(_) | Part::HexU(_) => Val::Int(parse_hex(text)),
                Part::Lit(_) => unreachable!(),
            };
            out.push(v);
        }
        true
    }

    fn parse_simple<'a>(&self, id: &'a [u8], out: &mut Vec<Val<'a>>) -> bool {
        out.clear();
        let mut pos = 0;
        for (i, p) in self.parts.iter().enumerate() {
            let rest = &id[pos..];
            match p {
                Part::Lit(l) => {
                    if !rest.starts_with(l) {
                        return false;
                    }
                    pos += l.len();
                }
                Part::Dec => {
                    let run = rest.iter().take_while(|b| b.is_ascii_digit()).count();
                    if run == 0 {
                        return false;
                    }
                    let take = if rest[0] == b'0' { 1 } else { run };
                    match parse_dec(&rest[..take]) {
                        Some(v) => out.push(Val::Int(v)),
                        None => return false,
                    }
                    pos += take;
                }
                Part::DecW(w) => {
                    let w = *w as usize;
                    if rest.len() < w || !rest[..w].iter().all(|b| b.is_ascii_digit()) {
                        return false;
                    }
                    match parse_dec(&rest[..w]) {
                        Some(v) => out.push(Val::Int(v)),
                        None => return false,
                    }
                    pos += w;
                }
                Part::HexL(w) | Part::HexU(w) => {
                    let w = *w as usize;
                    let upper = matches!(p, Part::HexU(_));
                    if rest.len() < w || !rest[..w].iter().all(|&b| is_hex(b, upper)) {
                        return false;
                    }
                    out.push(Val::Int(parse_hex(&rest[..w])));
                    pos += w;
                }
                Part::Str => {
                    // Everything after STR is fixed width (checked by is_simple).
                    let suffix: usize = self.parts[i + 1..].iter().map(|p| p.fixed_len().unwrap()).sum();
                    if id.len() < pos + suffix {
                        return false;
                    }
                    let end = id.len() - suffix;
                    out.push(Val::Str(&id[pos..end]));
                    pos = end;
                }
            }
        }
        pos == id.len()
    }

    /// Append the formatted ID to `out`.
    pub fn format(&self, vals: &[Val], out: &mut Vec<u8>) {
        let mut k = 0;
        for p in &self.parts {
            match p {
                Part::Lit(l) => out.extend_from_slice(l),
                _ => {
                    let v = vals[k];
                    k += 1;
                    match (p, v) {
                        (Part::Str, Val::Str(s)) => out.extend_from_slice(s),
                        (Part::Dec, Val::Int(x)) => write_dec(x, 0, out),
                        (Part::DecW(w), Val::Int(x)) => write_dec(x, *w as usize, out),
                        (Part::HexL(w), Val::Int(x)) => write_hex(x, *w as usize, false, out),
                        (Part::HexU(w), Val::Int(x)) => write_hex(x, *w as usize, true, out),
                        _ => panic!("value kind does not match template field"),
                    }
                }
            }
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = crate::bits::W::default();
        w.u16(self.parts.len() as u16);
        for p in &self.parts {
            match p {
                Part::Lit(l) => {
                    w.u8(KIND_LIT).u8(0).u16(l.len() as u16).bytes(l);
                }
                Part::Dec => {
                    w.u8(KIND_DEC).u8(0).u16(0);
                }
                Part::DecW(x) => {
                    w.u8(KIND_DECW).u8(*x).u16(0);
                }
                Part::HexL(x) => {
                    w.u8(KIND_HEXL).u8(*x).u16(0);
                }
                Part::HexU(x) => {
                    w.u8(KIND_HEXU).u8(*x).u16(0);
                }
                Part::Str => {
                    w.u8(KIND_STR).u8(0).u16(0);
                }
            }
        }
        w.0
    }

    /// Decode a template at `b[*pos..]`, advancing `pos` past it (not past padding).
    pub fn from_bytes(b: &[u8], pos: &mut usize) -> Result<Template> {
        let corrupt = || Error::Corrupt("truncated template".into());
        let get = |p: usize, n: usize| b.get(p..p + n).ok_or_else(corrupt);
        let n_parts = u16::from_le_bytes(get(*pos, 2)?.try_into().unwrap());
        *pos += 2;
        let mut parts = Vec::with_capacity(n_parts as usize);
        for _ in 0..n_parts {
            let h = get(*pos, 4)?;
            let (kind, width, lit_len) = (h[0], h[1], u16::from_le_bytes([h[2], h[3]]) as usize);
            *pos += 4;
            parts.push(match kind {
                KIND_LIT => {
                    let l = get(*pos, lit_len)?.to_vec();
                    *pos += lit_len;
                    Part::Lit(l)
                }
                KIND_DEC => Part::Dec,
                KIND_DECW => Part::DecW(width),
                KIND_HEXL => Part::HexL(width),
                KIND_HEXU => Part::HexU(width),
                KIND_STR => Part::Str,
                k => return Err(Error::Unsupported(format!("unknown template part kind {k}"))),
            });
        }
        Template::new(parts)
    }
}

impl fmt::Display for Template {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for p in &self.parts {
            match p {
                Part::Lit(l) => {
                    for &b in l {
                        match b {
                            b'{' => f.write_str("{{")?,
                            b'}' => f.write_str("}}")?,
                            0x20..=0x7e => write!(f, "{}", b as char)?,
                            _ => write!(f, "\\x{b:02x}")?,
                        }
                    }
                }
                Part::Dec => f.write_str("{dec}")?,
                Part::DecW(w) => write!(f, "{{dec:{w}}}")?,
                Part::HexL(w) => write!(f, "{{hex:{w}}}")?,
                Part::HexU(w) => write!(f, "{{HEX:{w}}}")?,
                Part::Str => f.write_str("{str}")?,
            }
        }
        Ok(())
    }
}

fn regex_source(parts: &[Part]) -> String {
    let mut s = String::from("(?s-u)^");
    for p in parts {
        match p {
            Part::Lit(l) => {
                for &b in l {
                    if b.is_ascii_alphanumeric() {
                        s.push(b as char);
                    } else {
                        s.push_str(&format!("\\x{b:02x}"));
                    }
                }
            }
            Part::Dec => s.push_str("(0|[1-9][0-9]*)"),
            Part::DecW(w) => s.push_str(&format!("([0-9]{{{w}}})")),
            Part::HexL(w) => s.push_str(&format!("([0-9a-f]{{{w}}})")),
            Part::HexU(w) => s.push_str(&format!("([0-9A-F]{{{w}}})")),
            Part::Str => s.push_str("(.*)"),
        }
    }
    s.push('$');
    s
}

/// Whether the hand-written left-to-right parser is equivalent to leftmost-greedy regex matching:
/// every DEC is followed by the end or by a literal starting with a non-digit, and everything after
/// a STR is fixed width.
fn is_simple(parts: &[Part]) -> bool {
    for (i, p) in parts.iter().enumerate() {
        match p {
            Part::Dec => match parts.get(i + 1) {
                None => {}
                Some(Part::Lit(l)) if !l.is_empty() && !l[0].is_ascii_digit() => {}
                _ => return false,
            },
            Part::Str => {
                if parts[i + 1..].iter().any(|p| p.fixed_len().is_none()) {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

#[inline]
fn is_hex(b: u8, upper: bool) -> bool {
    b.is_ascii_digit() || if upper { (b'A'..=b'F').contains(&b) } else { (b'a'..=b'f').contains(&b) }
}

#[inline]
fn parse_dec(text: &[u8]) -> Option<u64> {
    if text.len() > 20 {
        return None;
    }
    let mut v: u64 = 0;
    for &b in text {
        v = v.checked_mul(10)?.checked_add((b - b'0') as u64)?;
    }
    Some(v)
}

#[inline]
fn parse_hex(text: &[u8]) -> u64 {
    let mut v: u64 = 0;
    for &b in text {
        let d = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            _ => b - b'A' + 10,
        };
        v = (v << 4) | d as u64;
    }
    v
}

#[inline]
pub fn write_dec(mut x: u64, width: usize, out: &mut Vec<u8>) {
    let mut buf = [0u8; 20];
    let mut i = 20;
    loop {
        i -= 1;
        buf[i] = b'0' + (x % 10) as u8;
        x /= 10;
        if x == 0 {
            break;
        }
    }
    let n = 20 - i;
    for _ in n..width {
        out.push(b'0');
    }
    out.extend_from_slice(&buf[i..]);
}

#[inline]
pub fn write_hex(x: u64, width: usize, upper: bool, out: &mut Vec<u8>) {
    let digits = if upper { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
    for k in (0..width).rev() {
        let d = if k >= 16 { 0 } else { (x >> (4 * k)) & 0xf };
        out.push(digits[d as usize]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(parts: Vec<Part>) -> Template {
        Template::new(parts).unwrap()
    }
    fn lit(s: &str) -> Part {
        Part::Lit(s.as_bytes().to_vec())
    }

    #[test]
    fn simple_matches_regex() {
        let templates = vec![
            t(vec![lit("D"), Part::Dec]),
            t(vec![Part::Dec]),
            t(vec![lit("doc-"), Part::DecW(5), lit("-"), Part::Dec]),
            t(vec![Part::HexL(8), lit("-"), Part::HexL(4)]),
            t(vec![lit("x"), Part::Str, lit(".txt")]),
            t(vec![Part::Str, Part::HexU(2)]),
            t(vec![Part::Dec, lit("a"), Part::Str, Part::DecW(2)]),
        ];
        let ids: Vec<&[u8]> = vec![
            b"D1", b"D0", b"D01", b"D", b"D18446744073709551615", b"D18446744073709551616", b"12",
            b"0", b"00", b"doc-00012-7", b"doc-00012-07", b"doc-0012-7", b"deadbeef-0a1b",
            b"DEADBEEF-0a1b", b"xabc.txt", b"x.txt", b"x.tx", b"hello1F", b"1F", b"F", b"5aqq07",
            b"5a07", b"5a7", b"",
        ];
        let mut a = Vec::new();
        let mut b = Vec::new();
        for tm in &templates {
            assert!(tm.simple, "{tm}");
            for id in &ids {
                let ra = tm.parse_simple(id, &mut a);
                let rb = tm.parse_regex(id, &mut b);
                assert_eq!(ra, rb, "{tm} {:?}", String::from_utf8_lossy(id));
                if ra {
                    assert_eq!(a, b, "{tm} {:?}", String::from_utf8_lossy(id));
                }
            }
        }
    }

    #[test]
    fn roundtrip_rule() {
        let tm = t(vec![lit("D"), Part::Dec]);
        let (mut v, mut s) = (Vec::new(), Vec::new());
        assert!(tm.parse_exact(b"D123", &mut v, &mut s));
        assert!(!tm.parse_exact(b"D0123", &mut v, &mut s));
        let tm = t(vec![Part::DecW(3)]);
        assert!(tm.parse_exact(b"007", &mut v, &mut s));
        let tm = t(vec![Part::HexL(4)]);
        assert!(tm.parse_exact(b"00af", &mut v, &mut s));
        assert!(!tm.parse_exact(b"00AF", &mut v, &mut s));
    }

    #[test]
    fn serialization() {
        let tm = t(vec![lit("a{b}\x01"), Part::DecW(3), Part::Str, Part::HexU(4)]);
        let bytes = tm.to_bytes();
        let mut pos = 0;
        let back = Template::from_bytes(&bytes, &mut pos).unwrap();
        assert_eq!(pos, bytes.len());
        assert_eq!(back, tm);
        assert_eq!(tm.to_string(), "a{{b}}\\x01{dec:3}{str}{HEX:4}");
    }
}
