import os
import tempfile
import threading
import unittest

import numpy as np

import npids
from npids import Lookup, legacy


class TestV2(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = self._tmp.name

    def tearDown(self):
        self._tmp.cleanup()

    def path(self, name='x'):
        return os.path.join(self.dir, name)

    def test_concurrent_readers(self):
        # Many threads share one Lookup, starting together so that the lazily-initialised chunk
        # state is first touched concurrently. On free-threaded Python this runs without a GIL.
        ids = ([f'D{i}' for i in range(70_000)]  # derived inverse (sorted sequence)
               + [f'q{(i * 7919) % 100_003:x}' for i in range(70_000)])  # hashed inverse
        Lookup.build(ids, self.path(), chunk_bits=12)
        lk = Lookup(self.path())
        n_threads, barrier, errors = 8, threading.Barrier(8), []

        def work(seed):
            try:
                rng = np.random.default_rng(seed)
                barrier.wait()
                for _ in range(20):
                    idxs = rng.integers(0, len(ids), 500)
                    expected = [ids[i] for i in idxs]
                    if lk.fwd[idxs].tolist() != expected:
                        raise AssertionError('fwd mismatch')
                    if lk.inv[np.array(expected)].tolist() != idxs.tolist():
                        raise AssertionError('inv mismatch')
                    i = int(idxs[0])
                    if lk.fwd[i] != ids[i] or lk.inv[ids[i]] != i:
                        raise AssertionError('scalar mismatch')
            except BaseException as e:
                errors.append(e)

        threads = [threading.Thread(target=work, args=(s,)) for s in range(n_threads)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        self.assertEqual(errors, [])

    def test_concurrent_builders(self):
        def build(k):
            Lookup.build([f'{k}-{i}' for i in range(20_000)], self.path(f'b{k}'))

        threads = [threading.Thread(target=build, args=(k,)) for k in range(6)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        for k in range(6):
            lk = Lookup(self.path(f'b{k}'))
            self.assertEqual(len(lk), 20_000)
            self.assertEqual(lk.inv[f'{k}-123'], 123)

    def test_fwd_forms(self):
        ids = [f'doc{i}' for i in range(100)]
        lk = Lookup.build(ids, self.path())
        self.assertEqual(lk.fwd[3], 'doc3')
        self.assertEqual(lk.fwd[-1], 'doc99')
        self.assertEqual(lk.fwd[np.int64(-2)], 'doc98')
        self.assertEqual(lk.fwd[10:13], ['doc10', 'doc11', 'doc12'])
        self.assertEqual(lk.fwd[::-40], ['doc99', 'doc59', 'doc19'])
        self.assertEqual(lk[[1, 2]], ['doc1', 'doc2'])
        arr = lk.fwd[np.array([[0, 1], [2, -1]])]
        self.assertEqual(arr.shape, (2, 2))
        self.assertEqual(arr.dtype.kind, 'U')
        self.assertEqual(arr.tolist(), [['doc0', 'doc1'], ['doc2', 'doc99']])
        self.assertEqual(lk.fwd.lookup([5], as_bytes=True), [b'doc5'])
        self.assertEqual(lk.fwd[np.array([], dtype=np.int64)].shape, (0,))
        with self.assertRaises(IndexError):
            lk.fwd[100]
        with self.assertRaises(IndexError):
            lk.fwd[np.array([-101])]
        self.assertEqual(list(lk), ids)
        self.assertTrue(99 in lk and -1 not in lk and 100 not in lk)

    def test_inv_forms(self):
        ids = [f'doc{i}' for i in range(100)]
        lk = Lookup.build(ids, self.path())
        self.assertEqual(lk.inv['doc7'], 7)
        self.assertEqual(lk.inv[b'doc7'], 7)
        self.assertEqual(lk['doc7'], 7)
        self.assertEqual(lk[['doc1', 'nope', 'doc2']], [1, -1, 2])
        self.assertEqual(lk[(x for x in ['doc3'])], [3])
        res = lk.inv[np.array([['doc1', 'doc2'], ['zzz', 'doc0']])]
        self.assertEqual(res.tolist(), [[1, 2], [-1, 0]])
        with self.assertRaises(KeyError):
            lk.inv['nope']
        with self.assertRaises(LookupError):
            lk['nope']
        self.assertIsNone(lk.inv.get('nope'))
        self.assertTrue('doc5' in lk and 'nope' not in lk)

    def test_tricky_ids(self):
        ids = ['ab\x00', 'ab', '', 'é', '日本語', 'ab\x00', '\x00']
        lk = Lookup.build(ids, self.path())
        self.assertEqual(list(lk), ids)
        self.assertEqual(lk.inv['ab\x00'], 0)
        self.assertEqual(lk.inv[''], 2)
        self.assertEqual(lk.inv['日本語'], 4)
        lk2 = Lookup.build([b'\xff\xfe', b'ok'], self.path('bytes'))
        self.assertEqual(lk2.fwd.lookup(0, as_bytes=True), b'\xff\xfe')
        self.assertEqual(lk2.inv[b'\xff\xfe'], 0)
        with self.assertRaises(UnicodeDecodeError):
            lk2.fwd[0]

    def test_append_after_inverse(self):
        p = self.path()
        Lookup.build([f'D{i}' for i in range(1000)], p, return_self=False)
        Lookup.build([f'D{i}' for i in range(1000, 1300)], p, return_self=False)
        lk = Lookup(p)
        self.assertEqual(len(lk), 1300)
        for i in [0, 999, 1000, 1299]:
            self.assertEqual(lk.inv[f'D{i}'], i)
            self.assertEqual(lk.fwd[i], f'D{i}')
        Lookup.build(['X', 'D5'], p, return_self=False)
        lk = Lookup(p)
        self.assertEqual(lk.inv['X'], 1300)
        self.assertEqual(lk.inv['D5'], 5)  # first occurrence
        lk.verify(deep=True)
        Lookup.compact(p)
        lk = Lookup(p)
        self.assertEqual(lk.describe().count('phf') + lk.describe().count('derived'), 1)
        self.assertEqual(lk.inv['X'], 1300)
        lk.verify(deep=True)

    def test_builder_exception_commits_nothing(self):
        p = self.path()
        Lookup.build(['a', 'b'], p, return_self=False)
        with self.assertRaises(ZeroDivisionError):
            with Lookup.builder(p) as b:
                b.add('c')
                1 / 0
        self.assertEqual(list(Lookup(p)), ['a', 'b'])
        with Lookup.builder(p) as b:
            b.add('c')
            self.assertEqual(len(b), 3)
        self.assertEqual(list(Lookup(p)), ['a', 'b', 'c'])

    def test_compact_keeps_long_sequential_runs(self):
        p = self.path()
        Lookup.build([str(i) for i in range(200_000)], p, return_self=False)
        Lookup.build(['X', 'Y'], p, return_self=False)
        Lookup.compact(p)
        lk = Lookup(p)
        # The chunk holding X and Y falls back to {str}; the rest stays sequential and DERIVED.
        self.assertIn('[0, 196608) derived {dec}', lk.describe())
        self.assertLess(os.path.getsize(p), 60_000, lk.describe())
        self.assertEqual(lk.inv['199999'], 199999)
        self.assertEqual(lk.inv['Y'], 200_001)
        lk.verify(deep=True)

    def test_meta(self):
        lk = Lookup.build(['a'], self.path(), meta={'dataset': 'x', 'n': 1})
        self.assertEqual(lk.meta, {'dataset': 'x', 'n': 1})
        self.assertIsNone(Lookup.build(['a'], self.path('y')).meta)

    def test_no_inverse(self):
        lk = Lookup.build(['a', 'b'], self.path(), build_inv=False)
        self.assertEqual(lk.fwd[1], 'b')
        with self.assertRaises(RuntimeError):
            lk.inv['a']

    def test_legacy_read_and_upgrade(self):
        p = self.path()
        ids = [f'D{i}' for i in range(500)] + ['X1', 'Y2']
        legacy.Lookup.build(ids, p, return_self=False)
        lk = Lookup(p)
        self.assertIsInstance(lk, legacy.Lookup)
        self.assertIsInstance(lk, Lookup)
        self.assertEqual(lk.inv['D7'], 7)
        with self.assertRaises(ValueError):
            Lookup.build(['more'], p)
        lk.close()
        npids.upgrade(p)
        lk = Lookup(p)
        self.assertIsInstance(lk, Lookup)
        self.assertNotIsInstance(lk, legacy.Lookup)
        self.assertEqual(list(lk), ids)
        self.assertEqual(lk.inv['Y2'], 501)

    def test_legacy_append_bug_fixed(self):
        # npids < 0.2 could not find IDs appended after the inverse index was built.
        p = self.path()
        legacy.Lookup.build([f'D{i}' for i in range(1000)], p, return_self=False)
        legacy.Lookup.build([f'D{i}' for i in range(1000, 1300)], p, return_self=False)
        lk = legacy.Lookup(p)
        self.assertEqual(lk.inv['D1100'], 1100)


if __name__ == '__main__':
    unittest.main()
