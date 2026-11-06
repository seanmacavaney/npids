# npids file format, version 2

Status: draft. This document is normative for v2 files; the Rust core (`src/`) implements it.

An npids file is a bidirectional map between dense indices `0..N` and byte strings (IDs).
Forward lookups map an index to its ID; inverse lookups map an ID to its index. IDs may repeat; an
inverse lookup of a repeated ID returns its first (lowest) index.

## Conventions

- All integers are little-endian. `u8/u16/u32/u64` are unsigned, `i64` is two's complement.
- All offsets are absolute byte offsets into the file unless stated otherwise.
- Every structure starts on an 8-byte boundary; writers pad with zero bytes.
- `crc32c` is CRC-32C (Castagnoli).
- `bitlen(x)` is the number of bits needed to represent `x` (`bitlen(0) = 0`).

## Layout

```
Header (80 B)
Segment 1:  chunk payloads, inverse-unit payloads, manifest
Segment 2:  chunk payloads, inverse-unit payloads, manifest      (one per append session)
...
(possibly a torn tail left by a crashed writer)
```

Everything after the header is append-only. The only bytes ever overwritten are the two commit slots
in the header.

## Header

| Offset | Size | Field |
|-------:|-----:|-------|
| 0  | 8  | magic `"NPIDSv2\n"` |
| 8  | 2  | `major` = 2 |
| 10 | 2  | `minor` = 0 |
| 12 | 4  | `flags` = 0 (reserved) |
| 16 | 32 | commit slot A |
| 48 | 32 | commit slot B |

Commit slot:

| Offset | Size | Field |
|-------:|-----:|-------|
| 0  | 8 | `gen`: commit generation; 0 means the slot is empty |
| 8  | 8 | `manifest_off` |
| 16 | 4 | `manifest_len` |
| 20 | 4 | `manifest_crc`: crc32c of the manifest bytes |
| 24 | 4 | `slot_crc`: crc32c of bytes 0..24 of this slot |
| 28 | 4 | reserved, 0 |

Detecting the version: a file starting with `"NPIDSv2\n"` is v2. A file whose first four bytes are
`"NPID"` otherwise is v1, and anything else is v0 (both read by the legacy reader).

Readers MUST refuse `major > 2`. They MAY read files with a higher `minor`, but MUST refuse any chunk
encoding, field kind or inverse-unit kind they do not know.

### Opening

A reader validates both slots (`gen != 0`, `slot_crc` matches, `manifest_off + manifest_len` is within
the file, `manifest_crc` matches) and uses the valid slot with the higher `gen`. If neither is valid,
the file is corrupt.

### Committing

A writer:

1. holds an exclusive `flock` on the npids file itself (see Concurrency);
2. appends chunk payloads, inverse-unit payloads, then the manifest, and fsyncs;
3. overwrites the slot with the **lower** `gen` with `gen = current + 1`, and fsyncs.

A crash before step 3 leaves the previous commit valid; the bytes past the end of the current
manifest are garbage. A torn write in step 3 fails `slot_crc`, so readers fall back to the other slot.
A writer that opens a file truncates it to the end of the current manifest before appending.

A new, empty file is a header plus an empty manifest (`total_count = 0`) committed with `gen = 1` in
slot A.

## Concurrency

- **One writer** at a time, enforced by an exclusive `flock` on the npids file. After acquiring the
  lock, the writer compares `fstat` of its descriptor with `stat` of the path (device and inode). If
  they differ, the file was replaced (by compaction) while it waited: it reopens the path and retries.
- **Any number of readers.** A reader takes a snapshot of the current manifest when it opens the file.
  Everything a manifest references is immutable, so the snapshot stays consistent while writers append.
  A reader sees later commits only by reopening the file.
- **Compaction** writes a complete new file next to the original and renames it over the path while
  holding the lock on the original.

## Manifest

