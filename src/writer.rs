//! Building and appending to v2 files (FORMAT.md, "Committing", "Appending", "Compaction").

use crate::chunk::{self, ChunkInfo};
use crate::format::{self, ChunkEntry, HEADER_LEN, Manifest, SLOT_OFFSETS, Slot, UNIT_DERIVED, UNIT_PHF, UnitEntry};
use crate::reader::Reader;
use crate::template::Val;
use crate::{Error, Result, infer, phf};
use memmap2::Mmap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

/// PHF units are capped at this many IDs to bound the memory needed to build them.
const MAX_PHF_UNIT: u64 = 1 << 27;

#[derive(Clone, Debug)]
pub struct Options {
    /// log2 of the chunk size, for new files (existing files keep theirs).
    pub chunk_bits: u8,
    /// Build inverse units for IDs not yet covered by one.
    pub build_inv: bool,
    /// Replace the file's metadata (JSON) if set.
    pub meta: Option<Vec<u8>>,
}

impl Default for Options {
    fn default() -> Self {
        Options { chunk_bits: 16, build_inv: true, meta: None }
    }
}

pub struct Writer {
    path: PathBuf,
    file: File,
    opts: Options,
    manifest: Manifest,
    base: Slot,
    slots: [Slot; 2],
    pos: u64,
    pending: Vec<u8>,
    pending_ends: Vec<usize>,
}

fn write_at(file: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let (mut buf, mut offset) = (buf, offset);
        while !buf.is_empty() {
            let n = file.seek_write(buf, offset)?;
            buf = &buf[n..];
            offset += n as u64;
        }
        Ok(())
    }
}

/// Open (creating if needed) and exclusively lock the file at `path`, retrying if the path was
/// replaced by compaction while we waited for the lock.
pub(crate) fn open_locked(path: &Path) -> Result<File> {
    loop {
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        file.lock()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let a = file.metadata()?;
            match fs::metadata(path) {
                Ok(b) if a.ino() == b.ino() && a.dev() == b.dev() => return Ok(file),
                _ => continue,
            }
        }
        #[cfg(not(unix))]
        return Ok(file);
    }
}

impl Writer {
    /// Create a new file or append to an existing v2 file.
    pub fn open(path: impl AsRef<Path>, opts: Options) -> Result<Writer> {
        if !(1..=32).contains(&opts.chunk_bits) {
            return Err(Error::Invalid("chunk_bits must be between 1 and 32".into()));
        }
        let path = path.as_ref().to_path_buf();
        let file = open_locked(&path)?;
        let len = file.metadata()?.len();
        let mut w = if len == 0 {
            // A new file starts with an empty commit, so readers never see a file without one.
            let manifest = Manifest::empty(opts.chunk_bits);
            let bytes = manifest.encode();
            let base = Slot {
                generation: manifest.generation,
                manifest_off: HEADER_LEN as u64,
                manifest_len: bytes.len() as u32,
                manifest_crc: crc32c::crc32c(&bytes),
            };
            write_at(&file, &format::encode_header(&[Slot::default(); 2]), 0)?;
            write_at(&file, &bytes, HEADER_LEN as u64)?;
            file.sync_all()?;
            write_at(&file, &base.encode(), SLOT_OFFSETS[0] as u64)?;
            file.sync_all()?;
            Writer {
                path,
                file,
                opts: opts.clone(),
                manifest,
                base,
                slots: [base, Slot::default()],
                pos: (HEADER_LEN + bytes.len()) as u64,
                pending: Vec::new(),
                pending_ends: Vec::new(),
            }
        } else {
            // SAFETY: we hold the lock, and only read committed regions.
            let mmap = unsafe { Mmap::map(&file)? };
            if mmap.len() < 8 || &mmap[..8] != format::MAGIC {
                return Err(Error::Unsupported(
                    "cannot append to a file that is not an npids v2 file (convert it with npids.upgrade first)".into(),
                ));
            }
            let header = format::decode_header(&mmap)?;
            let (_, base) = format::current_slot(&mmap)?;
            let off = base.manifest_off as usize;
            let manifest = Manifest::decode(&mmap[off..off + base.manifest_len as usize])?;
            drop(mmap);
            let end = base.manifest_off + base.manifest_len as u64;
            if len > end {
                file.set_len(end)?; // discard a torn tail
            }
            Writer {
                path,
                file,
                opts: opts.clone(),
                manifest,
                base,
                slots: header.map(|s| s.unwrap_or_default()),
                pos: end.next_multiple_of(8),
                pending: Vec::new(),
                pending_ends: Vec::new(),
            }
        };
        if let Some(meta) = &opts.meta {
            w.manifest.meta = meta.clone();
        }
        w.reopen_tail()?;
        Ok(w)
    }

