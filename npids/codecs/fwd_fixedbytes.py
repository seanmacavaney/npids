import numpy as np
from npids.utils import wrap_mmap


class FwdFixedBytes:
    NAME = 'fixedbytes'
    def __init__(self, prefix=None, length=None):
        self.prefix = (prefix.encode() if prefix is not None else None)
        self.length = length

    def seed(self, id):
        id = id.encode()
        if self.prefix is None:
            self.prefix = id
        while not id.startswith(self.prefix):
            self.prefix = self.prefix[:-1]
        if self.length is None:
            self.length = len(id)
        if len(id) > self.length:
            self.length = len(id)
        return True

    def reset(self):
        self.prefix = None
        self.length = None

    def size(self):
        return self.length - len(self.prefix)

    def config(self):
        return {'length': self.length, 'prefix': self.prefix.decode()}

    def encode(self, id):
        id = id.encode()
        if not id.startswith(self.prefix):
            return None
        if len(id) > self.length:
            return None
        id = id[len(self.prefix):]
        if len(id) < self.length - len(self.prefix):
            id = id + bytes(self.length - len(self.prefix) - len(id))
        return id

    def build_context(self, mmp, count):
        return wrap_mmap(mmp, f'S{self.length-len(self.prefix)}')

    def lookup(self, idxs: np.array, ctxt) -> np.array:
        mmp = ctxt
        docnos = mmp[idxs]
        if self.prefix:
            docnos = np.char.add(self.prefix, docnos)
        return docnos.astype('S')

    def iterator(self, ctxt):
        mmp = ctxt
        for i in range(mmp.shape[0]):
            docno = mmp[i]
            if self.prefix:
                docno = self.prefix + docno
            yield docno.decode()
