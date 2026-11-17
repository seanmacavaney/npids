//! Header, commit slots and manifest (FORMAT.md, "Header" and "Manifest").

use crate::bits::{W, read_u32, read_u64};
use crate::template::Template;
use crate::{Error, Result};

pub const MAGIC: &[u8; 8] = b"NPIDSv2\n";
pub const MANIFEST_MAGIC: &[u8; 8] = b"NPIDMNFT";
pub const MAJOR: u16 = 2;
pub const MINOR: u16 = 1;
pub const HEADER_LEN: usize = 80;
pub const SLOT_OFFSETS: [usize; 2] = [16, 48];
const MANIFEST_HEADER: usize = 80;
const CHUNK_ENTRY: usize = 32;
const UNIT_ENTRY: usize = 40;

pub const UNIT_DERIVED: u32 = 1;
pub const UNIT_PHF: u32 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    pub generation: u64,
    pub manifest_off: u64,
    pub manifest_len: u32,
    pub manifest_crc: u32,
}

impl Slot {
    pub fn encode(&self) -> [u8; 32] {
        let mut w = W::default();
        w.u64(self.generation).u64(self.manifest_off).u32(self.manifest_len).u32(self.manifest_crc);
        let crc = crc32c::crc32c(&w.0);
        w.u32(crc).u32(0);
        w.0.try_into().unwrap()
    }

    /// Decode a slot; None if it is empty or its checksum does not match.
    pub fn decode(b: &[u8]) -> Option<Slot> {
        let s = Slot {
            generation: read_u64(b, 0),
            manifest_off: read_u64(b, 8),
            manifest_len: read_u32(b, 16),
            manifest_crc: read_u32(b, 20),
        };
        if s.generation == 0 || crc32c::crc32c(&b[..24]) != read_u32(b, 24) {
            return None;
        }
        Some(s)
    }
}

pub fn encode_header(slots: &[Slot; 2]) -> Vec<u8> {
    let mut w = W::default();
    w.bytes(MAGIC).u16(MAJOR).u16(MINOR).u32(0);
    for s in slots {
        if s.generation == 0 { w.zeros(32) } else { w.bytes(&s.encode()) };
    }
    debug_assert_eq!(w.len(), HEADER_LEN);
    w.0
}

/// Parse the header and return its two slots (None where invalid).
pub fn decode_header(b: &[u8]) -> Result<[Option<Slot>; 2]> {
    if b.len() < HEADER_LEN || &b[..8] != MAGIC {
        return Err(Error::Unsupported("not an npids v2 file".into()));
    }
    let major = u16::from_le_bytes([b[8], b[9]]);
    if major != MAJOR {
        return Err(Error::Unsupported(format!(
            "file format version {major} is not supported by this version of npids; please upgrade npids"
        )));
    }
    Ok([Slot::decode(&b[16..48]), Slot::decode(&b[48..80])])
}

