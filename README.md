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

Build a lookup:

```python
from npids import Lookup
Lookup.build(['id1', 'id2', 'id3'], 'path/to/lookup.npids')
```

Perform forward lookups (index to ID)

```python
lookup = Lookup('path/to/lookup.npids')
# individual indices
lookup.fwd[0] # -> 'id1'
lookup.fwd[2] # -> 'id3'
# multiple indices
lookup.fwd[0,1] # -> ['id1', 'id2']
# works with numpy too
lookup.fwd[np.array([0,1])] # -> array(['id1', 'id2'], dtype='<U3')
```

Perform inverse lookups (ID to index)

```python
lookup = Lookup('path/to/lookup.npids')
# individual IDs
lookup.inv['id1'] # -> 0
lookup.inv['id3'] # -> 2
# multiple IDs
lookup.inv['id1', 'id3'] # -> [0, 2]
# works with numpy too
lookup.inv[np.array(['id1', 'id3'])] # -> array([0, 2])
```

That's about it!

## Codecs

The following codecs are currently supported for forward and inverted lookups. The file format is
flexible, allowing new codecs to be added in the future.

Forward:

 - `fixedbytes`: Every item is stored as a fixed number of bytes (with optional prefix). This
   serves as a fallback if other forward codecs do not work.
 - `intsequence`: A sequence of integers (e.g., 49, 50, 51) is identifed (with optional prefix); only
   metadata about the sequence is stored.
 - `intstored`: Integers are identified (with optional prefix), but they are not in a periodic sequence
   (e.g., 49, 55, 21). The integer values are encoded and stored.
 - `uuid`: UUIDs are identified (with optional prefix). The byte values of the UUIDs are stored.

Inverse:

 - `hash`: Hashes of every item are stored on disk, enabling O(1) lookups (but with extra storage).
   This serves as a fallback if other inverse codecs do not work.
 - `intsequence`: The values only consist of a single forward `intsequence` block; these values can be
   used to compute the indices.
 - `intstored`: The values consist of only `intstored` blocks with values in sorted order. These values
   can be deconstructed and looked up in the foward codec using a binary search.

## Benchmarks

The following benchmarks test the speed of building, forward/inverse lookups (10k random lookups,
both "cold" and "hot"), and the size of the structure. Rows marked with `*` indicate that the values
include additional overheads that are not directly related to operation -- namely,
full engines include content indexing.

 - `npids`: This software
 - `inmem`: A simple Python lookup structure in memory (a list and a dict), backed by a plain text file
   that is read into memory
 - `Terrier`: Terrier engine acccessed via the pyterrier package
 - `Lucene`: Apache Lucene accessed via the pyserini package

The benchmarks show that `npids` is a reasonable choice for performing ID lookups.
Although it is a bit slower than loading them all into memory, it avoids the
considerable upfront cost of doing so. Compared to other approaches for loading them
from disk (Lucene, Terrier), it consumes far less storage, is built faster, and
(usually) performs the lookups considerably faster.

[`msmarco-passage`](https://ir-datasets.com/msmarco-passage) (8.8M docnos: `0`, `1`, `2`, ...)

| System   | Build Time | Cold Fwd | Hot Fwd | Cold Inv | Hot Inv | File Size |
|----------|-----------:|---------:|--------:|---------:|--------:|----------:|
| inmem    |      5.95s |      4ms |     1ms |      6ms |     2ms |     1.3GB |
| `npids`  |     13.88s |      6ms |     6ms |      4ms |     2ms |      206B |
| Lucene   |   * 55.39s |    119ms |    53ms |    194ms |    60ms | * 130.3MB |
| Terrier  |    * 3m53s |    121ms |   107ms |    1.60s |   218ms | * 502.9MB |

[`msmarco-document`](https://ir-datasets.com/msmarco-document) (3.2M docnos: `D1555982`, `D301595`, `D1359209`, ...)

| System   | Build Time | Cold Fwd | Hot Fwd | Cold Inv | Hot Inv | File Size |
|----------|-----------:|---------:|--------:|---------:|--------:|----------:|
| inmem    |      1.44s |      3ms |     1ms |      5ms |     2ms |    27.9MB |
| `npids`  |     13.02s |      6ms |     5ms |      8ms |     8ms |    42.5MB |
| Lucene   |   * 25.57s |    142ms |    61ms |    162ms |    62ms |  * 67.6MB |
| Terrier  |    * 1m26s |    111ms |   103ms |    866ms |   197ms | * 195.0MB |

[`hc4/fa`](https://ir-datasets.com/hc4#hc4/fa) (486k docnos: `9064520f-bc4d-4118-a30e-7d99f5adc612`, `e34ce085-cc13-4a1f-90e4-81a7fbfd7f0d`, `fa2fc4eb-4f97-4bee-bf92-a7330a80c66f`, ...)

| System   | Build Time | Cold Fwd | Hot Fwd | Cold Inv | Hot Inv | File Size |
|----------|-----------:|---------:|--------:|---------:|--------:|----------:|
| inmem    |      0.14s |      2ms |     1ms |      5ms |     1ms |    18.0MB |
| `npids`  |      2.81s |     21ms |    20ms |     32ms |    31ms |    11.8MB |
| Lucene   |    * 4.26s |    145ms |    79ms |    163ms |    75ms |  * 49.4MB |
| Terrier  |   * 14.76s |    125ms |   107ms |    564ms |   187ms |  * 85.1MB |
