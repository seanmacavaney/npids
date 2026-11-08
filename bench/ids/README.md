# Doc ID benchmark lists

Real document ID lists from IR collections, one ID per line in collection order, gzip'd. The lists
themselves are too large for the repository. They are kept on the IDA cluster at
`/mnt/extra/npids-ids/` (with `counts.txt` and `SHA256SUMS`); `collect.sh` rebuilds them.

| list | IDs | .txt.gz | example | source / order |
|---|---:|---:|---|---|
| msmarco-passage | 8,841,823 | 19 MB | `0` | ir_datasets `msmarco-passage` |
| beir_fever | 5,416,568 | 49 MB | `1928_in_association_football` | ir_datasets `beir/fever` |
| neuclir_1_fa | 2,232,016 | 47 MB | UUID | ir_datasets `neuclir/1/fa` |
| neuclir_1_ru | 4,627,543 | 98 MB | UUID | ir_datasets `neuclir/1/ru` |
| neuclir_1_zh | 3,179,209 | 67 MB | UUID | ir_datasets `neuclir/1/zh` |
| arxiv | 3,243,515 | 10 MB | `cond-mat/0208404`, `0704.0001` | `/mnt/arxiv` sqlite, by first harvested revision |
| pubmed | 40,999,979 | 91 MB | `1` | `/mnt/pubmed` sqlite, article insertion order |
| gov2 | 25,205,179 | 113 MB | `GX000-00-0000000` | `<DOCNO>`s of the corpus files, directory/file order |
| car-v2.0 | 29,794,697 | 658 MB | 40-hex-digit SHA1 | ir_datasets `car/v2.0` |
| clueweb22-b-en | 87,208,655 | 231 MB | `clueweb22-en0000-00-00000` | document list of the ClueWeb22-B (en) PISA index |
| clueweb12 | 733,019,372 | 1.9 GB | `clueweb12-0000tw-00-00000` | ClueWeb12 per-WARC record counts, file order |
| msmarco-passage-v2 | 138,364,198 | 546 MB | `msmarco_passage_00_0` | pids of the extracted corpus files, file order |
| msmarco-v2.1-segmented | 113,520,750 | 816 MB | `msmarco_v2.1_doc_00_0#0_0` | TREC RAG 2024 segmented corpus, file order |

```
bash collect.sh                      # all lists, then counts.txt and SHA256SUMS
bash collect.sh gov2 pubmed summary  # or some of them
python bench.py [--v1-max N] *.txt.gz > bench.md 2> bench.err
```

`bench.py` streams each list twice: once to count it and sample 10k random positions, and once to
build a v2 file with `Lookup.builder` (and a v1 file with `npids.legacy`, for lists of at most
`--v1-max` IDs). It checks the sample's forward and inverse lookups, then times 10k random forward
lookups (as `list[str]`, and as raw `(offsets, bytes)` via `get_many_raw`) and 10k inverse lookups,
best of 3 with a warm cache. The v2 file's `describe()` goes to stderr.

## Results

IDA cluster pod (Xeon, files on local disk, warm page cache), npids 0.2.0 vs 0.1.3 (format v1).
Times are for 10k lookups.

