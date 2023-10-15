import re
import numpy as np


class FwdIntSequence:
    NAME = 'intsequence'
    def __init__(self, prefix=None, start=None):
        self.prefix = prefix
        self.start = start
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
        number = int(match.group())
        if self.start is None:
            self.start = number
        else:
            if self.seed_prev + 1 != number:
                return False
        if self.prefix + str(number) != id:
            return False
        self.seed_prev = number
        return True

    def reset(self):
        self.prefix = None
        self.start = None
        self.seed_prev = None
        self.encode_prev = None

    def config(self):
        return {'prefix': self.prefix, 'start': self.start}

    def size(self):
        return 0

    def encode(self, id):
        match = self.prefix_re.search(id)
        if not match:
            return None
        prefix = id[:match.span()[0]]
        if prefix != self.prefix:
            return None
        number = int(match.group())
        if self.encode_prev is None:
            if number != self.start:
                return None
        else:
            if self.encode_prev + 1 != number:
                return None
            if self.prefix + str(number) != id:
                return None
        self.encode_prev = number
        return b''

    def build_context(self, mmp, count):
        return count

    def lookup(self, idxs: np.array, ctxt) -> np.array:
        docnos = idxs
        if self.start:
            docnos = docnos + self.start
        if self.prefix:
            docnos = np.char.mod(f'{self.prefix}%i'.encode(), docnos)
        return docnos.astype('S')

    def iterator(self, ctxt):
        count = ctxt
        for i in range(self.start, self.start+count):
            item = str(i)
            if self.prefix:
                item = self.prefix + item
            yield item

    # def iterator(self, ctxt):
    #     count = ctxt
    #     BATCH_SIZE = 1000
    #     for i in range(self.start, self.start+count):
    #         batch = np.arange(i, min(i+BATCH_SIZE, self.start+count))
    #         if self.prefix:
    #             batch = np.char.mod(f'{self.prefix}%i'.encode(), batch)
    #         else:
    #             batch = batch.astype('S')
    #         yield from batch
