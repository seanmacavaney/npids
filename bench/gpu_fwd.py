"""Check and time npids.cuda forward lookups against the CPU reader.

Usage: python bench/gpu_fwd.py [sample ...]   (needs a CUDA GPU and cupy)
For each sample: builds a v2 file, checks GPU output == CPU output on random batches (including the
first/last IDs), then times batches of B queries x k=1000 random indices already on the device.
"""
import gzip, sys, tempfile, time
import numpy as np
import cupy as cp
import npids
from npids.cuda import DeviceLookup

SAMPLES = sys.argv[1:] or ['msmarco-passage', 'msmarco-document', 'neuclir_1_fa', 'beir_fever', 'disks45_nocr', 'cord19', 'hexsamples']
K = 1000
rng = np.random.default_rng(0)
print(f'GPU: {cp.cuda.runtime.getDeviceProperties(0)["name"].decode()}')
print('| sample | B | GPU (result on device) | GPU + copy to host | CPU raw, 1 thread | per query: GPU + copy | per query: CPU |')
print('|---|---:|---:|---:|---:|---:|---:|')
for name in SAMPLES:
    ids = [l.rstrip('\n') for l in gzip.open(f'test/samples/{name}.txt.gz', 'rt')]
    lk = npids.Lookup.build(ids, tempfile.mkdtemp() + '/x')
    dl = DeviceLookup(lk)
    n = len(ids)
    for b in [np.array([0, n - 1, 0, n // 2]), rng.integers(0, n, 20000)]:
        got = dl.lookup_list(b)
        exp = [ids[i] for i in b]
        assert got == exp, (name, [(g, e) for g, e in zip(got, exp) if g != e][:5])
    r = lk._reader
    for B in [1, 8, 32]:
        batches = [rng.integers(0, n, B * K) for _ in range(200)]
        dev = [cp.asarray(b) for b in batches]
        def timeit(fn, xs):
            for x in xs[:10]:
                fn(x)
            cp.cuda.Device().synchronize()
            t = time.perf_counter()
            for x in xs:
                fn(x)
            cp.cuda.Device().synchronize()
            return (time.perf_counter() - t) / len(xs) * 1e3
        g_dev = timeit(dl.lookup, dev)
        g_host = timeit(dl.lookup_host, dev)
        c = timeit(r.get_many_raw, batches)
        print(f'| {name} | {B} | {g_dev:.3f} ms | {g_host:.3f} ms | {c:.3f} ms | {g_host / B:.3f} ms | {c / B:.3f} ms |', flush=True)
