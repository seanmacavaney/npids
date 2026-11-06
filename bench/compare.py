"""Compare npids v2 with the legacy (v1) format on the test samples.

Usage: python bench/compare.py [sample ...]   (from the repo root, with npids installed)
Reports build time, file size, and the time for 10k random forward / inverse lookups (warm cache).
"""
import gzip, os, sys, tempfile, time
import numpy as np
import npids
from npids import legacy

SAMPLES = sys.argv[1:] or sorted(f[:-7] for f in os.listdir('test/samples') if f.endswith('.txt.gz'))

def fmt_size(n):
    for unit in ['B', 'KB', 'MB', 'GB']:
        if n < 1024 or unit == 'GB':
            return f'{n:.1f}{unit}' if unit != 'B' else f'{n}B'
        n /= 1024

def timed(fn):
    t = time.perf_counter(); r = fn(); return time.perf_counter() - t, r

rng = np.random.default_rng(0)
print(f'| sample | n | system | build | size | fwd 10k | inv 10k |')
print(f'|---|---:|---|---:|---:|---:|---:|')
for name in SAMPLES:
    with gzip.open(f'test/samples/{name}.txt.gz', 'rt') as f:
        ids = [l.rstrip('\n') for l in f]
    idx = rng.integers(0, len(ids), 10_000)
    qs = [ids[i] for i in idx]
    with tempfile.TemporaryDirectory() as d:
        for system, build, open_ in [
            ('v1', lambda p: legacy.Lookup.build(ids, p, return_self=False), legacy.Lookup),
            ('v2', lambda p: npids.Lookup.build(ids, p, return_self=False), npids.Lookup),
        ]:
            p = f'{d}/{system}'
            tb, _ = timed(lambda: build(p))
            lk = open_(p)
            lk.fwd[idx]; lk.inv[qs]  # warm
            tf, res_f = timed(lambda: lk.fwd[idx])
            ti, res_i = timed(lambda: lk.inv[qs])
            ok = all(a == ids[i] for a, i in zip(res_f, idx))
            print(f'| {name} | {len(ids):,} | {system} | {tb:.2f}s | {fmt_size(os.path.getsize(p))} | {tf*1000:.1f}ms | {ti*1000:.1f}ms |{"" if ok else " WRONG"}', flush=True)
            if system == 'v2':
                print('\n'.join('    ' + l for l in lk.describe().splitlines()[1:]), file=sys.stderr)
