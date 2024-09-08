from npids import Lookup
import ir_datasets
import unittest
import tempfile

logger = ir_datasets.log.easy()

class TestBugs(unittest.TestCase):
    def test_intparse(self):
        with tempfile.TemporaryDirectory() as tdir:
            # This previously caused bugs with fwd_intsequence and fwd_insorted
            # since it would expect a capture group to be parsable as an int.
            # This test captures this case.
            Lookup.build(['ABC', '123'], f'{tdir}/docnos')

    def test_intstored_noprefix_lookup(self):
        with tempfile.TemporaryDirectory() as tdir:
            # There was a bug for intstored where if there was no prefix, lookups would fail
            lookup = Lookup.build(['2', '1'], f'{tdir}/docnos')
            assert lookup.inv['2'] == 0
            assert lookup.inv['1'] == 1
            # Make sure the fix didn't break the prefixed case
            lookup = Lookup.build(['D2', 'D1'], f'{tdir}/docnos2')
            assert lookup.inv['D2'] == 0
            assert lookup.inv['D1'] == 1


if __name__ == '__main__':
    unittest.main()
