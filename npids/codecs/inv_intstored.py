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


class InvIntStored:
    NAME = 'intstored'
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
                this_mask2 = isnumeric(target_nums)
                target_nums = target_nums[this_mask2].astype(int)
                this_mask[np.where(this_mask)[0][~this_mask2]] = 0
                indexes = np.searchsorted(codec.ctxt, target_nums)
                indexes[indexes >= codec.ctxt.shape[0]] = -1
                this_mask3 = codec.ctxt[indexes] == target_nums
                indexes = indexes[this_mask3]
                this_mask[np.where(this_mask)[0][~this_mask3]] = 0
                try:
                    result[this_mask] = indexes + offset
                except:
                    import pdb; pdb.set_trace()
                    result[this_mask] = indexes + offset
                mask = mask | this_mask
            if mask.all():
                break
        return result

    @staticmethod
    def build(fwd, writer):
        writer.write_header(1, len(fwd), {
            'format': 'intstored',
            'count': len(fwd),
        })

    @staticmethod
    def condition(fwd):
        # all fwds must be intstored
        if not all(isinstance(c.fmt, codecs.fwd['intstored']) for c in fwd.codecs):
            return False
        # all must be exclusively increasing
        if not all((c.ctxt[1:] > c.ctxt[:-1]).all() for c in fwd.codecs):
            return False
        return True
