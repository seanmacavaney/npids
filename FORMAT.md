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
| 10 | 2  | `minor` = 1 |
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

Minor versions:

- **2.0** (npids 1.0.0, 1.0.1): column encodings 0–3.
- **2.1** (npids 1.1): adds column encoding 4, `RUNS`. Files without `RUNS` columns are unchanged.

Writers put their own `minor` in the header of a file they create; appending never rewrites the header,
so a file created as 2.0 can hold `RUNS` chunks appended later. The header's `minor` is therefore only a
hint: what a reader must support is decided per chunk, by its encodings. A 2.0 reader opens a file with
`RUNS` columns, then refuses (with an "unknown column encoding 4" error) each lookup that touches such a
chunk; it never misreads one.

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

| `kind` | Name | Matches | Value |
|------:|------|---------|-------|
| 0 | `LIT` | the literal bytes (at least one) | none |
| 1 | `DEC` | a decimal number zero-padded to at least `width` digits, `0 <= width <= 20` (`width = 0`: no leading zeros) | u64 |
| 2 | `DECW` | exactly `width` digits, `1 <= width <= 20` | u64 |
| 3 | `HEXL` | exactly `width` digits from `0-9a-f`, `1 <= width <= 16` | u64 |
| 4 | `HEXU` | exactly `width` digits from `0-9A-F`, `1 <= width <= 16` | u64 |
| 5 | `STR` | any bytes, possibly none | bytes |

A template is valid only if:

- no two `LIT` parts are adjacent;
- every `DEC` is the last part, or is followed by a `LIT` whose first byte is not a digit;
- it has at most one `STR`, and every part after the `STR` is a `LIT`, `DECW`, `HEXL` or `HEXU`.

These rules make parsing a single left-to-right pass with no backtracking. Readers MUST reject files with
invalid templates.

**Parsing** an ID with a template reads the parts in order from position 0:

- `LIT`: the bytes at the position must equal the literal.
- `DEC`: take the run of digits at the position (at least one). If `width = 0` and it starts with `0`,
  take only that `0`.
- `DECW`, `HEXL`, `HEXU`: take exactly `width` bytes, all from the part's character set.
- `STR`: take everything up to `len(id) - s`, where `s` is the total width of the parts after it
  (which must not be before the position).

The ID matches if every part succeeds, no `DEC` or `DECW` value exceeds `2^64 - 1`, and the parts end
exactly at the end of the ID. **Formatting** writes the parts in order (`DEC` zero-padded to at least
`width` digits, `DECW` zero-padded to `width`, hex at exactly `width` digits).

An ID may be stored with a template only if parsing succeeds and formatting the parsed values gives back
the ID byte for byte. Writers MUST check this for every ID; readers rely on it. Because parsing is
deterministic, a query that equals a stored ID parses to the same values.

Examples, written in the human-readable form used by `describe()`:

- `D{dec}`: msmarco-document
- `{dec}`: msmarco-passage
- `{hex:8}-{hex:4}-{hex:4}-{hex:4}-{hex:12}`: a lowercase UUID
- `{hex:16}{hex:16}`: a 32-digit hex digest
- `GX{dec:3}-{dec:2}-{dec:7+}`: GOV2 (the last field is padded to 7 digits but can be longer)
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
| 2  | 2 | `stride` (FOR only): bits per row; 0 means `bits` |
| 4  | 2 | `bit_off` (FOR only): offset of this field within a row |
| 6  | 2 | reserved, 0 |
| 8  | 8 | `p0` |
| 16 | 8 | `p1` |
| 24 | 4 | `data_off`, relative to the chunk payload start |
| 28 | 4 | `data_len` |

Value of position `j` (`0 <= j < count`) for each encoding:

| `enc` | Name | Field kinds | Value at `j` | Data |
|-----:|------|-------------|--------------|------|
| 0 | `CONST` | integer | `p0` | none |
| 1 | `SEQ` | integer | `p0 + j * (i64)p1`, wrapping in u64 | none |
| 2 | `FOR` | integer | `p0 +` the `bits` bits at `j * stride + bit_off` | packed bit stream |
| 3 | `PLAIN` | `STR` | `blob[off[j] .. off[j+1]]` | `u32 off[count+1]`, then `blob` |
| 4 | `RUNS` | integer | `vals[r * width + slot] + (j - starts[r]) * (i64)p0`, wrapping in u64 | run table (below) |

