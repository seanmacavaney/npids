//! Randomized round-trip tests: build, append, crash, compact, and check every lookup.

use npids::{Options, Reader, Writer, compact};
use std::collections::HashMap;
use std::path::Path;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = npids::phf::fmix64(self.0.wrapping_add(0x9e37_79b9_7f4a_7c15));
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn datasets(rng: &mut Rng) -> Vec<(&'static str, Vec<Vec<u8>>)> {
    let n = 3000;
    let mut out: Vec<(&'static str, Vec<Vec<u8>>)> = Vec::new();
    out.push(("seq", (0..n).map(|i| i.to_string().into_bytes()).collect()));
    out.push(("seq_from_5", (5..n + 5).map(|i| format!("D{i}").into_bytes()).collect()));
    let mut shuffled: Vec<u64> = (0..n).collect();
    for i in (1..shuffled.len()).rev() {
        shuffled.swap(i, rng.below(i as u64 + 1) as usize);
    }
    out.push(("shuffled", shuffled.iter().map(|i| format!("D{i}").into_bytes()).collect()));
    out.push(("sorted_sparse", {
        let mut v: Vec<u64> = (0..n).map(|_| rng.below(1 << 40)).collect();
        v.sort();
        v.dedup();
        v.iter().map(|i| format!("doc{i}").into_bytes()).collect()
    }));
    out.push(("step3", (0..n).map(|i| format!("s{}", i * 3 + 1).into_bytes()).collect()));
    out.push(("const_then_seq", (0..n).map(|i| format!("v7-{i}").into_bytes()).collect()));
    out.push(("padded", (0..n).map(|i| format!("x-{i:06}").into_bytes()).collect()));
    out.push(("uuid", (0..n).map(|_| {
        let (a, b) = (rng.next(), rng.next());
        format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", a >> 32, a & 0xffff, (a >> 16) & 0xffff, b >> 48, b & 0xffff_ffff_ffff).into_bytes()
    }).collect()));
    out.push(("random_bytes", (0..n).map(|_| {
        let len = rng.below(12) as usize;
        (0..len).map(|_| rng.below(256) as u8).collect()
    }).collect()));
    out.push(("with_outliers", (0..n).map(|i| {
        if rng.below(500) == 0 { vec![b'x'; 200] } else { format!("doc-{i:x}z").into_bytes() }
    }).collect()));
    out.push(("duplicates", (0..n).map(|_| format!("d{}", rng.below(300)).into_bytes()).collect()));
    out.push(("nul_and_empty", (0..n).map(|i| match i % 4 {
        0 => b"".to_vec(),
        1 => b"ab\x00".to_vec(),
        2 => format!("ab\x00{i}").into_bytes(),
        _ => format!("{i}\x00").into_bytes(),
    }).collect()));
    out.push(("mixed_prefixes", (0..n).map(|i| match i / 700 {
        0 => format!("FBIS{i}"),
        1 => format!("{i}"),
        2 => format!("LA{:06}", i),
        _ => format!("FT-{}-{}", i % 7, i),
    }.into_bytes()).collect()));
    // ClueWeb22-style: the last field counts up within a run and restarts at 0 when the middle one
    // increments (RUNS columns sharing one run table; increasing, so DERIVED).
    out.push(("cw22", runs_ids(rng, n, 20, 200, |a, b, i| format!("clueweb22-en{a:04}-{b:02}-{i:05}"))));
    // Long runs, so that chunks of 2^7 hold a single run and others hold a restart.
    out.push(("cw22_long", runs_ids(rng, n, 100, 400, |a, b, i| format!("clueweb22-en{a:04}-{b:02}-{i:05}"))));
    // Runs counting down by 3, then a restart: not increasing (PHF).
    out.push(("runs_down", runs_ids(rng, n, 10, 120, |a, b, i| format!("r{a}.{b}.{}", 100_000 - 3 * i))));
    // Two fields whose breakpoints don't nest (separate run tables).
    out.push(("runs_two_tables", (0..n).map(|i| format!("t{}-{}", (i / 37) * 2 + 100, i % 50 + 7).into_bytes()).collect()));
    // Runs interrupted by stretches of random values (chunks with and without run tables).
    out.push(("runs_and_noise", (0..n).map(|i| {
        if (i / 500) % 2 == 1 { format!("c-{:02}-{:05}", rng.below(100), rng.below(100_000)) } else { format!("c-{:02}-{:05}", i / 97, i % 97) }
    }.into_bytes()).collect()));
    out
}

/// `n` IDs in runs of `min..max` (random) positions; `f(a, b, i)` formats position `i` of a run, where
/// `b` counts runs and wraps at 48, incrementing `a`.
fn runs_ids(rng: &mut Rng, n: u64, min: u64, max: u64, f: impl Fn(u64, u64, u64) -> String) -> Vec<Vec<u8>> {
    let (mut a, mut b) = (0, 0);
    let mut out = Vec::new();
    while (out.len() as u64) < n {
        let len = min + rng.below(max - min);
        for i in 0..len.min(n - out.len() as u64) {
            out.push(f(a, b, i).into_bytes());
        }
        b += 1;
        if b == 48 {
            (a, b) = (a + 1, 0);
        }
    }
    out
}

fn check(path: &Path, ids: &[Vec<u8>]) {
    let r = Reader::open(path).unwrap();
    assert_eq!(r.len(), ids.len() as u64);
    let mut first: HashMap<&[u8], u64> = HashMap::new();
    let mut buf = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        first.entry(id.as_slice()).or_insert(i as u64);
        buf.clear();
        r.get(i as u64, &mut buf).unwrap();
        assert_eq!(&buf, id, "fwd {i}");
    }
    for (id, &i) in &first {
        assert_eq!(r.find(id).unwrap(), Some(i), "inv {:?}", String::from_utf8_lossy(id));
    }
    for absent in [&b"absent"[..], b"D99999999", b"", b"\xff\xfe", b"s2", b"s0", b"s9001", b"v8-1", b"v7-3000", b"0", b"3000", b"D4", b"D3005",
        b"clueweb22-en0000-00-99999", b"clueweb22-en0000-00-00000x", b"clueweb22-en0000-99-00000", b"clueweb22-en0001-00-00000",
        b"clueweb22-en9999-00-00000", b"clueweb22-en0000-01-00199", b"t100-6", b"t100-57", b"c-00-00097", b"r0.0.100001", b"r0.0.99998"] {
        if !first.contains_key(absent) {
            assert_eq!(r.find(absent).unwrap(), None);
        }
    }
    assert!(r.get(ids.len() as u64, &mut buf).is_err());
    r.verify(true).unwrap();
}

