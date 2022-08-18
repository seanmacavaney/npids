import numpy as np
from npids import codecs


def slice_vectorized(a, start):
    b = a.view('S1').reshape(len(a), -1)[:, start:]
    return np.ascontiguousarray(b).view(f'S{b.shape[1]}').reshape(len(a))


def isnumeric(a):
    # much faster than np.char.isnumeric because it doesn't need to be converted to unicode
    b = a.view(np.uint8).reshape(len(a), -1)
    mask = (b >= 48) & (b <= 57) | (b == 0) # 48=0, 57=9, 0=NUL
    return mask.all(axis=1)


class InvIntSequence:
    NAME = 'intsequence'
    def __init__(self, count: int, prefix: str, start: int):
        self.count = count
        self.prefix = prefix.encode()
        self.start = start
        self.fwd = None

    def _lookup(self, docnos: np.array) -> np.array:
        result = np.full(docnos.shape, -1)
        if self.prefix:
            mask = docnos.astype(f'S{len(self.prefix)}') == self.prefix
            docnos = slice_vectorized(docnos, len(self.prefix))
            mask = mask & isnumeric(docnos)
        else:
            mask = isnumeric(docnos)

        result[mask] = docnos[mask].astype(int)
        if self.start:
            result[mask] -= self.start
        invalid_mask = (result < 0) | (result >= self.count)
        if invalid_mask.any():
            result[invalid_mask] = -1
        return result

    @staticmethod
    def build(fwd, writer):
        assert len(fwd.codecs) == 1 and isinstance(fwd.codecs[0].fmt, codecs.fwd['intsequence'])
        writer.write_header(1, len(fwd), {
            'format': 'intsequence',
            'prefix': fwd.codecs[0].fmt.prefix,
            'start': fwd.codecs[0].fmt.start,
            'count': len(fwd),
        })