| list | IDs | build v1 | build v2 | size v1 | size v2 | fwd v1 | fwd v2 (str / raw) | inv v1 | inv v2 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| arxiv | 3.2M | 3s | 2s | 77.9MB | 54.0MB | 2.4ms | 0.7ms / 0.2ms | 4.2ms | 1.3ms |
| msmarco-passage | 8.8M | 8s | 3s | 206B | 9.8KB | 3.0ms | 0.6ms / 0.2ms | 3.5ms | 0.9ms |
| neuclir_1_fa | 2.2M | 7s | 2s | 58.6MB | 39.2MB | 13.1ms | 1.0ms / 0.6ms | 17.7ms | 2.0ms |
| neuclir_1_ru | 4.6M | 14s | 4s | 120.3MB | 81.7MB | 13.2ms | 1.0ms / 0.6ms | 18.0ms | 2.1ms |
| neuclir_1_zh | 3.2M | 10s | 3s | 76.6MB | 55.8MB | 13.5ms | 1.0ms / 0.6ms | 19.8ms | 1.9ms |
| beir_fever | 5.4M | 9s | 4s | 1.1GB | 137.3MB | 4.2ms | 0.9ms / 0.3ms | 10.3ms | 1.5ms |
| pubmed | 41.0M | 75s | 27s | 568.6MB | 232.6MB | 3.9ms | 0.6ms / 0.3ms | 6.1ms | 1.3ms |
| gov2 | 25.2M | 28s | 10s | 455.2MB | 94.9MB | 40.9ms | 0.9ms / 0.5ms | 64.9ms | 4.8ms |
| car-v2.0 | 29.8M | 54s | 18s | 809.9MB | 539.9MB | 3.7ms | 0.9ms / 0.5ms | 6.6ms | 6.1ms |
| clueweb22-b-en | 87.2M | 155s | 36s | 861.8MB | 183.8MB | 34.9ms | 1.0ms / 0.5ms | 52.4ms | 5.5ms |
| msmarco-v2.1-segmented | 113.5M | 137s | 61s | 3.1GB | 816.4MB | 83.0ms | 1.3ms / 0.8ms | 111.3ms | 6.9ms |
| msmarco-passage-v2 | 138.4M | 224s | 61s | 527.8MB | 415.8MB | 5.3ms | 0.8ms / 0.4ms | 28.6ms | 4.8ms |
| clueweb12 | 733.0M | - | 336s | - | 1.9GB | - | 1.0ms / 0.7ms | - | 40.4ms |

clueweb12 was not built with v1 (`--v1-max 150000000`).

Notes:

- gov2's last field is a byte offset, zero-padded to 7 digits but 8 digits past 10M
  (`GX000-71-11579329`); it uses `GX{dec:3}-{dec:2}-{dec:7+}` (a minimum-width `DEC`).
- Lists whose IDs increase (car, gov2, the msmarco v2 lists, clueweb) get DERIVED inverse units at no
  storage cost, and inverse lookups there are a binary search (5-7ms per 10k) rather than a hash
  probe (1.3-2ms). The others get a perfect hash.
- clueweb12's IDs alternate between `...tw-...` and `...wb-...` segments, which need different
  templates, so its inverse index is ~35 units (DERIVED runs with PHF gaps). Lookups probe units in
  order to return the first index, which makes them the slowest here (4us per lookup).
- For msmarco-passage, v1 stores a range (206 bytes) and v2 one chunk record per 64K IDs.

SHA256 of the lists:

```
1d104e8cb43a04693ef4837643fc578ae868216fb0237d57a11a7f36dfa3d407  arxiv.txt.gz
abc56748e996e318156048c09ccde763e7bff167a26bca21e948155f18cee6c7  beir_fever.txt.gz
3f055375f4a90c925ca1e99ce42d20a6eb1b9ce22503a91c5a86d4d0b326c1b1  car-v2.0.txt.gz
d2a3a34e88ee23027c60ffc18c91383b5223ed89c8dee35def40261da4b9bf94  clueweb12.txt.gz
5a3eb7d2041ffc1f16466d0ce530b7f2106bf79bc06c7de05b35a50f7505860e  clueweb22-b-en.txt.gz
957f44786ecd455bae4ab3ee5b1b714b4cbd0d9425dc474cf2e823c22a0ffa77  gov2.txt.gz
e1b2741da51aa54129d1ed7cd5706e117431da74f47bbd766826186493c4e2eb  msmarco-passage-v2.txt.gz
5b7e1cdd13b938c78a11cf094b19285e4379a86d7b6847169ca15aa6ed34209c  msmarco-passage.txt.gz
9a30738a08fbd8db74a08a234e7683bf4c4c9a5785008fec2ffbb1376263f0b4  msmarco-v2.1-segmented.txt.gz
5e9d8662bd3e6e6bb49aed852c5959a723fd5e4545973e07b2a72266a42bd291  neuclir_1_fa.txt.gz
d8a60ea7df9abbe0b574df9c7a0f6db3a992194f6a6ef4633667358b3bbcb579  neuclir_1_ru.txt.gz
8fb8e19f0b79cf52832b8f5dee9dcd7355b8aab3722393c8b22438e1053ee005  neuclir_1_zh.txt.gz
03585a9e0c1d3b6ac9624083559ab99d9c30e8fc4a0ccb7cf79ec9cc80e8f208  pubmed.txt.gz
```