| Offset | Size | Field |
|-------:|-----:|-------|
| 0  | 8 | magic `"NPIDMNFT"` |
| 8  | 8 | `gen` (equal to the slot's `gen`) |
| 16 | 8 | `total_count` = N |
| 24 | 1 | `chunk_bits`; chunk size `C = 2^chunk_bits`. Default 16 |
| 25 | 3 | reserved, 0 |
| 28 | 4 | `n_templates` |
| 32 | 8 | `n_chunks` = `ceil(N / C)` |
| 40 | 4 | `n_units` (inverse units) |
| 44 | 4 | reserved, 0 |
| 48 | 8 | `prev_manifest_off`, 0 if none |
| 56 | 4 | `templates_off`, relative to the start of the manifest |
| 60 | 4 | `chunks_off`, relative |
| 64 | 4 | `units_off`, relative |
| 68 | 4 | `meta_off`, relative |
| 72 | 4 | `meta_len` |
| 76 | 4 | reserved, 0 |

`chunk_bits` is fixed for the life of a file (only compaction can change it).

### Template table

`n_templates` entries, packed one after another, each starting on an 8-byte boundary:

```
u16 n_parts
then n_parts parts:
  u8  kind
  u8  width        (0 if unused)
  u16 lit_len      (0 unless kind = LIT)
  lit_len bytes    (kind = LIT only)
```

Templates in a manifest are distinct; their indices are template IDs.

### Chunk table

`n_chunks` records of 32 bytes. Chunk `i` holds indices `i*C .. min((i+1)*C, N)`. Every chunk except
the last holds exactly `C` IDs.

| Offset | Size | Field |
|-------:|-----:|-------|
| 0  | 8 | `offset` of the chunk payload |
| 8  | 4 | `len` of the chunk payload |
| 12 | 4 | `count` |
| 16 | 4 | `template_id` |
| 20 | 4 | `crc32c` of the chunk payload |
| 24 | 8 | reserved, 0 |

### Inverse-unit table

`n_units` records of 40 bytes, sorted by `lo`, covering disjoint index ranges `[lo, hi)`.

| Offset | Size | Field |
|-------:|-----:|-------|
| 0  | 8 | `lo` |
| 8  | 8 | `hi` |
| 16 | 8 | `offset` of the unit payload |
| 24 | 8 | `len` of the unit payload |
| 32 | 4 | `kind` |
| 36 | 4 | `crc32c` of the unit payload |

Indices not covered by any unit are not reachable by inverse lookups (for example, a file built with
the inverse disabled). Implementations report this as an error rather than "not found".

### Metadata

`meta_len` bytes of UTF-8 JSON (an object), or `meta_len = 0`. Applications may store anything here;
readers ignore keys they do not know.

## Templates

A template is a sequence of parts. Each part is a literal or a field:

| `kind` | Name | Matches (regex) | Value |
|------:|------|-----------------|-------|
| 0 | `LIT` | the literal bytes | none |
| 1 | `DEC` | `0\|[1-9][0-9]*` | u64 |
| 2 | `DECW` | `[0-9]{width}` | u64 |
| 3 | `HEXL` | `[0-9a-f]{width}`, `1 <= width <= 16` | u64 |
| 4 | `HEXU` | `[0-9A-F]{width}`, `1 <= width <= 16` | u64 |
| 5 | `STR` | `.*` (any bytes) | bytes |

`DEC` and `DECW` values must fit in a u64. A template has at most one `STR` part.

**Parsing** an ID with a template means full-matching it against the concatenation of the part regexes
with leftmost-greedy semantics (as in Perl or Rust's `regex`), and reading each field's value from its
capture. **Formatting** writes the parts in order (`DEC` without leading zeros, `DECW` zero-padded to
`width`, hex at exactly `width` digits).

An ID may be stored with a template only if parsing succeeds and formatting the parsed values gives back
the ID byte for byte. Writers MUST check this for every ID; readers rely on it. Because parsing is
deterministic, a query that equals a stored ID parses to the same values.

Examples, written in the human-readable form used by `describe()`:

- `D{dec}`: msmarco-document
- `{dec}`: msmarco-passage
- `{hex:8}-{hex:4}-{hex:4}-{hex:4}-{hex:12}`: a lowercase UUID
- `{hex:16}{hex:16}`: a 32-digit hex digest
- `{str}`: anything

Choosing a template is up to the writer. A chunk has exactly one template, which every ID in the chunk
must satisfy; `{str}` always works.

## Chunk payload

```
u32 count
u16 n_fields           (number of non-LIT parts in the template)
u16 reserved
then n_fields field descriptors (32 B each), in template order
then column data
```

Field descriptor:

| Offset | Size | Field |
|-------:|-----:|-------|
| 0  | 1 | `enc` |
| 1  | 1 | `bits` (FOR only) |
| 2  | 6 | reserved, 0 |
| 8  | 8 | `p0` |
| 16 | 8 | `p1` |
| 24 | 4 | `data_off`, relative to the chunk payload start |
| 28 | 4 | `data_len` |

Value of position `j` (`0 <= j < count`) for each encoding:

| `enc` | Name | Field kinds | Value at `j` | Data |
|-----:|------|-------------|--------------|------|
| 0 | `CONST` | integer | `p0` | none |
| 1 | `SEQ` | integer | `p0 + j * (i64)p1`, wrapping in u64 | none |
| 2 | `FOR` | integer | `p0 + packed[j]` | packed array, `bits` wide |
| 3 | `PLAIN` | `STR` | `blob[off[j] .. off[j+1]]` | `u32 off[count+1]`, then `blob` |

`PLAIN` blob offsets are relative to the start of the blob, which immediately follows `off`.

Encodings 4–15 are reserved (for example, FSST-compressed strings).

### Bit packing

A packed array of `n` values of width `bits` (`0 <= bits <= 64`) is a sequence of u64 words. Value `j`
occupies bits `j*bits .. (j+1)*bits` of the stream, least-significant bit first within each word and
earlier words first. The array is `8 * (ceil(n * bits / 64) + 1)` bytes long, so a reader can always
read the word after the one a value starts in. With `bits = 0` every value is 0 and only the padding
word is stored.

## Inverse units

An inverse lookup of an ID checks units in order of `lo` and returns the first match.

### Kind 1: `DERIVED`

Payload: `u32 template_id`, `u32 reserved`.

Valid when every ID in `[lo, hi)` parses with `template_id` and the parsed **keys are strictly
increasing** with the index. A key is the tuple of field values in template order, compared
lexicographically; integer fields compare numerically and `STR` fields bytewise.

Lookup: parse the query with `template_id` (no match means not found), then binary search `[lo, hi)`
by key. Reading the key at an index may use the chunk's columns directly when the chunk's template is
`template_id`; otherwise it parses the stored ID. (A chunk can have a different template from the unit
when an append rewrote it.)

This costs no space beyond the payload header. `{str}` units over sorted IDs are `DERIVED` too.

### Kind 2: `PHF`

A perfect hash over the distinct IDs in `[lo, hi)`, plus a table from hash slot to index.

Payload:

| Offset | Size | Field |
|-------:|-----:|-------|
| 0  | 8 | `n`: number of distinct IDs |
| 8  | 8 | `m`: number of slots, `m >= n` |
| 16 | 8 | `b`: number of buckets, `1 <= b < 2^32` |
| 24 | 8 | `seed` |
| 32 | 1 | `pilot_bits` |
| 33 | 1 | `slot_bits` = `bitlen(hi - lo)` |
| 34 | 6 | reserved, 0 |
| 40 | 8 | `pilots_off`, relative to the payload start |
| 48 | 8 | `slots_off`, relative |

followed by `pilots` (`b` values packed at `pilot_bits`) and `slots` (`m` values packed at `slot_bits`).
A slot holds `index - lo`, or `2^slot_bits - 1` if empty.

Definitions:

```
h          = xxh3_64(id, seed)
b1         = floor(0.3 * b)                              # dense buckets
bucket(h)  = (h & 0xFFFFFFFF) < 0x99999999               # ~60% of keys
               ? fastrange32(h >> 32, b1)
               : b1 + fastrange32(h >> 32, b - b1)
pos(h, p)  = fastrange64(fmix64(h ^ fmix64(p)), m)
fastrange32(x, k) = (x * k) >> 32                        # u64 arithmetic
fastrange64(x, k) = (x * k) >> 64                        # u128 arithmetic
fmix64     = the MurmurHash3 64-bit finalizer
```

When `b1 = 0` (very small units), every key goes to the second branch.

Lookup: compute `h`, `p = pilots[bucket(h)]`, `v = slots[pos(h, p)]`. If `v` is the empty marker the ID
is not in this unit; otherwise compare the query with the forward lookup of `lo + v`.

Building requirements:

- Duplicate IDs are reduced to their first index before building.
- Distinct IDs must have distinct `h`. If two collide, the writer picks another `seed`.
- Every pilot must place all keys of its bucket in distinct, previously empty slots.

Kind 0 and kinds above 2 are reserved.

## Appending

1. Lock, open the current commit, and truncate any torn tail.
2. If the last chunk is partial, re-encode its IDs followed by the new ones into new chunks. Those IDs
   keep their indices, so existing inverse units stay valid.
3. Write new chunks, then one new inverse unit covering only the new indices `[N_old, N_new)`.
4. Write the manifest (with `prev_manifest_off` set) and commit.

## Compaction

Write a fresh file containing only live data, optionally with a different `chunk_bits`, with inverse
units merged (one unit for the whole file, `DERIVED` where it qualifies), then rename it over the
original while holding the lock (see Concurrency).

## Integrity

Readers verify `manifest_crc` on open. Chunk and unit checksums are verified on demand (for example,
by a `verify()` call), not on every access.
