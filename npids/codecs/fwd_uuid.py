import re
import uuid
import numpy as np
from npids.utils import wrap_mmap

def _coerce_bytes(b):
    if hasattr(b, 'tobytes'):
        return b.tobytes()
    return b

class FwdUuid:
    NAME = 'uuid'
    def __init__(self, upper=None, prefix=None):
        self.upper = upper
        self.prefix = prefix
        self.lc_re = re.compile(r'^[0-9a-f]{8}\b-[0-9a-f]{4}\b-[0-9a-f]{4}\b-[0-9a-f]{4}\b-[0-9a-f]{12}$')
        self.uc_re = re.compile(r'^[0-9A-F]{8}\b-[0-9A-F]{4}\b-[0-9A-F]{4}\b-[0-9A-F]{4}\b-[0-9A-F]{12}$')
        self.vuuid = np.vectorize(lambda x: str(uuid.UUID(bytes=_coerce_bytes(x))).encode())

    def seed(self, id):
        if self.prefix is None:
            self.prefix = id[:-36]
        if self.upper is None:
            self.upper = bool(self.uc_re.match(id[-36:]))
        if not id.startswith(self.prefix):
            return False
        return self.encode(id) is not None

    def reset(self):
        self.upper = None
        self.prefix = None

    def size(self):
        return 16 # all uuids are 16 bytes

    def config(self):
        return {'upper': self.upper, 'prefix': self.prefix}

    def encode(self, id):
        if not id.startswith(self.prefix):
            return None
        id = id[-36:]
        r = self.uc_re if self.upper else self.lc_re
        if not r.match(id):
            return None
        return uuid.UUID(id).bytes

    def build_context(self, mmp, count):
        return wrap_mmap(mmp, 'V16')

    def lookup(self, idxs: np.array, ctxt) -> np.array:
        mmp = ctxt
        docnos = self.vuuid(mmp[idxs])
        if self.upper:
            docnos = np.char.upper(docnos)
        if self.prefix:
            docnos = np.char.add(self.prefix, docnos)
        return docnos.astype('S')

    def iterator(self, ctxt):
        mmp = ctxt
        for i in range(mmp.shape[0]):
            docno = str(uuid.UUID(bytes=mmp[i].tobytes()))
            if self.upper:
                docno = docno.upper()
            if self.prefix:
                docno = self.prefix + docno
            yield docno