fn opts(bits: u8) -> Options {
    Options { chunk_bits: bits, ..Default::default() }
}

#[test]
fn build_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut rng = Rng(1);
    for (name, ids) in datasets(&mut rng) {
        for bits in [4, 7, 16] {
            let path = dir.path().join(format!("{name}-{bits}"));
            let mut w = Writer::open(&path, opts(bits)).unwrap();
            for id in &ids {
                w.add(id).unwrap();
            }
            w.finish().unwrap();
            check(&path, &ids);
        }
    }
}

#[test]
fn appends() {
    let dir = tempfile::tempdir().unwrap();
    let mut rng = Rng(2);
    for (name, ids) in datasets(&mut rng) {
        for bits in [4, 7] {
            let path = dir.path().join(format!("{name}-{bits}"));
            let mut i = 0;
            let mut sessions = 0;
            while i < ids.len() {
                let n = (rng.below(400) + 1) as usize;
                let mut w = Writer::open(&path, opts(bits)).unwrap();
                for id in &ids[i..(i + n).min(ids.len())] {
                    w.add(id).unwrap();
                }
                w.finish().unwrap();
                i = (i + n).min(ids.len());
                sessions += 1;
                check(&path, &ids[..i]);
            }
            assert!(sessions > 1);
            let before = std::fs::metadata(&path).unwrap().len();
            compact(&path, None).unwrap();
            check(&path, &ids);
            let r = Reader::open(&path).unwrap();
            assert!(r.manifest().units.len() <= 2, "{name}: {}", r.describe());
            assert!(std::fs::metadata(&path).unwrap().len() <= before);
        }
    }
}

