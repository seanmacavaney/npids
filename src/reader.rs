//! Reading v2 files through a memory map.

use crate::chunk::{ChunkInfo, ENC_PLAIN};
use crate::format::{self, Manifest, UNIT_DERIVED, UNIT_PHF};
use crate::phf::PhfView;
use crate::template::Val;
use crate::{Error, Result};
use memmap2::Mmap;
use std::cmp::Ordering;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub(crate) enum UnitKind {
    Derived { template_id: u32 },
    Phf(PhfView),
}

pub(crate) struct Unit {
    pub lo: u64,
    pub hi: u64,
    pub kind: UnitKind,
}

pub struct Reader {
    path: PathBuf,
    mmap: Mmap,
    manifest: Manifest,
    chunks: Vec<OnceLock<ChunkInfo>>,
    units: Vec<Unit>,
}

impl Reader {
    pub fn open(path: impl AsRef<Path>) -> Result<Reader> {
        let path = path.as_ref();
        let f = File::open(path)?;
        // SAFETY: committed regions of an npids file are never modified (FORMAT.md, "Layout").
        let mmap = unsafe { Mmap::map(&f)? };
        let (_, slot) = format::current_slot(&mmap)?;
        let off = slot.manifest_off as usize;
        let manifest = Manifest::decode(&mmap[off..off + slot.manifest_len as usize])?;
        Reader::from_parts(path.to_path_buf(), mmap, manifest)
    }

