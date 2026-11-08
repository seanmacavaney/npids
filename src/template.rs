//! ID templates (FORMAT.md, "Templates").

use crate::{Error, Result};
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
    /// A decimal number, zero-padded to at least this many digits (0: no leading zeros).
    Dec(u8),
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
        matches!(self, Part::Dec(_) | Part::DecW(_) | Part::HexL(_) | Part::HexU(_))
    }
    /// Length in bytes if the part always matches a fixed number of bytes.
    pub fn fixed_len(&self) -> Option<usize> {
        match self {
            Part::Lit(l) => Some(l.len()),
            Part::DecW(w) | Part::HexL(w) | Part::HexU(w) => Some(*w as usize),
            Part::Dec(_) | Part::Str => None,
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
    /// Check the validity rules of FORMAT.md ("Templates").
    pub fn new(parts: Vec<Part>) -> Result<Template> {
        let invalid = |m: &str| Err(Error::Invalid(format!("invalid template: {m}")));
        let mut seen_str = false;
        for (i, p) in parts.iter().enumerate() {
            let next = parts.get(i + 1);
            match p {
                Part::Lit(l) => {
                    if l.is_empty() || l.len() > u16::MAX as usize {
                        return invalid("literal must have 1 to 65535 bytes");
                    }
                    if matches!(next, Some(Part::Lit(_))) {
                        return invalid("adjacent literals");
                    }
                }
                Part::Dec(w) if *w > 20 => return invalid("DEC width out of range"),
                Part::Dec(_) => match next {
                    None => {}
                    Some(Part::Lit(l)) if !l[0].is_ascii_digit() => {}
                    _ => return invalid("DEC must be last or followed by a literal starting with a non-digit"),
                },
                Part::DecW(w) if *w == 0 || *w > 20 => return invalid("DECW width out of range"),
                Part::HexL(w) | Part::HexU(w) if *w == 0 || *w > 16 => return invalid("HEX width out of range"),
                Part::Str => {
                    if seen_str {
                        return invalid("more than one STR");
                    }
                    seen_str = true;
                    if parts[i + 1..].iter().any(|p| p.fixed_len().is_none()) {
                        return invalid("parts after STR must be fixed width");
                    }
                }
                _ => {}
            }
        }
        let n_fields = parts.iter().filter(|p| p.is_field()).count();
        Ok(Template { parts, n_fields })
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
                Part::Dec(w) => {
                    let run = rest.iter().take_while(|b| b.is_ascii_digit()).count();
                    if run == 0 {
                        return false;
                    }
                    let take = if *w == 0 && rest[0] == b'0' { 1 } else { run };
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

    /// Parse and check that formatting gives back exactly `id` (the round-trip rule).
    pub fn parse_exact<'a>(&self, id: &'a [u8], out: &mut Vec<Val<'a>>, scratch: &mut Vec<u8>) -> bool {
        if !self.parse(id, out) {
            return false;
        }
        scratch.clear();
        self.format(out, scratch);
        scratch.as_slice() == id
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
                        (Part::Dec(w), Val::Int(x)) => write_dec(x, *w as usize, out),
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
                Part::Dec(x) => {
                    w.u8(KIND_DEC).u8(*x).u16(0);
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
                KIND_DEC => Part::Dec(width),
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
                Part::Dec(0) => f.write_str("{dec}")?,
                Part::Dec(w) => write!(f, "{{dec:{w}+}}")?,
                Part::DecW(w) => write!(f, "{{dec:{w}}}")?,
                Part::HexL(w) => write!(f, "{{hex:{w}}}")?,
                Part::HexU(w) => write!(f, "{{HEX:{w}}}")?,
                Part::Str => f.write_str("{str}")?,
            }
        }
        Ok(())
    }
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
    let mut buf = [0u8; 16];
    let width = width.min(16);
    for (k, b) in buf[..width].iter_mut().enumerate() {
        *b = digits[((x >> (4 * (width - 1 - k))) & 0xf) as usize];
    }
    out.extend_from_slice(&buf[..width]);
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

    fn parse(t: &Template, id: &str) -> Option<Vec<String>> {
        let mut v = Vec::new();
        t.parse(id.as_bytes(), &mut v).then(|| {
            v.iter()
                .map(|x| match x {
                    Val::Int(i) => i.to_string(),
                    Val::Str(s) => format!("'{}'", String::from_utf8_lossy(s)),
                })
                .collect()
        })
    }

    #[test]
    fn parsing() {
        let d = t(vec![lit("D"), Part::Dec(0)]);
        assert_eq!(parse(&d, "D1"), Some(vec!["1".into()]));
        assert_eq!(parse(&d, "D0"), Some(vec!["0".into()]));
        assert_eq!(parse(&d, "D01"), None);
        assert_eq!(parse(&d, "D"), None);
        assert_eq!(parse(&d, "D18446744073709551615"), Some(vec!["18446744073709551615".into()]));
        assert_eq!(parse(&d, "D18446744073709551616"), None);
        let x = t(vec![lit("doc-"), Part::DecW(5), lit("-"), Part::Dec(0)]);
        assert_eq!(parse(&x, "doc-00012-7"), Some(vec!["12".into(), "7".into()]));
        assert_eq!(parse(&x, "doc-0012-7"), None);
        let h = t(vec![Part::HexL(8), lit("-"), Part::HexL(4)]);
        assert_eq!(parse(&h, "deadbeef-0a1b"), Some(vec![0xdeadbeefu64.to_string(), 0x0a1b.to_string()]));
        assert_eq!(parse(&h, "DEADBEEF-0a1b"), None);
        let s = t(vec![lit("x"), Part::Str, lit(".txt")]);
        assert_eq!(parse(&s, "xabc.txt"), Some(vec!["'abc'".into()]));
        assert_eq!(parse(&s, "x.txt"), Some(vec!["''".into()]));
        assert_eq!(parse(&s, "x.tx"), None);
        let m = t(vec![Part::Dec(0), lit("a"), Part::Str, Part::DecW(2)]);
        assert_eq!(parse(&m, "5aqq07"), Some(vec!["5".into(), "'qq'".into(), "7".into()]));
        assert_eq!(parse(&m, "5a7"), None);
        // A minimum width takes the whole digit run.
        let p = t(vec![lit("GX-"), Part::Dec(7)]);
        assert_eq!(parse(&p, "GX-0000949"), Some(vec!["949".into()]));
        assert_eq!(parse(&p, "GX-11579329"), Some(vec!["11579329".into()]));
        assert_eq!(p.to_string(), "GX-{dec:7+}");
    }

    #[test]
    fn validity() {
        assert!(Template::new(vec![Part::Dec(0), Part::Dec(0)]).is_err());
        assert!(Template::new(vec![Part::Dec(0), lit("1")]).is_err());
        assert!(Template::new(vec![Part::Dec(0), Part::Str]).is_err());
        assert!(Template::new(vec![Part::Str, Part::Dec(0)]).is_err());
        assert!(Template::new(vec![Part::Str, lit("a"), Part::Str]).is_err());
        assert!(Template::new(vec![lit("a"), lit("b")]).is_err());
        assert!(Template::new(vec![lit("")]).is_err());
        assert!(Template::new(vec![Part::HexL(17)]).is_err());
        assert!(Template::new(vec![Part::Dec(21)]).is_err());
        assert!(Template::new(vec![Part::Dec(3), lit("1")]).is_err());
        assert!(Template::new(vec![Part::Dec(3), lit("-"), Part::Dec(2)]).is_ok());
        assert!(Template::new(vec![Part::Dec(0), lit("-"), Part::Str, Part::HexL(4)]).is_ok());
    }

    #[test]
    fn roundtrip_rule() {
        let tm = t(vec![lit("D"), Part::Dec(0)]);
        let (mut v, mut s) = (Vec::new(), Vec::new());
        assert!(tm.parse_exact(b"D123", &mut v, &mut s));
        assert!(!tm.parse_exact(b"D0123", &mut v, &mut s));
        let tm = t(vec![Part::DecW(3)]);
        assert!(tm.parse_exact(b"007", &mut v, &mut s));
        let tm = t(vec![Part::Dec(3)]);
        assert!(tm.parse_exact(b"007", &mut v, &mut s));
        assert!(tm.parse_exact(b"1234", &mut v, &mut s));
        assert!(!tm.parse_exact(b"7", &mut v, &mut s));
        assert!(!tm.parse_exact(b"0007", &mut v, &mut s));
        let tm = t(vec![Part::HexL(4)]);
        assert!(tm.parse_exact(b"00af", &mut v, &mut s));
        assert!(!tm.parse_exact(b"00AF", &mut v, &mut s));
    }

    #[test]
    fn serialization() {
        let tm = t(vec![lit("a{b}\x01"), Part::Dec(4), lit("-"), Part::DecW(3), Part::Str, Part::HexU(4)]);
        let bytes = tm.to_bytes();
        let mut pos = 0;
        let back = Template::from_bytes(&bytes, &mut pos).unwrap();
        assert_eq!(pos, bytes.len());
        assert_eq!(back, tm);
        assert_eq!(tm.to_string(), "a{{b}}\\x01{dec:4+}-{dec:3}{str}{HEX:4}");
    }
}