`PLAIN` blob offsets are relative to the start of the blob, which immediately follows `off`.

### Runs

`RUNS` (since 2.1) stores a field that is piecewise arithmetic: the chunk splits into `n` runs, and
within each run the field steps by the same `p0` (as `i64`; 0 makes it piecewise constant). It is meant
for IDs like ClueWeb22's `clueweb22-en0000-03-08290`, where the last field counts up within a source
file and restarts at 0 when the middle field increments: one chunk holds a few such runs, so storing
each run's start costs bytes per chunk instead of bits per ID.

For a `RUNS` field, the descriptor's `p0` is the step, `p1` is the number of runs `n`
(`1 <= n <= count`), `stride` is the run table's `width` (number of fields it holds; 0 means 1), and
`bit_off` is this field's `slot` in it (`slot < width`). `bits` is 0. `data_off` points to the run
table:

```
u32 starts[n]              first position of each run: starts[0] = 0, strictly increasing, < count
zero padding to 8 bytes
u64 vals[n * width]        vals[r * width + slot]: the slot's field value at position starts[r]
```

`data_len` is at least `ceil8(4 * n) + 8 * n * width`. Position `j` is in run `r`, the last run with
`starts[r] <= j` (a binary search over `starts`). Readers MUST reject a run table whose `starts` are not
as above.

Several `RUNS` fields of a chunk may share one run table (and its `starts`), like `FOR` fields sharing a
bit stream: their descriptors then have the same `data_off`, `data_len`, `p1` and `stride`, and different
`slot`s. Each field still has its own step. A run of a shared table need not be a maximal run of every
field in it (a field that does not change at a breakpoint simply repeats its value).

When every non-constant field of a chunk is `RUNS` in one shared table, keys within a run differ only by
the steps, so an inverse lookup (`DERIVED`) can binary-search the runs and then compute the position.

Several `FOR` fields of a chunk may share one bit stream, stored row by row: each row holds one value of
every such field, `stride` bits in all, with the field's value at `bit_off` within the row. Their
descriptors then have the same `data_off`, `data_len` and `stride`, and `bit_off + bits <= stride`.
This puts all of an ID's fields next to each other, so a random lookup reads one place instead of one
per field. A `FOR` field with its own stream has `stride = 0` (meaning `bits`) and `bit_off = 0`.

Encodings 5–15 are reserved (for example, FSST-compressed strings).

### Bit packing

A bit stream is a sequence of u64 words. Bit `i` of the stream is bit `i % 64` (least significant
first) of word `i / 64`. Reading `bits` bits (`0 <= bits <= 64`) at position `i` gives the value whose
bit `k` is stream bit `i + k`; with `bits = 0` the value is 0. A stream holding `n` rows of `stride`
bits is `8 * (ceil(n * stride / 64) + 1)` bytes long, so a reader can always read the word after the
one a value starts in. A packed array of `n` values of width `bits` is a stream with `stride = bits`.

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

Definitions (all arithmetic on u64, wrapping, unless noted):

```
h          = hash64(id, seed)
b1         = (b * 3) / 10                                # dense buckets
bucket(h)  = (h & 0xFFFFFFFF) < 0x99999999               # ~60% of keys
               ? fastrange32(h >> 32, b1)
               : b1 + fastrange32(h >> 32, b - b1)
pos(h, p)  = fastrange64(fmix64(h ^ fmix64(p)), m)
fastrange32(x, k) = (x * k) >> 32
fastrange64(x, k) = (x * k) >> 64                        # u128 product

fmix64(k):                                               # MurmurHash3 finalizer
  k ^= k >> 33; k *= 0xff51afd7ed558ccd
  k ^= k >> 33; k *= 0xc4ceb9fe1a85ec53
  k ^= k >> 33; return k

hash64(id, seed):
  h = seed ^ (len(id) * 0x9e3779b97f4a7c15)
  for each 8-byte block w of id, read little-endian, the last one padded with zero bytes:
    h = (h ^ w) * 0xbf58476d1ce4e5b9
    h ^= h >> 31
  return fmix64(h)
```

`hash64` has a single code path for every ID length, so it is cheap to implement on a GPU.

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