    pub(crate) fn from_parts(path: PathBuf, mmap: Mmap, manifest: Manifest) -> Result<Reader> {
        let mut units = Vec::with_capacity(manifest.units.len());
        for u in &manifest.units {
            let kind = match u.kind {
                UNIT_DERIVED => {
                    let p = mmap
                        .get(u.offset as usize..(u.offset + u.len) as usize)
                        .filter(|p| p.len() >= 8)
                        .ok_or_else(|| Error::Corrupt("DERIVED unit out of bounds".into()))?;
                    let template_id = crate::bits::read_u32(p, 0);
                    if template_id as usize >= manifest.templates.len() {
                        return Err(Error::Corrupt("DERIVED unit has a bad template id".into()));
                    }
                    UnitKind::Derived { template_id }
                }
                UNIT_PHF => UnitKind::Phf(PhfView::read(&mmap, u.offset, u.len)?),
                k => return Err(Error::Unsupported(format!("unknown inverse unit kind {k}"))),
            };
            units.push(Unit { lo: u.lo, hi: u.hi, kind });
        }
        let chunks = (0..manifest.chunks.len()).map(|_| OnceLock::new()).collect();
        Ok(Reader { path, mmap, manifest, chunks, units })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> u64 {
        self.manifest.total_count
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn meta(&self) -> &[u8] {
        &self.manifest.meta
    }

    pub(crate) fn file(&self) -> &[u8] {
        &self.mmap
    }

    /// Number of indices reachable by inverse lookups.
    pub fn inverse_coverage(&self) -> u64 {
        self.units.iter().map(|u| u.hi - u.lo).sum()
    }

    pub(crate) fn chunk(&self, ci: usize) -> Result<&ChunkInfo> {
        let cell = &self.chunks[ci];
        if let Some(c) = cell.get() {
            return Ok(c);
        }
        let e = &self.manifest.chunks[ci];
        let info = ChunkInfo::read(&self.mmap, e, &self.manifest.templates[e.template_id as usize])?;
        Ok(cell.get_or_init(|| info))
    }

    /// Append the ID at `idx` to `out`.
    #[inline]
    pub fn get(&self, idx: u64, out: &mut Vec<u8>) -> Result<()> {
        if idx >= self.manifest.total_count {
            return Err(Error::OutOfRange(idx));
        }
        let ci = (idx >> self.manifest.chunk_bits) as usize;
        let j = (idx & (self.manifest.chunk_size() - 1)) as usize;
        let info = self.chunk(ci)?;
        info.decode(&self.mmap, &self.manifest.templates[info.template_id as usize], j, out);
        Ok(())
    }

    /// The first index of `id`. Errors with `NoInverse` if `id` was not found and some indices are not
    /// covered by an inverse unit.
    pub fn find(&self, id: &[u8]) -> Result<Option<u64>> {
        let mut buf = Vec::new();
        for u in &self.units {
            let hit = match &u.kind {
                UnitKind::Phf(view) => match view.candidate(&self.mmap, id) {
                    Some(local) if local < u.hi - u.lo => {
                        buf.clear();
                        self.get(u.lo + local, &mut buf)?;
                        (buf == id).then_some(u.lo + local)
                    }
                    _ => None,
                },
                UnitKind::Derived { template_id } => self.find_derived(u, *template_id, id, &mut buf)?,
            };
            if hit.is_some() {
                return Ok(hit);
            }
        }
        if self.inverse_coverage() < self.len() {
            return Err(Error::NoInverse);
        }
        Ok(None)
    }

    fn find_derived(&self, u: &Unit, tid: u32, id: &[u8], buf: &mut Vec<u8>) -> Result<Option<u64>> {
        let t = &self.manifest.templates[tid as usize];
        let mut q = Vec::new();
        if !t.parse(id, &mut q) {
            return Ok(None);
        }
        let (mut lo, mut hi) = (u.lo, u.hi);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match self.cmp_key(mid, tid, &q, buf)? {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => {
                    buf.clear();
                    self.get(mid, buf)?;
                    return Ok((buf.as_slice() == id).then_some(mid));
                }
            }
        }
        Ok(None)
    }

    /// Compare the key at `idx` (under template `tid`) with `q`.
    fn cmp_key(&self, idx: u64, tid: u32, q: &[Val], buf: &mut Vec<u8>) -> Result<Ordering> {
        let ci = (idx >> self.manifest.chunk_bits) as usize;
        let j = (idx & (self.manifest.chunk_size() - 1)) as usize;
        let info = self.chunk(ci)?;
        if info.template_id == tid {
            for (k, f) in info.fields.iter().enumerate() {
                let v = if f.enc == ENC_PLAIN { Val::Str(info.str(&self.mmap, k, j)) } else { Val::Int(info.int(&self.mmap, k, j)) };
                match v.cmp(&q[k]) {
                    Ordering::Equal => continue,
                    o => return Ok(o),
                }
            }
            Ok(Ordering::Equal)
        } else {
            buf.clear();
            info.decode(&self.mmap, &self.manifest.templates[info.template_id as usize], j, buf);
            let mut key = Vec::new();
            if !self.manifest.templates[tid as usize].parse(buf, &mut key) {
                return Err(Error::Corrupt(format!("ID at {idx} does not match its DERIVED unit's template")));
            }
            Ok(key.as_slice().cmp(q))
        }
    }

    /// Check checksums. With `deep`, also check that every ID decodes and every covered ID is found
    /// at its first index.
    pub fn verify(&self, deep: bool) -> Result<()> {
        let file: &[u8] = &self.mmap;
        for (i, e) in self.manifest.chunks.iter().enumerate() {
            let p = &file[e.offset as usize..e.offset as usize + e.len as usize];
            if crc32c::crc32c(p) != e.crc {
                return Err(Error::Corrupt(format!("chunk {i} checksum mismatch")));
            }
            self.chunk(i)?;
        }
        for (i, u) in self.manifest.units.iter().enumerate() {
            let p = &file[u.offset as usize..(u.offset + u.len) as usize];
            if crc32c::crc32c(p) != u.crc {
                return Err(Error::Corrupt(format!("inverse unit {i} checksum mismatch")));
            }
        }
        if deep {
            let mut buf = Vec::new();
            let mut buf2 = Vec::new();
            for u in &self.units {
                for idx in u.lo..u.hi {
                    buf.clear();
                    self.get(idx, &mut buf)?;
                    let found = self.find(&buf)?.ok_or_else(|| Error::Corrupt(format!("ID at {idx} not found")))?;
                    if found != idx {
                        buf2.clear();
                        self.get(found, &mut buf2)?;
                        if found > idx || buf2 != buf {
                            return Err(Error::Corrupt(format!("ID at {idx} found at {found}")));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn describe(&self) -> String {
        use std::fmt::Write;
        let m = &self.manifest;
        let mut s = String::new();
        let size = self.mmap.len();
        let _ = writeln!(
            s,
            "npids v2: {} ids, {} chunks of {}, {} bytes, generation {}",
            m.total_count, m.chunks.len(), m.chunk_size(), size, m.generation
        );
        let _ = writeln!(s, "  fwd");
        let mut i = 0;
        while i < m.chunks.len() {
            let t = m.chunks[i].template_id;
            let mut j = i;
            let (mut ids, mut bytes) = (0u64, 0u64);
            while j < m.chunks.len() && m.chunks[j].template_id == t {
                ids += m.chunks[j].count as u64;
                bytes += m.chunks[j].len as u64;
                j += 1;
            }
            let lo = i as u64 * m.chunk_size();
            let _ = writeln!(s, "    [{lo}, {}) {} ({} chunks, {bytes} bytes)", lo + ids, m.templates[t as usize], j - i);
            i = j;
        }
        let _ = writeln!(s, "  inv");
        if self.units.is_empty() {
            let _ = writeln!(s, "    (none)");
        }
        for (u, e) in self.units.iter().zip(&m.units) {
            let kind = match &u.kind {
                UnitKind::Derived { template_id } => format!("derived {}", m.templates[*template_id as usize]),
                UnitKind::Phf(_) => "phf".to_string(),
            };
            let _ = writeln!(s, "    [{}, {}) {kind} ({} bytes)", u.lo, u.hi, e.len);
        }
        s.pop();
        s
    }
}
