"""Piecewise-arithmetic columns (RUNS, FORMAT.md): ClueWeb22-style IDs, mixed chunks, appends, and
reading files written before RUNS existed."""
import os
import tempfile
import unittest

import numpy as np

from npids import Lookup

HERE = os.path.dirname(os.path.abspath(__file__))


def cw22_ids(n, seed=0, min_run=5_000, max_run=40_000):
    """`clueweb22-en%04d-%02d-%05d`: the last field counts up within a run, then restarts at 0 when the
    middle field increments (wrapping after 48, which increments the first)."""
    rng = np.random.default_rng(seed)
    out, a, b = [], 0, 0
    while len(out) < n:
        run = min(int(rng.integers(min_run, max_run + 1)), n - len(out))
        out.extend(f'clueweb22-en{a:04d}-{b:02d}-{i:05d}' for i in range(run))
        b += 1
        if b == 48:
            a, b = a + 1, 0
    return out


def fixture_ids():
    """The IDs of test/data/npids-1.0.1.npids, written by npids 1.0.1 with chunk_bits=8 in two sessions
    (the first 768 IDs, then the rest)."""
    rng = np.random.default_rng(42)
    ids = cw22_ids(1500, seed=1, min_run=50, max_run=400)
    ids += [f'D{x}' for x in rng.permutation(1000) + 7]
    ids += [str(i) for i in range(700)]
    ids += [f'{x:016x}' for x in rng.integers(0, 2**63, 300)]
    ids += ['title %d: %s' % (i, 'x' * (i % 13)) for i in range(400)]
    return ids


class TestRuns(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = self._tmp.name

    def tearDown(self):
        self._tmp.cleanup()

    def path(self, name='x'):
        return os.path.join(self.dir, name)

    def check(self, lk, ids, sample=None):
        idx = np.arange(len(ids)) if sample is None else np.random.default_rng(0).integers(0, len(ids), sample)
        self.assertEqual(list(lk.fwd[idx]), [ids[i] for i in idx])
        offsets, data = lk._reader.get_many_raw(idx)
        self.assertEqual([data[offsets[k]:offsets[k + 1]].decode() for k in range(len(idx))], [ids[i] for i in idx])
        first = {}
        for i, x in enumerate(ids):
            first.setdefault(x, i)
        self.assertEqual(list(lk.inv[[ids[i] for i in idx]]), [first[ids[i]] for i in idx])

    def test_cw22_size(self):
        ids = cw22_ids(1 << 20)
        Lookup.build(ids, self.path())
        with Lookup(self.path()) as lk:
            # 16 chunks of a few hundred bytes (npids 1.0.0 needed ~2.2 bytes per ID).
            self.assertLess(os.path.getsize(self.path()), 8_000, lk.describe())
            self.assertIn('derived', lk.describe())
            self.check(lk, ids, sample=50_000)
            # Run boundaries, and IDs just past the end of each run.
            starts = [i for i, x in enumerate(ids) if x.endswith('-00000')]
            for s in starts[1:]:
                self.assertEqual(lk.inv[ids[s - 1]], s - 1)
                self.assertEqual(lk.inv[ids[s]], s)
                prefix, last = ids[s - 1].rsplit('-', 1)
                self.assertIsNone(lk.inv.get(f'{prefix}-{int(last) + 1:05d}'))
            for absent in ['clueweb22-en0000-00-99999', 'clueweb22-en0099-00-00000', 'clueweb22-en0000-00-0000',
                           'clueweb22-en0000-00-00000x', 'clueweb23-en0000-00-00000', 'clueweb22-de0000-00-00000']:
                self.assertIsNone(lk.inv.get(absent), absent)
            lk.verify()

    def test_mixed_chunks(self):
        # Runs, then shuffled IDs, then runs again, with small chunks: some chunks hold run tables,
        # some bit-packed fields, and some both kinds of IDs.
        rng = np.random.default_rng(3)
        a = cw22_ids(3000, seed=1, min_run=100, max_run=900)
        b = [f'clueweb22-en{x // 100000:04d}-{x // 1000 % 100:02d}-{x % 1000:05d}' for x in rng.permutation(10**6)[:2000]]
        c = cw22_ids(3000, seed=2, min_run=10, max_run=60)
        ids = a + b + [x.replace('clueweb22-en', 'clueweb22-en1') for x in c]
        Lookup.build(ids, self.path(), chunk_bits=9)
        with Lookup(self.path()) as lk:
            self.check(lk, ids)
            lk.verify(deep=True)

    def test_appends_across_chunk_boundaries(self):
        ids = cw22_ids(6000, seed=4, min_run=200, max_run=1500)
        rng = np.random.default_rng(5)
        i = 0
        while i < len(ids):
            n = int(rng.integers(1, 900))
            with Lookup.builder(self.path(), chunk_bits=10) as b:
                b.add_many(ids[i:i + n])
            i += n
            with Lookup(self.path()) as lk:
                self.check(lk, ids[:i])
        Lookup.compact(self.path())
        with Lookup(self.path()) as lk:
            self.check(lk, ids)
            lk.verify(deep=True)
            self.assertLess(os.path.getsize(self.path()), 6 * 600 + 2000, lk.describe())

    def test_duplicates_and_decreasing_runs(self):
        # Not increasing, so inverse lookups use a perfect hash; forward lookups still use runs.
        ids = cw22_ids(5000, seed=6, min_run=300, max_run=900)
        ids = ids + ids[::-1][:2000] + ids[:1000]
        Lookup.build(ids, self.path(), chunk_bits=11)
        with Lookup(self.path()) as lk:
            self.check(lk, ids)
            lk.verify(deep=True)

    def test_reads_files_from_1_0_1(self):
        ids = fixture_ids()
        with Lookup(os.path.join(HERE, 'data', 'npids-1.0.1.npids')) as lk:
            self.check(lk, ids)
            lk.verify(deep=True)
        # Appending to it with this version keeps the old chunks and may add run tables.
        p = self.path()
        with open(os.path.join(HERE, 'data', 'npids-1.0.1.npids'), 'rb') as f, open(p, 'wb') as g:
            g.write(f.read())
        more = cw22_ids(3000, seed=9, min_run=300, max_run=900)
        with Lookup.builder(p) as b:
            b.add_many(more)
        with Lookup(p) as lk:
            self.check(lk, ids + more)
            lk.verify(deep=True)


if __name__ == '__main__':
    unittest.main()
