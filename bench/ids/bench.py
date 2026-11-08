"""Benchmark npids on the doc ID lists collected by collect.sh, streaming them (no list of all IDs).

Usage: python bench.py [--v1-max N] [--out DIR] list.txt.gz [...]

For each list: build a v2 file straight from the gzip stream (and a v1 file with npids.legacy if the
list has at most --v1-max IDs), recording build time and peak RSS growth. A random sample of 10k
positions is read during a separate pass over the stream; it is used to check lookups and to time
10k random forward lookups (as list[str] and as raw buffers) and 10k inverse lookups (warm cache).
Prints one markdown row per system and, to stderr, the v2 file's describe().
"""
import argparse, gzip, os, resource, sys, time
import numpy as np
import npids
from npids import legacy

SAMPLE = 10_000


def stream(path):
    with gzip.open(path, 'rt', encoding='utf8', newline='\n') as f:
        for line in f:
            yield line[:-1] if line.endswith('\n') else line


def count_and_sample(path, rng):
    n = sum(1 for _ in stream(path))
    pos = np.sort(rng.choice(n, size=min(SAMPLE, n), replace=False))
    ids, k = [], 0
    for i, x in enumerate(stream(path)):
        if k < len(pos) and i == pos[k]:
            ids.append(x)
            k += 1
            if k == len(pos):
                break
    return n, pos, ids


def rss_gb():
    return resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024 ** 2  # KB on Linux


def fmt_size(n):
    for unit in ['B', 'KB', 'MB', 'GB']:
        if n < 1024 or unit == 'GB':
            return f'{n:.1f}{unit}' if unit != 'B' else f'{n}B'
        n /= 1024


def timed(fn, reps=3):
    fn()
    best = float('inf')
    for _ in range(reps):
        t = time.perf_counter()
        r = fn()
        best = min(best, time.perf_counter() - t)
    return best, r


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('lists', nargs='+')
    ap.add_argument('--v1-max', type=int, default=150_000_000)
    ap.add_argument('--out', default='/tmp/npids_bench')
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    rng = np.random.default_rng(0)
    print('| list | IDs | version | build | peak RSS growth | size | bytes/ID | fwd 10k (str) | fwd 10k (raw) | inv 10k |')
    print('|---|---:|---|---:|---:|---:|---:|---:|---:|---:|', flush=True)
    for path in args.lists:
        name = os.path.basename(path).replace('.txt.gz', '')
        t = time.perf_counter()
        n, pos, sample = count_and_sample(path, rng)
        read_s = time.perf_counter() - t
        print(f'{name}: {n:,} ids, two read passes took {read_s:.0f}s', file=sys.stderr, flush=True)
        shuffle = rng.permutation(len(pos))
        q_idx, q_ids = pos[shuffle], [sample[i] for i in shuffle]
        systems = [('v2', npids.Lookup)] + ([('v1', legacy.Lookup)] if n <= args.v1_max else [])
        for system, cls in systems:
            p = f'{args.out}/{name}.{system}'
            if os.path.exists(p):
                os.remove(p)
            rss0 = rss_gb()
            t = time.perf_counter()
            if system == 'v2':
                with npids.Lookup.builder(p) as b:
                    batch = []
                    for x in stream(path):
                        batch.append(x)
                        if len(batch) == 65536:
                            b.add_many(batch)
                            batch = []
                    b.add_many(batch)
            else:
                legacy.Lookup.build(stream(path), p, return_self=False)
            build = time.perf_counter() - t
            rss = rss_gb() - rss0
            lk = cls(p)
            fwd = lk.fwd.lookup(q_idx.tolist())
            assert list(fwd) == q_ids, f'{name} {system}: forward lookups differ'
            inv = lk.inv.lookup(q_ids)
            assert list(lk.fwd.lookup(list(inv))) == q_ids, f'{name} {system}: inverse lookups wrong'
            t_fwd, _ = timed(lambda: lk.fwd.lookup(q_idx.tolist()))
            t_inv, _ = timed(lambda: lk.inv.lookup(q_ids))
            raw = f"{timed(lambda: lk._reader.get_many_raw(q_idx.astype(np.int64)))[0] * 1e3:.1f}ms" if system == 'v2' else '-'
            size = os.path.getsize(p)
            print(f'| {name} | {n:,} | {system} | {build:.0f}s | +{rss:.1f}GB | {fmt_size(size)} | {size / n:.2f} '
                  f'| {t_fwd * 1e3:.1f}ms | {raw} | {t_inv * 1e3:.1f}ms |', flush=True)
            if system == 'v2':
                lines = lk._reader.describe().splitlines()
                inv = next(i for i, l in enumerate(lines) if l.strip() == 'inv')
                fwd = lines[:min(inv, 22)] + (['        ...'] if inv > 22 else [])
                print('\n'.join('    ' + l for l in fwd + lines[inv:inv + 21]), file=sys.stderr, flush=True)
            lk.close()
            del lk
            os.remove(p)


if __name__ == '__main__':
    main()
