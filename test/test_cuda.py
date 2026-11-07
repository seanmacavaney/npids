"""npids.cuda must decode exactly what the CPU reader does. Skipped without cupy and a GPU."""
import os
import tempfile
import unittest

import numpy as np

try:
    import cupy as cp
    cp.cuda.runtime.getDeviceCount()
    from npids.cuda import DeviceLookup
except Exception:  # no cupy or no GPU
    cp = None

from npids import Lookup


@unittest.skipIf(cp is None, 'needs cupy and a CUDA GPU')
class TestCuda(unittest.TestCase):
    def check(self, ids, chunk_bits=16, appends=1, extra_idxs=()):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, 'x')
            step = (len(ids) + appends - 1) // appends
            for a in range(0, len(ids), step):
                Lookup.build(ids[a:a + step], p, chunk_bits=chunk_bits, return_self=False)
            lk = Lookup(p)
            dl = DeviceLookup(lk)
            rng = np.random.default_rng(0)
            idxs = np.concatenate([np.arange(len(ids)), rng.integers(0, len(ids), 5000)]).astype(np.int64)
            offsets, data = dl.lookup_host(idxs)
            b = data.tobytes()
            got = [b[offsets[i]:offsets[i + 1]] for i in range(len(idxs))]
            exp = [ids[i].encode() if isinstance(ids[i], str) else ids[i] for i in idxs]
            self.assertEqual(got, exp)
            # device input of other dtypes, and out-of-range indices
            o2, _ = dl.lookup(cp.asarray(idxs[:10], dtype=cp.int32))
            self.assertEqual(cp.asnumpy(o2).tolist(), offsets[:11].tolist())
            o3, _ = dl.lookup_host(np.array([len(ids), len(ids) + 5, *extra_idxs]))
            self.assertEqual(o3[1] - o3[0], 0)
            self.assertEqual(dl.lookup_host(np.array([], dtype=np.int64))[0].tolist(), [0])

    def test_sequences(self):
        self.check([str(i) for i in range(5000)], chunk_bits=8)
        self.check([f'D{i * 3 + 7}' for i in range(5000)], chunk_bits=8)
        self.check([f'x-{i:06}' for i in range(5000)], chunk_bits=8)

    def test_bitpacked_and_rows(self):
        rng = np.random.default_rng(1)
        self.check([f'D{int(x)}' for x in rng.integers(0, 10**9, 5000)], chunk_bits=8)
        self.check([f'{int(a):016x}{int(b):016X}' for a, b in rng.integers(0, 2**63, (5000, 2))], chunk_bits=8)
        self.check([f'{int(a):08x}-{int(b) & 0xffff:04x}-{int(b) >> 20 & 0xffff:04x}-{int(a) >> 3 & 0xffff:04x}-{int(b):012x}'
                    for a, b in rng.integers(0, 2**40, (5000, 2))], chunk_bits=8)
        self.check([f'doc{int(a)}-{int(b)}' for a, b in rng.integers(0, 10**12, (5000, 2))], chunk_bits=8)

    def test_strings(self):
        rng = np.random.default_rng(2)
        ids = ['', 'ab\x00', '\x00', 'é', '日本語'] + [''.join(chr(97 + int(c)) for c in rng.integers(0, 26, int(n)))
                                                      for n in rng.integers(0, 40, 5000)]
        self.check(ids, chunk_bits=8)
        self.check([b'\xff\xfe', b'ok', b''] * 100, chunk_bits=4)

    def test_mixed_and_appended(self):
        ids = [f'FBIS{i}' for i in range(700)] + [str(i) for i in range(700)] + [f'LA{i:06}' for i in range(700)] + ['x' * 200]
        self.check(ids, chunk_bits=7, appends=5)


if __name__ == '__main__':
    unittest.main()