    /// If the last chunk is partial, take its IDs back into the pending buffer; it is rewritten with
    /// the new IDs.
    fn reopen_tail(&mut self) -> Result<()> {
        let c = self.manifest.chunk_size();
        let total = self.manifest.total_count;
        if total.is_multiple_of(c) {
            return Ok(());
        }
        let reader = self.view()?;
        let start = (self.manifest.chunks.len() as u64 - 1) * c;
        let mut buf = Vec::new();
        for idx in start..total {
            buf.clear();
            reader.get(idx, &mut buf)?;
            self.pending.extend_from_slice(&buf);
            self.pending_ends.push(self.pending.len());
        }
        drop(reader);
        self.manifest.chunks.pop();
        self.manifest.total_count = start;
        Ok(())
    }

    /// A reader over what has been written so far (committed or not).
    fn view(&self) -> Result<Reader> {
        // SAFETY: we hold the lock and do not modify written regions while the view is alive.
        let mmap = unsafe { Mmap::map(&self.file)? };
        Reader::from_parts(self.path.clone(), mmap, self.manifest.clone())
    }

    /// Number of IDs in the file, including ones added but not yet committed.
    pub fn len(&self) -> u64 {
        self.manifest.total_count + self.pending_ends.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn add(&mut self, id: &[u8]) -> Result<()> {
        self.pending.extend_from_slice(id);
        self.pending_ends.push(self.pending.len());
        if self.pending_ends.len() as u64 == self.manifest.chunk_size() {
            self.flush_chunk()?;
        }
        Ok(())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<u64> {
        let off = self.pos.next_multiple_of(8);
        write_at(&self.file, bytes, off)?;
        self.pos = off + bytes.len() as u64;
        Ok(off)
    }

    fn flush_chunk(&mut self) -> Result<()> {
        if self.pending_ends.is_empty() {
            return Ok(());
        }
        let mut ids = Vec::with_capacity(self.pending_ends.len());
        let mut s = 0;
        for &e in &self.pending_ends {
            ids.push(&self.pending[s..e]);
            s = e;
        }
        let (t, cols) = infer::choose(&ids);
        let payload = chunk::encode(&cols, ids.len());
        let count = ids.len() as u32;
        drop(cols);
        let template_id = self.manifest.intern(&t);
        let offset = self.append(&payload)?;
        self.manifest.chunks.push(ChunkEntry {
            offset,
            len: payload.len() as u32,
            count,
            template_id,
            crc: crc32c::crc32c(&payload),
        });
        self.manifest.total_count += count as u64;
        self.pending.clear();
        self.pending_ends.clear();
        Ok(())
    }

    /// Write remaining IDs and inverse units, and commit. Dropping a writer without calling this
    /// discards everything added since it was opened.
    pub fn finish(mut self) -> Result<()> {
        self.flush_chunk()?;
        let total = self.manifest.total_count;
        let covered = self.manifest.units.last().map_or(0, |u| u.hi);
        if self.opts.build_inv && covered < total {
            let reader = self.view()?;
            let mut built = Vec::new();
            for (lo, hi, kind, payload) in build_units(&reader, covered, total)? {
                built.push((lo, hi, kind, payload));
            }
            drop(reader);
            for (lo, hi, kind, payload) in built {
                let offset = self.append(&payload)?;
                self.manifest.units.push(UnitEntry {
                    lo,
                    hi,
                    offset,
                    len: payload.len() as u64,
                    kind,
                    crc: crc32c::crc32c(&payload),
                });
            }
        }
        self.manifest.generation = self.base.generation + 1;
        self.manifest.prev_manifest_off = self.base.manifest_off;
        let bytes = self.manifest.encode();
        let off = self.append(&bytes)?;
        self.file.sync_all()?;
        let slot = Slot {
            generation: self.manifest.generation,
            manifest_off: off,
            manifest_len: bytes.len() as u32,
            manifest_crc: crc32c::crc32c(&bytes),
        };
        let which = if self.slots[0].generation <= self.slots[1].generation { 0 } else { 1 };
        write_at(&self.file, &slot.encode(), SLOT_OFFSETS[which] as u64)?;
        self.file.sync_all()?;
        Ok(())
    }
}

type BuiltUnit = (u64, u64, u32, Vec<u8>);

/// Build inverse units covering `[lo, hi)`: DERIVED units for long runs of increasing keys under one
/// template (they cost nothing), and PHF units for everything in between.
fn build_units(reader: &Reader, lo: u64, hi: u64) -> Result<Vec<BuiltUnit>> {
    let min_run = 4096.max((hi - lo) / 32);
    let mut out = Vec::new();
    let mut gap = lo;
    let push_phf = |out: &mut Vec<BuiltUnit>, a: u64, b: u64| -> Result<()> {
        let mut a = a;
        while a < b {
            let e = (a + MAX_PHF_UNIT).min(b);
            out.push((a, e, UNIT_PHF, build_phf(reader, a, e)?));
            a = e;
        }
        Ok(())
    };
    for (a, b, tid) in increasing_runs(reader, lo, hi)? {
        if b - a >= min_run || (a == lo && b == hi) {
            push_phf(&mut out, gap, a)?;
            let mut p = Vec::with_capacity(8);
            p.extend_from_slice(&tid.to_le_bytes());
            p.extend_from_slice(&[0; 4]);
            out.push((a, b, UNIT_DERIVED, p));
            gap = b;
        }
    }
    push_phf(&mut out, gap, hi)?;
    Ok(out)
}

/// Maximal runs `(start, end, template_id)` partitioning `[lo, hi)`, each with a single template and
/// strictly increasing keys.
fn increasing_runs(reader: &Reader, lo: u64, hi: u64) -> Result<Vec<(u64, u64, u32)>> {
    let m = reader.manifest();
    let bits = m.chunk_bits;
    let file = reader.file();
    let mask = m.chunk_size() - 1;
    let mut runs = Vec::new();
    let mut start = lo;
    let mut tid_cur: Option<u32> = None;
    let (mut prev, mut cur): (Vec<Val>, Vec<Val>) = (Vec::new(), Vec::new());
    for ci in (lo >> bits) as usize..=((hi - 1) >> bits) as usize {
        let info: &ChunkInfo = reader.chunk(ci)?;
        let base = (ci as u64) << bits;
        let (ja, jb) = (lo.max(base) & mask, (hi - base).min(info.count as u64));
        if tid_cur != Some(info.template_id) {
            if let Some(t) = tid_cur {
                runs.push((start, base + ja, t));
            }
            start = base + ja;
            tid_cur = Some(info.template_id);
            prev.clear();
        }
        for j in ja..jb {
            info.key(file, j as usize, &mut cur);
            if !prev.is_empty() && cur <= prev {
                runs.push((start, base + j, info.template_id));
                start = base + j;
            }
            std::mem::swap(&mut prev, &mut cur);
        }
    }
    runs.push((start, hi, tid_cur.unwrap()));
    Ok(runs)
}

fn build_phf(reader: &Reader, lo: u64, hi: u64) -> Result<Vec<u8>> {
    let mut seed = phf::DEFAULT_SEED;
    let (mut a, mut b) = (Vec::new(), Vec::new());
    'seed: for _ in 0..64 {
        let mut keys = Vec::with_capacity((hi - lo) as usize);
        for idx in lo..hi {
            a.clear();
            reader.get(idx, &mut a)?;
            keys.push((phf::hash(&a, seed), idx - lo));
        }
        keys.sort_unstable();
        // Drop later duplicates of an ID; reseed on a hash collision between different IDs.
        let mut kept: Vec<(u64, u64)> = Vec::with_capacity(keys.len());
        for k in keys {
            // Sorted by (hash, index), so the last kept entry with this hash is the first index.
            if let Some(&(h, first)) = kept.last()
                && h == k.0 {
                    a.clear();
                    b.clear();
                    reader.get(lo + first, &mut a)?;
                    reader.get(lo + k.1, &mut b)?;
                    if a == b {
                        continue;
                    }
                    seed = phf::fmix64(seed.wrapping_add(1));
                    continue 'seed;
                }
            kept.push(k);
        }
        if let Some(p) = phf::build(&kept, hi - lo, seed) {
            return Ok(p);
        }
        seed = phf::fmix64(seed.wrapping_add(1));
    }
    Err(Error::Invalid("could not build a perfect hash".into()))
}

/// Rewrite the file at `path` without dead space, with merged inverse units, and optionally a new
/// chunk size.
pub fn compact(path: impl AsRef<Path>, chunk_bits: Option<u8>) -> Result<()> {
    let path = path.as_ref();
    if !path.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{} not found", path.display())).into());
    }
    let _lock = open_locked(path)?;
    let reader = Reader::open(path)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.compact-{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    let opts = Options {
        chunk_bits: chunk_bits.unwrap_or(reader.manifest().chunk_bits),
        build_inv: reader.inverse_coverage() > 0,
        meta: Some(reader.meta().to_vec()),
    };
    let result = (|| {
        let mut w = Writer::open(&tmp, opts)?;
        let mut buf = Vec::new();
        for idx in 0..reader.len() {
            buf.clear();
            reader.get(idx, &mut buf)?;
            w.add(&buf)?;
        }
        w.finish()?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
