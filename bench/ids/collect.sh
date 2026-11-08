#!/bin/bash
# Collect the doc ID benchmark lists (one ID per line, in collection order, gzip'd).
#
# Written for the IDA cluster (an idabox pod with ~/.ir_datasets, /mnt/clueweb12, /mnt/indices,
# /mnt/datasets/DOTGOV2, /mnt/pubmed and /mnt/arxiv mounted). Outputs go to the current directory.
# Everything streams; peak memory is small. See README.md for where each list comes from.
#
# Usage: bash collect.sh [name ...]   (default: all, then summary)
#        e.g. run groups in parallel, then: bash collect.sh summary
set -euo pipefail
IRDS=${IR_DATASETS_HOME:-$HOME/.ir_datasets}
JOBS=${JOBS:-4}

irds() {  # ir_datasets-id output-name
  ir_datasets export "$1" docs --fields doc_id | gzip -6 > "$2.txt.gz.part"
  mv "$2.txt.gz.part" "$2.txt.gz"
}

# Run "$cmd {}" for every item on stdin with $JOBS workers, each writing a gzip member to parts/$name/{}.gz,
# then concatenate the members in sorted order.
parts() {  # name cmd
  rm -rf "parts/$1" && mkdir -p "parts/$1"
  xargs -P "$JOBS" -I{} bash -c "$2 | gzip -6 > parts/$1/{}.gz"
  cat $(ls "parts/$1"/*.gz | sort) > "$1.txt.gz"
}

clueweb12() {
  # IDs are clueweb12-<file>-<record:05d>; the record counts per WARC file give the full list.
  python3 - <<'PY' | gzip -6 > clueweb12.txt.gz.part
import glob, sys
for counts in sorted(glob.glob('/mnt/clueweb12/recordcounts/ClueWeb12_*_counts.txt')):
    rows = []
    for line in open(counts):
        path, n = line.split()
        rows.append((path.split('/')[-2], path.split('/')[-1].split('.')[0], int(n)))
    for d, f, n in sorted(rows):
        sys.stdout.write(''.join(f'clueweb12-{f}-{r:05d}\n' for r in range(n)))
PY
  mv clueweb12.txt.gz.part clueweb12.txt.gz
}

clueweb22-b-en() {
  # The document list of the PISA index of ClueWeb22-B (English), in index order.
  gzip -6 -c /mnt/indices/cw22.b.en.pisa/fwd.documents > clueweb22-b-en.txt.gz
}

msmarco-passage-v2() {
  # pid is the first JSON field of each line of the extracted corpus files.
  local D=$IRDS/msmarco-passage-v2/msmarco_v2_passage.tar.extracted
  ls "$D" | grep -E '^msmarco_passage_[0-9]+$' | sort | parts msmarco-passage-v2 "cut -d'\"' -f4 $D/{}"
}

msmarco-v2.1-segmented() {
  # docid is the first JSON field of each line of the segmented corpus (TREC RAG 2024).
  local D=$IRDS/msmarco-document-v2.1/docs-segmented/msmarco_v2.1_doc_segmented
  ls "$D" | grep 'json.gz$' | sort | parts msmarco-v2.1-segmented "zcat $D/{} | cut -d'\"' -f4"
}

gov2() {
  # <DOCNO> tags of every file, in directory/file order. -a: GOV2 contains binary documents.
  local D=/mnt/datasets/DOTGOV2/gov2-corpus
  ls "$D" | grep '^GX' | sort | parts gov2 \
    "for f in \$(ls $D/{}/*.gz | sort); do zcat \$f | grep -a -o '<DOCNO>[^<]*' | cut -c8-; done"
}

msmarco-passage() { irds msmarco-passage msmarco-passage; }
beir_fever() { irds beir/fever beir_fever; }
car-v2.0() { irds car/v2.0 car-v2.0; }
neuclir() { irds neuclir/1/fa neuclir_1_fa; irds neuclir/1/ru neuclir_1_ru; irds neuclir/1/zh neuclir_1_zh; }

pubmed() {
  # PMIDs in article insertion order (articles without a PMID are skipped).
  python3 -c "
import gzip, sqlite3
c = sqlite3.connect('file:/mnt/pubmed/pmdb.db?mode=ro', uri=True)
with gzip.open('pubmed.txt.gz', 'wt', compresslevel=6) as f:
    for (pmid,) in c.execute('select pmid from articles where pmid is not null order by id'):
        f.write(pmid + '\n')"
}

arxiv() {
  # arXiv IDs in order of their first harvested revision (old- and new-style IDs interleave).
  python3 -c "
import gzip, sqlite3
c = sqlite3.connect('file:/mnt/arxiv/arxiv.db?mode=ro', uri=True)
with gzip.open('arxiv.txt.gz', 'wt', compresslevel=6) as f:
    for (aid,) in c.execute('select arxiv_id from papers order by first_revision_id'):
        f.write(aid + '\n')"
}

summary() {  # ID counts and checksums of every list
  for f in *.txt.gz; do echo "$f $(zcat "$f" | wc -l)" & done | sort > counts.txt
  sha256sum *.txt.gz > SHA256SUMS
  rm -rf parts
}

ALL="msmarco-passage beir_fever clueweb12 clueweb22-b-en msmarco-passage-v2 msmarco-v2.1-segmented gov2 car-v2.0 neuclir pubmed arxiv summary"
for name in ${@:-$ALL}; do
  echo "$(date +%T) $name start" >&2
  "$name"
  echo "$(date +%T) $name done" >&2
done
