import os
from npids import Lookup
import ir_datasets
import unittest
import tempfile
import random
import numpy as np

logger = ir_datasets.log.easy()

class TestDatasets(unittest.TestCase):
    def test_vaswani(self):
        self._test_dataset('vaswani', invalid_docnos=['0', '-1', '11430', '9999999', 'ab', 'a1'])

    def test_msmarco_document(self):
        self._test_dataset('msmarco-document')

    def test_msmarco_passage(self):
        self._test_dataset('msmarco-passage')

    def test_disks45_nocr(self):
        self._test_dataset('disks45/nocr')

    def test_hc4_fa(self):
        self._test_dataset('hc4/fa')

    def _test_dataset(self, dataset, invalid_docnos=None, limit=None):
        with self.subTest(dataset):
            docs = ir_datasets.load(dataset).docs
            if limit:
                docs = docs[:limit]
            docnos = [d.doc_id for d in logger.pbar(docs, desc=f'loading {dataset} docnos')]
            doc_idxs = np.arange(len(docnos))
            doc_idxs_shuf = doc_idxs.copy()
            np.random.shuffle(doc_idxs_shuf)
            docnos_shuf = [docnos[i] for i in doc_idxs_shuf]
            with tempfile.TemporaryDirectory() as tdir:
                lookup = Lookup.build(iter(docnos), f'{tdir}/docnos')
                fsize = os.path.getsize(f'{tdir}/docnos')
                baseline = max(len(d) for d in docnos) * len(docnos)
                for i in range(100):
                    docno, docidx = docnos_shuf[i], doc_idxs_shuf[i]
                    self.assertEqual(docno, lookup[docidx])
                    self.assertEqual(docidx, lookup[docno])
                self.assertEqual(doc_idxs_shuf[:100].tolist(), lookup[docnos_shuf[:100]])
                self.assertEqual(doc_idxs_shuf[:100].tolist(), lookup[(x for x in docnos_shuf[:100])])
                self.assertTrue((doc_idxs_shuf[:100] == lookup[np.array(docnos_shuf[:100])]).all())
                if invalid_docnos is not None:
                    for docno in invalid_docnos:
                        with self.subTest(f'dataset={dataset} invalid_docno={docno}'):
                            with self.assertRaises(LookupError):
                                lookup[docno]


if __name__ == '__main__':
    unittest.main()
