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


class InvIntSequenceMulti:
    NAME = 'intsequencemulti'
    def __init__(self, count):
        self.count = count
        self.fwd = None

    def _lookup(self, docnos: np.array) -> np.array:
        result = np.full(docnos.shape, -1)
        mask = result != -1
        for codec, offset in zip(self.fwd.codecs, self.fwd.offsets):
            if codec.fmt.prefix:
              this_mask = (docnos.astype(f'S{len(codec.fmt.prefix.encode())}') == codec.fmt.prefix.encode()) & ~mask
            else:
              this_mask = ~mask
            if this_mask.any():
                if codec.fmt.prefix:
                  target_nums = slice_vectorized(docnos[this_mask], len(codec.fmt.prefix.encode()))
                else:
                  target_nums = docnos[this_mask]
                this_mask_2 = isnumeric(target_nums)
                idxs = target_nums[this_mask_2].astype(int)
                this_mask_3 = (idxs >= 0) & (idxs < codec.count)
                idxs = idxs[this_mask_3]
                if codec.fmt.start:
                    idxs -= codec.fmt.start
                this_mask[this_mask] = this_mask_2
                this_mask[this_mask] = this_mask_3
                result[this_mask] = idxs + offset
                mask[this_mask] = True
                if mask.all():
                    break
        return result

    @staticmethod
    def build(fwd, writer):
        writer.write_header(1, len(fwd), {
            'format': 'intsequencemulti',
            'count': len(fwd),
        })

    @staticmethod
    def condition(fwd):
        # all fwds must be intsequence or intsequencepad
        return all(isinstance(c.fmt, (codecs.fwd['intsequence'], codecs.fwd['intsequencepad'])) for c in fwd.codecs)
