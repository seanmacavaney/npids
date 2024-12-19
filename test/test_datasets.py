import os
import gzip
from collections import Counter
from npids import Lookup
import ir_datasets
import unittest
import tempfile
import random
import numpy as np

logger = ir_datasets.log.easy()

class TestDatasets(unittest.TestCase):
    def test_beir_dbpedia_entity(self):
        self._test_dataset('beir_dbpedia-entity.txt.gz')

    def test_beir_fever(self):
        self._test_dataset('beir_fever.txt.gz')

    def test_cord19(self):
        self._test_dataset('cord19.txt.gz')

    def test_disks45_nocr(self):
        self._test_dataset('disks45_nocr.txt.gz')

    def test_msmarco_document(self):
        self._test_dataset('msmarco-document.txt.gz', invalid_docnos=['D-1', 'D10000000', 'Dab', 'Da1', '-1', '10000000', 'ab', 'a1'])

    def test_msmarco_passage(self):
        self._test_dataset('msmarco-passage.txt.gz', invalid_docnos=['D-1', 'D10000000', 'Dab', 'Da1', '-1', '10000000', 'ab', 'a1'])

    def test_neuclir_1_fa(self):
        self._test_dataset('neuclir_1_fa.txt.gz')

    def test_vaswani(self):
        self._test_dataset('vaswani.txt.gz', invalid_docnos=['0', '-1', '11430', '9999999', 'ab', 'a1'])

    def test_hexsamples(self):
        self._test_dataset('hexsamples.txt.gz')

    def _test_dataset(self, file, invalid_docnos=None):
        with gzip.open(f'{os.path.dirname(__file__)}/samples/{file}', 'rt') as fin:
            docnos = [d.strip() for d in logger.pbar(fin, desc=f'loading {file} docnos')]
        duplicates = set(did for did, count in Counter(docnos).items() if count > 1)
        doc_idxs = np.arange(len(docnos))
        doc_idxs_shuf = doc_idxs.copy()
        np.random.shuffle(doc_idxs_shuf)
        docnos_shuf = [docnos[i] for i in doc_idxs_shuf]
        with tempfile.TemporaryDirectory() as tdir:
            lookup = Lookup.build(docnos, f'{tdir}/docnos')
            print(lookup.describe())
            self.assertFalse(-1 in lookup)
            self.assertFalse(len(docnos) in lookup)
            for i in range(100):
                docno, docidx = docnos_shuf[i], doc_idxs_shuf[i]
                self.assertEqual(docno, lookup[docidx])
                if docno not in duplicates:
                    self.assertEqual(docidx, lookup[docno])
                self.assertTrue(docno in lookup)
                self.assertTrue(int(docidx) in lookup)
            if len(duplicates) == 0:
                count = random.randrange(1, 1000)
                self.assertEqual(doc_idxs_shuf[:count].tolist(), lookup[docnos_shuf[:count]])
                self.assertEqual(doc_idxs_shuf[:count].tolist(), lookup[(x for x in docnos_shuf[:count])])
                self.assertTrue((doc_idxs_shuf[:count] == lookup[np.array(docnos_shuf[:count])]).all())
            with self.subTest(f'file={file} iter'):
                for a, b in zip(docnos, lookup):
                    self.assertEqual(a, b)
            if invalid_docnos is not None:
                for docno in invalid_docnos:
                    with self.subTest(f'file={file} invalid_docno={docno}'):
                        with self.assertRaises(LookupError):
                            lookup[docno]
                        self.assertFalse(docno in lookup)


if __name__ == '__main__':
    unittest.main()
