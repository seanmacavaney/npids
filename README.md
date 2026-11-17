# npids

This package provides time- and space-efficient bi-directional lookups for identifiers.
Contents are mmap'd, eliminating most load times and allowing for efficient caching
through the file system.

## Motivation

It's often helpful to map an external string identifier to an integer index and vice versa.
Existing techniques for doing this in Python are either slow or require a lot of memory.

## Getting Started

Install via pip:

```bash
pip install npids
```

Prebuilt wheels are available for Linux (x86_64, aarch64, musl x86_64), macOS (x86_64, arm64) and
Windows (x86_64), for CPython 3.9 and later, including free-threaded CPython 3.14t and 3.15t (except
on musl). Other platforms build from source, which needs a Rust toolchain.

On free-threaded Python, npids runs without the GIL: many threads can share one `Lookup`. A builder
should only be used from one thread at a time.

Build a lookup:

```python
from npids import Lookup
Lookup.build(['id1', 'id2', 'id3'], 'path/to/lookup.npids')
```

Perform forward lookups (index to ID)

```python
lookup = Lookup('path/to/lookup.npids')
# individual indices (negative indices count from the end)
lookup.fwd[0] # -> 'id1'
lookup.fwd[-1] # -> 'id3'
# multiple indices
lookup.fwd[0,1] # -> ['id1', 'id2']
lookup.fwd[1:] # -> ['id2', 'id3']
# works with numpy too
lookup.fwd[np.array([0,1])] # -> array(['id1', 'id2'], dtype='<U3')
```

Perform inverse lookups (ID to index)

```python
lookup = Lookup('path/to/lookup.npids')
# individual IDs
lookup.inv['id1'] # -> 0
lookup.inv['id3'] # -> 2
lookup.inv['nope'] # -> KeyError
lookup.inv.get('nope') # -> None
# multiple IDs (-1 for IDs that are not present)
lookup.inv['id1', 'id3', 'nope'] # -> [0, 2, -1]
# works with numpy too
lookup.inv[np.array(['id1', 'id3'])] # -> array([0, 2])
```

`lookup[...]` picks a forward or inverse lookup based on the type of its argument. IDs may repeat;
an inverse lookup returns the first index of an ID.

### Appending

Building into an existing file appends to it. Nothing is committed until the builder closes, so a
crash or an exception leaves the file as it was:

```python
with Lookup.builder('path/to/lookup.npids') as builder:
    builder.add('id4')
    builder.add_many(['id5', 'id6'])
```

Appends only pay for the IDs they add. Over many appends, `Lookup.compact(path)` rewrites the file
without leftover space and merges its inverse indexes.

Readers can keep using a file while it's being appended to; they see the IDs that were present when
they opened it. One writer at a time is allowed (enforced with a file lock).

### Metadata

You can store a JSON object with the IDs:

```python
Lookup.build(ids, path, meta={'dataset': 'msmarco-passage'})
Lookup(path).meta # -> {'dataset': 'msmarco-passage'}
```

### Files from npids 0.1

npids 1.0 writes a new file format (v2). `Lookup(path)` still opens files written by npids 0.x (as a
read-only `npids.legacy.Lookup`), so existing files keep working without changes. They can't be
appended to, and lookups are faster on v2 files. Convert them with `npids.upgrade`:

```python
import npids
npids.upgrade('path/to/lookup.npids')                   # in place
npids.upgrade('old.npids', new_path='new.npids')         # or to a new file
```

For large batches, `lookup._reader.get_many_raw(idxs)` returns the IDs as an Arrow-style `(offsets, data)`
pair without creating a Python object per ID (e.g. for `pyarrow.StringArray.from_buffers`).

## How it works

IDs are stored in fixed-size chunks (65,536 IDs by default). Each chunk picks a *template* that all of
its IDs match, such as `D{dec}`, `{hex:8}-{hex:4}-{hex:4}-{hex:4}-{hex:12}` or `doc-{dec:5}`, and
stores only the template's fields, compressed as constants, arithmetic sequences, runs of arithmetic
sequences (like ClueWeb22's `clueweb22-en0000-03-08290`, whose last field restarts at 0 for every
source file) or bit-packed integers. IDs that don't follow a pattern are stored as plain strings
(`{str}`).

Inverse lookups use the same templates where they can: when IDs are in increasing order (like
`0, 1, 2, ...` or sorted docnos), finding an ID is a binary search over the forward data, at no extra
storage cost. Otherwise, npids builds a perfect hash (about 3 bits per ID) plus a table from hash slot
to index.

The core is written in Rust. The file format is specified in [FORMAT.md](FORMAT.md).

## Benchmarks

`bench/compare.py` builds each sample in `test/samples` with npids 0.1 (format v1) and 1.0 (format
v2), and times 10k random lookups with a warm cache (Apple M3).

| sample | IDs | version | build | size | fwd 10k | inv 10k |
|---|---:|---|---:|---:|---:|---:|
| msmarco-passage (`0`, `1`, ...) | 8.8M | 0.1 | 3.45s | 206B | 3.1ms | 4.9ms |
| | | 1.0 | 1.37s | 9.8KB | 0.8ms | 1.4ms |
| msmarco-document (`D1555982`, ...) | 3.2M | 0.1 | 3.65s | 40.5MB | 4.1ms | 7.8ms |
| | | 1.0 | 1.28s | 18.1MB | 1.0ms | 1.4ms |
| neuclir/1/fa (UUIDs) | 2.2M | 0.1 | 4.72s | 58.6MB | 10.5ms | 17.7ms |
| | | 1.0 | 1.23s | 39.2MB | 1.6ms | 4.6ms |
| beir/fever (Wikipedia titles) | 5.4M | 0.1 | 10.15s | 1.1GB | 4.3ms | 12.1ms |
| | | 1.0 | 2.58s | 137.1MB | 1.5ms | 2.2ms |
| beir/dbpedia-entity | 4.6M | 0.1 | 8.03s | 833.1MB | 4.9ms | 14.3ms |
| | | 1.0 | 2.34s | 120.6MB | 1.6ms | 3.1ms |
| disks45/nocr | 528k | 0.1 | 0.39s | 10.7MB | 6.2ms | 11.2ms |
| | | 1.0 | 0.21s | 5.3MB | 1.2ms | 1.7ms |
| cord19 | 193k | 0.1 | 0.08s | 3.2MB | 2.0ms | 4.0ms |
| | | 1.0 | 0.08s | 2.7MB | 0.8ms | 1.1ms |

## Development

```bash
pip install maturin
maturin develop --release
cargo test --release
pytest test/
```
