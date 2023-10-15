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


if __name__ == '__main__':
    unittest.main()