/// Pick the newest slot whose manifest is intact.
pub fn current_slot(file: &[u8]) -> Result<(usize, Slot)> {
    let slots = decode_header(file)?;
    let mut best: Option<(usize, Slot)> = None;
    for (i, s) in slots.iter().enumerate() {
        let Some(s) = s else { continue };
        let (off, len) = (s.manifest_off as usize, s.manifest_len as usize);
        let Some(m) = file.get(off..off.saturating_add(len)) else { continue };
        if crc32c::crc32c(m) != s.manifest_crc {
            continue;
        }
        if best.is_none_or(|(_, b)| s.generation > b.generation) {
            best = Some((i, *s));
        }
    }
    best.ok_or_else(|| Error::Corrupt("no valid commit found".into()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkEntry {
    pub offset: u64,
    pub len: u32,
    pub count: u32,
    pub template_id: u32,
    pub crc: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitEntry {
    pub lo: u64,
    pub hi: u64,
    pub offset: u64,
    pub len: u64,
    pub kind: u32,
    pub crc: u32,
}

#[derive(Clone, Debug)]
pub struct Manifest {
    pub generation: u64,
    pub total_count: u64,
    pub chunk_bits: u8,
    pub templates: Vec<Template>,
    pub chunks: Vec<ChunkEntry>,
    pub units: Vec<UnitEntry>,
    pub prev_manifest_off: u64,
    pub meta: Vec<u8>,
}

impl Manifest {
    pub fn empty(chunk_bits: u8) -> Manifest {
        Manifest {
            generation: 1,
            total_count: 0,
            chunk_bits,
            templates: Vec::new(),
            chunks: Vec::new(),
            units: Vec::new(),
            prev_manifest_off: 0,
            meta: Vec::new(),
        }
    }

    pub fn chunk_size(&self) -> u64 {
        1u64 << self.chunk_bits
    }

    /// Index of a template, adding it if it is new.
    pub fn intern(&mut self, t: &Template) -> u32 {
        match self.templates.iter().position(|x| x == t) {
            Some(i) => i as u32,
            None => {
                self.templates.push(t.clone());
                (self.templates.len() - 1) as u32
            }
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut body = W::default();
        let templates_off = MANIFEST_HEADER + body.len();
        for t in &self.templates {
            body.bytes(&t.to_bytes()).pad8();
        }
        let chunks_off = MANIFEST_HEADER + body.len();
        for c in &self.chunks {
            body.u64(c.offset).u32(c.len).u32(c.count).u32(c.template_id).u32(c.crc).u64(0);
        }
        let units_off = MANIFEST_HEADER + body.len();
        for u in &self.units {
            body.u64(u.lo).u64(u.hi).u64(u.offset).u64(u.len).u32(u.kind).u32(u.crc);
        }
        let meta_off = MANIFEST_HEADER + body.len();
        body.bytes(&self.meta).pad8();

        let mut w = W::default();
        w.bytes(MANIFEST_MAGIC)
            .u64(self.generation)
            .u64(self.total_count)
            .u8(self.chunk_bits)
            .zeros(3)
            .u32(self.templates.len() as u32)
            .u64(self.chunks.len() as u64)
            .u32(self.units.len() as u32)
            .u32(0)
            .u64(self.prev_manifest_off)
            .u32(templates_off as u32)
            .u32(chunks_off as u32)
            .u32(units_off as u32)
            .u32(meta_off as u32)
            .u32(self.meta.len() as u32)
            .u32(0);
        debug_assert_eq!(w.len(), MANIFEST_HEADER);
        w.bytes(&body.0);
        w.0
    }

    pub fn decode(b: &[u8]) -> Result<Manifest> {
        let corrupt = |m: &str| Error::Corrupt(format!("manifest: {m}"));
        if b.len() < MANIFEST_HEADER || &b[..8] != MANIFEST_MAGIC {
            return Err(corrupt("bad magic"));
        }
        let generation = read_u64(b, 8);
        let total_count = read_u64(b, 16);
        let chunk_bits = b[24];
        let n_templates = read_u32(b, 28) as usize;
        let n_chunks = read_u64(b, 32) as usize;
        let n_units = read_u32(b, 40) as usize;
        let prev_manifest_off = read_u64(b, 48);
        let templates_off = read_u32(b, 56) as usize;
        let chunks_off = read_u32(b, 60) as usize;
        let units_off = read_u32(b, 64) as usize;
        let meta_off = read_u32(b, 68) as usize;
        let meta_len = read_u32(b, 72) as usize;
        if chunk_bits == 0 || chunk_bits > 32 {
            return Err(corrupt("chunk_bits out of range"));
        }
        if n_chunks as u64 != total_count.div_ceil(1u64 << chunk_bits) {
            return Err(corrupt("chunk count does not match total count"));
        }
        let mut templates = Vec::with_capacity(n_templates);
        let mut pos = templates_off;
        for _ in 0..n_templates {
            templates.push(Template::from_bytes(b, &mut pos)?);
            pos = pos.next_multiple_of(8);
        }
        let section = |off: usize, n: usize, size: usize| {
            b.get(off..off + n * size).ok_or_else(|| corrupt("section out of bounds"))
        };
        let cs = section(chunks_off, n_chunks, CHUNK_ENTRY)?;
        let chunks: Vec<ChunkEntry> = (0..n_chunks)
            .map(|i| {
                let e = &cs[i * CHUNK_ENTRY..];
                ChunkEntry {
                    offset: read_u64(e, 0),
                    len: read_u32(e, 8),
                    count: read_u32(e, 12),
                    template_id: read_u32(e, 16),
                    crc: read_u32(e, 20),
                }
            })
            .collect();
        let us = section(units_off, n_units, UNIT_ENTRY)?;
        let units: Vec<UnitEntry> = (0..n_units)
            .map(|i| {
                let e = &us[i * UNIT_ENTRY..];
                UnitEntry {
                    lo: read_u64(e, 0),
                    hi: read_u64(e, 8),
                    offset: read_u64(e, 16),
                    len: read_u64(e, 24),
                    kind: read_u32(e, 32),
                    crc: read_u32(e, 36),
                }
            })
            .collect();
        let meta = section(meta_off, meta_len, 1)?.to_vec();

        let c = 1u64 << chunk_bits;
        for (i, e) in chunks.iter().enumerate() {
            let expect = if i + 1 < n_chunks { c } else { total_count - c * i as u64 };
            if e.count as u64 != expect {
                return Err(corrupt("chunk count is not the chunk size"));
            }
            if e.template_id as usize >= templates.len() {
                return Err(corrupt("bad template id"));
            }
        }
        let mut prev_hi = 0;
        for u in &units {
            if u.lo < prev_hi || u.lo >= u.hi || u.hi > total_count {
                return Err(corrupt("inverse units overlap or are out of range"));
            }
            prev_hi = u.hi;
        }
        Ok(Manifest { generation, total_count, chunk_bits, templates, chunks, units, prev_manifest_off, meta })
    }
}