#[test]
fn crash_and_build_inv_later() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x");
    let ids: Vec<Vec<u8>> = (0..1000).map(|i| format!("q{}", i * 7 % 1000).into_bytes()).collect();

    // Without inverse units, finds error rather than claiming "not found".
    let mut w = Writer::open(&path, Options { chunk_bits: 6, build_inv: false, meta: Some(b"{\"a\":1}".to_vec()) }).unwrap();
    for id in &ids[..500] {
        w.add(id).unwrap();
    }
    w.finish().unwrap();
    let r = Reader::open(&path).unwrap();
    assert!(r.find(b"nope").is_err());
    assert_eq!(r.meta(), b"{\"a\":1}");
    drop(r);

    // A writer dropped without finishing leaves the last commit intact.
    let mut w = Writer::open(&path, opts(6)).unwrap();
    for id in &ids[500..800] {
        w.add(id).unwrap();
    }
    drop(w);
    let r = Reader::open(&path).unwrap();
    assert_eq!(r.len(), 500);
    drop(r);

    // Simulate a torn tail: garbage after the last commit.
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(&[0xab; 1234]).unwrap();
    }
    assert_eq!(Reader::open(&path).unwrap().len(), 500);

    // The next session covers everything with inverse units.
    let mut w = Writer::open(&path, opts(6)).unwrap();
    for id in &ids[500..] {
        w.add(id).unwrap();
    }
    w.finish().unwrap();
    check(&path, &ids);
    assert_eq!(Reader::open(&path).unwrap().meta(), b"{\"a\":1}");
}

#[test]
fn reader_snapshot_survives_append() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x");
    let mut w = Writer::open(&path, opts(4)).unwrap();
    for i in 0..50 {
        w.add(format!("a{i}").as_bytes()).unwrap();
    }
    w.finish().unwrap();
    let old = Reader::open(&path).unwrap();
    let mut w = Writer::open(&path, opts(4)).unwrap();
    for i in 50..100 {
        w.add(format!("b{i}").as_bytes()).unwrap();
    }
    w.finish().unwrap();
    assert_eq!(old.len(), 50);
    let mut buf = Vec::new();
    for i in 0..50u64 {
        buf.clear();
        old.get(i, &mut buf).unwrap();
        assert_eq!(buf, format!("a{i}").as_bytes());
        assert_eq!(old.find(&buf).unwrap(), Some(i));
    }
    assert_eq!(Reader::open(&path).unwrap().len(), 100);
}

/// ClueWeb22-style IDs cost bytes per chunk, not bits per ID, and lookups stay correct.
#[test]
fn cw22_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cw22");
    let mut rng = Rng(3);
    let ids = runs_ids(&mut rng, 1 << 20, 5_000, 40_000, |a, b, i| format!("clueweb22-en{a:04}-{b:02}-{i:05}"));
    let mut w = Writer::open(&path, Options::default()).unwrap();
    for id in &ids {
        w.add(id).unwrap();
    }
    w.finish().unwrap();
    let size = std::fs::metadata(&path).unwrap().len();
    let r = Reader::open(&path).unwrap();
    assert!(size < 16 * 400, "{size} bytes\n{}", r.describe());
    assert_eq!(r.manifest().units.len(), 1, "{}", r.describe());
    let mut buf = Vec::new();
    for i in (0..ids.len()).step_by(7) {
        buf.clear();
        r.get(i as u64, &mut buf).unwrap();
        assert_eq!(buf, ids[i]);
        assert_eq!(r.find(&ids[i]).unwrap(), Some(i as u64));
    }
    // Every run boundary, from both sides.
    for i in 1..ids.len() {
        if !ids[i].ends_with(b"-00000") {
            continue;
        }
        for k in [i - 1, i] {
            assert_eq!(r.find(&ids[k]).unwrap(), Some(k as u64));
        }
        let mut past = ids[i - 1].clone();
        let l = past.len();
        let next: u64 = std::str::from_utf8(&past[l - 5..]).unwrap().parse::<u64>().unwrap() + 1;
        past.truncate(l - 5);
        past.extend_from_slice(format!("{next:05}").as_bytes());
        assert_eq!(r.find(&past).unwrap(), None, "{}", String::from_utf8_lossy(&past));
    }
}

/// Readers refuse a column encoding they do not know, rather than misreading it.
#[test]
fn unknown_encoding_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x");
    let mut w = Writer::open(&path, opts(4)).unwrap();
    for i in 0..40 {
        w.add(format!("d{}", i * 13 % 40).as_bytes()).unwrap();
    }
    w.finish().unwrap();
    let r = Reader::open(&path).unwrap();
    let off = r.manifest().chunks[1].offset as usize;
    drop(r);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[off + 8] = 200; // first field descriptor's `enc`
    std::fs::write(&path, &bytes).unwrap();
    let r = Reader::open(&path).unwrap();
    let mut buf = Vec::new();
    r.get(0, &mut buf).unwrap();
    match r.get(16, &mut buf) {
        Err(npids::Error::Unsupported(m)) => assert!(m.contains("unknown column encoding 200"), "{m}"),
        other => panic!("{other:?}"),
    }
}
