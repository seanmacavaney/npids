import re
import numpy as np


class FwdIntSequencePad:
    NAME = 'intsequencepad'
    def __init__(self, prefix=None, start=None, pad=None):
        self.prefix = prefix
        self.start = start
        self.pad = pad
        self.seed_prev = None
        self.encode_prev = None
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
        if self.pad is None:
            self.pad = len(match.group())
        if self.pad != len(match.group()):
            return False
        number = int(match.group())
        if self.start is None:
            self.start = number
        else:
            if self.seed_prev + 1 != number:
                return False
        fmt_str = f'0{self.pad}d'
        if self.prefix + format(number, fmt_str) != id:
            return False
        self.seed_prev = number
        return True

    def reset(self):
        self.prefix = None
        self.start = None
        self.pad = None
        self.seed_prev = None
        self.encode_prev = None

    def config(self):
        return {'prefix': self.prefix, 'start': self.start, 'pad': self.pad}

    def size(self):
        return 0

    def encode(self, id):
        match = self.prefix_re.search(id)
        if not match:
            return None
        prefix = id[:match.span()[0]]
        if prefix != self.prefix:
            return None
        if len(match.group()) != self.pad:
            return None
        number = int(match.group())
        if self.encode_prev is None:
            if number != self.start:
                return None
        else:
            if self.encode_prev + 1 != number:
                return None
            if self.prefix + format(number, f'0{self.pad}d') != id:
                return None
        self.encode_prev = number
        return b''

    def build_context(self, mmp, count):
        return count

    def lookup(self, idxs: np.array, ctxt) -> np.array:
        docnos = idxs
        if self.start:
            docnos = docnos + self.start
        docnos = np.char.mod(f'{self.prefix}%0{self.pad}d'.encode(), docnos)
        return docnos.astype('S')

    def iterator(self, ctxt):
        count = ctxt
        fmt_str = f'0{self.pad}d'
        for i in range(self.start, self.start+count):
            item = format(i, fmt_str)
            if self.prefix:
                item = self.prefix + item
            yield item
