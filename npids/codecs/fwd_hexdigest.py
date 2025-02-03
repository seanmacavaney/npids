import re
import numpy as np
from npids.utils import wrap_mmap

def _coerce_bytes(b):
    if hasattr(b, 'tobytes'):
        return b.tobytes()
    return b

class FwdHexDigest:
    NAME = 'hexdigest'
    def __init__(self, upper=None, length=None, prefix=None):
        self.upper = upper
        self.length = length
        self.prefix = prefix
        self.lc_re = re.compile(r'([0-9a-f][0-9a-f])+$')
        self.uc_re = re.compile(r'([0-9A-F][0-9A-F])+$')
        self.vdigest = np.vectorize(lambda x: _coerce_bytes(x).hex().encode())

    def seed(self, id):
        if self.upper is None:
            self.upper = bool(self.uc_re.search(id))
        r = self.uc_re if self.upper else self.lc_re
        match = r.search(id)
        if not match:
            return False
        if self.length is None:
            self.length = len(match.group())
        if self.prefix is None:
            self.prefix = id[:match.start()]
        if not id.startswith(self.prefix) and len(match.group()) != self.length:
            return False
        return self.encode(id) is not None

    def reset(self):
        self.upper = None
        self.prefix = None
        self.length = None

    def size(self):
        return self.length // 2

    def config(self):
        return {'upper': self.upper, 'length': self.length, 'prefix': self.prefix}

    def encode(self, id):
        if id[:-self.length] != self.prefix:
            return None
        r = self.uc_re if self.upper else self.lc_re
        if not r.match(id[-self.length:]):
            return None
        return bytes.fromhex(id[-self.length:])

    def build_context(self, mmp, count):
        return wrap_mmap(mmp, f'V{self.length//2}')

    def lookup(self, idxs: np.array, ctxt) -> np.array:
        mmp = ctxt
        docnos = self.vdigest(mmp[idxs])
        if self.upper:
            docnos = np.char.upper(docnos)
        if self.prefix:
            docnos = np.char.add(self.prefix.encode(), docnos)
        return docnos.astype('S')

    def iterator(self, ctxt):
        mmp = ctxt
        for i in range(mmp.shape[0]):
            docno = mmp[i].tobytes().hex()
            if self.upper:
                docno = docno.upper()
            if self.prefix:
                docno = self.prefix.encode() + docno
            yield docno

    def __repr__(self):
        return f'{self.NAME} [prefix={self.prefix} length={self.length} upper={self.upper}]'
