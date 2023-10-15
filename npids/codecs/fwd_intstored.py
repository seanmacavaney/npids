import re
import numpy as np
from npids.utils import wrap_mmap


class FwdIntStored:
    NAME = 'intstored'
    def __init__(self, prefix=None, int_bytes=None):
        self.prefix = prefix
        self.int_bytes = int_bytes
        self.prefix_re = re.compile(r'[0-9]+$')

    def seed(self, id):
        match = self.prefix_re.search(id)
        if not match:
            return False
        prefix = id[:match.span()[0]]
        if self.prefix is None:
            self.prefix = prefix
        if self.prefix != prefix:
            return False
        number = int(match.group())
        if self.prefix + str(number) != id:
            return False
        n_bytes = (number.bit_length() + 7) // 8
        if n_bytes > 8:
            return False
        elif n_bytes > 4:
            self.int_bytes = max(8, self.int_bytes or 0)
        elif n_bytes > 2:
            self.int_bytes = max(4, self.int_bytes or 0)
        elif n_bytes > 1:
            self.int_bytes = max(2, self.int_bytes or 0)
        else:
            self.int_bytes = max(1, self.int_bytes or 0)
        return True

    def reset(self):
        self.prefix = None
        self.int_bytes = None

    def config(self):
        return {'prefix': self.prefix, 'int_bytes': self.int_bytes}

    def size(self):
        return self.int_bytes

    def encode(self, id):
        match = self.prefix_re.search(id)
        if not match:
            return None
        prefix = id[:match.span()[0]]
        if prefix != self.prefix:
            return None
        number = int(match.group())
        if number.bit_length() > self.int_bytes * 8:
            return None
        if self.prefix + str(number) != id:
            return None
        return {1: np.uint8, 2: np.uint16, 4: np.uint32, 8: np.uint64}[self.int_bytes](number).tobytes()

    def build_context(self, mmp, count):
        return wrap_mmap(mmp, f'u{self.int_bytes}')

    def lookup(self, idxs: np.array, ctxt) -> np.array:
        mmp = ctxt
        fmt = f'{self.prefix}%i'.encode()
        docnos = np.char.mod(fmt, mmp[idxs])
        return docnos.astype('S')

    def iterator(self, ctxt):
        mmp = ctxt
        for i in range(mmp.shape[0]):
            docno = str(mmp[i])
            if self.prefix:
                docno = self.prefix + docno
            yield docno
