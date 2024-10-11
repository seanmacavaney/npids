from collections.abc import Iterable as Iter
import mmap
import numpy as np
from typing import Iterable
import contextlib
from .builder import FwdLookupBuilder, InvLookupBuilder
from .utils import FileManager, peekable, wrap_mmap
from . import codecs


class Lookup:
    def __init__(self, path):
        self.path = path
        self.fwd, self.inv = self._load()

    def __getitem__(self, idx):
        return self.lookup(idx)

    def lookup(self, idx, as_bytes=False):
        # detect if we're doing a fwd or inv lookup
        mode = None
        detector = idx
        if isinstance(idx, (list, tuple)):
            if len(idx) > 0:
                detector = idx[0]
            else:
                return [] # empty always returns empty
        elif isinstance(idx, (str, bytes)):
            pass
        elif hasattr(idx, 'dtype'):
            pass
        elif isinstance(idx, Iter):
            idx = peekable(idx)
            if idx:
                detector = idx.peek()
            else:
                return [] # empty always returns empty

        if isinstance(detector, (str, bytes)):
            mode = 'inv'
        elif isinstance(detector, (int, slice)):
            mode = 'fwd'
        elif hasattr(detector, 'dtype'):
            if detector.dtype.kind in 'iu':
                mode = 'fwd'
            elif detector.dtype.kind in 'SUO':
                mode = 'inv'

        if mode == 'inv':
            return self.inv.lookup(idx)
        elif mode == 'fwd':
            return self.fwd.lookup(idx, as_bytes=as_bytes)
        raise RuntimeError('Cannot detect input type to route to fwd or inv lookup. Try using .inv or .fwd directly, depending on your needs.')

    def __len__(self) -> int:
        return len(self.fwd)

    def __iter__(self) -> Iterable[str]:
        return iter(self.fwd)

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()

    def close(self):
        if self.fwd is not None:
            self.fwd.close()
            self.fwd = None
        if self.inv is not None:
            self.inv.close()
            self.inv = None

    @staticmethod
    @contextlib.contextmanager
    def builder(path, build_inv=True, min_block=None):
        builder = FwdLookupBuilder(path, min_block=min_block)
        try:
            yield builder
            builder.close()
        except:
            raise
        if build_inv:
            InvLookupBuilder.build(path)

    @staticmethod
    def build(docnos, path, build_inv=True, min_block=None, return_self=True):
        with Lookup.builder(path, build_inv=build_inv, min_block=min_block) as builder:
            for docno in docnos:
                builder.add(docno)
        if return_self:
            return Lookup(path)

    def _load(self):
        with FileManager(self.path, 'r') as f:
            mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
            next_ptr = f.tell()
            start_idx = 0
            parts = []
            inv = None
            while next_ptr != -1:
                f.seek(next_ptr)
                type_id, next_ptr, doc_count, config = f.read_header()
                here = f.tell()
                if type_id == 0:
                    Fmt = codecs.fwd[config.pop('format')]
                    part = FormatFwdBlock(Fmt(**config), mm[here:None if next_ptr == -1 else next_ptr], doc_count)
                    parts.append((start_idx, part))
                    start_idx += len(part)
                elif type_id == 1:
                    if config['format'] == 'hash':
                        count = (1 << config['hash_bits']) + 1
                        hashes = wrap_mmap(mm[here:here+count*np.dtype(np.uint32).itemsize], np.uint32)
                        here += hashes.nbytes
                        dids = wrap_mmap(mm[here:here+doc_count*np.dtype(np.uint32).itemsize], np.uint32)
                        inv = codecs.inv['hash'](hashes, dids, config['hash_bits'])
                    elif config['format'] == 'intsequence':
                        config.pop('format')
                        inv = codecs.inv['intsequence'](**config)
                    elif config['format'] == 'intsequencemulti':
                        config.pop('format')
                        inv = codecs.inv['intsequencemulti'](**config)
                    elif config['format'] == 'intstored':
                        config.pop('format')
                        inv = codecs.inv['intstored'](**config)
        fwd = FwdLookup([p[1] for p in parts], np.array([p[0] for p in parts]))
        if inv is not None:
            inv.fwd = fwd
            inv = InvLookup(inv)
        return fwd, inv


class FormatFwdBlock:
    def __init__(self, fmt, mmp, count):
        self.fmt = fmt
        self.ctxt = fmt.build_context(mmp, count)
        self.count = count

    def __getitem__(self, idx):
        if isinstance(idx, (int, np.int64, np.int32)):
            return self._lookup(np.array([idx])).item()
        if isinstance(idx, (list, tuple)):
            return self._lookup(np.array(idx)).tolist()
        return self._lookup(idx)

    def _lookup(self, idxs: np.array, as_bytes=False) -> np.array:
        return self.fmt.lookup(idxs, self.ctxt)

    def close(self):
        pass

    def __iter__(self):
        return iter(self.fmt.iterator(self.ctxt))

    def __len__(self):
        return self.count


class FwdLookup:
    def __init__(self, codecs, offsets):
        self.codecs = codecs
        self.offsets = offsets
        self._count = sum(len(c) for c in self.codecs)

    def __getitem__(self, keys):
        return self.lookup(keys)

    def __iter__(self):
        for codec in self.codecs:
            yield from codec

    def lookup(self, idxs, as_bytes=False):
        out_format = None
        idxs_inp = None
        if isinstance(idxs, (int,)):
            out_format = 'single'
            idxs_inp = np.array([idxs], dtype=np.uint32)
        elif hasattr(idxs, 'dtype'):
            if idxs.shape == tuple():
                out_format = 'single'
                idxs_inp = np.array([idxs], dtype=np.uint32)
            else:
                out_format = 'numpy'
                idxs_inp = np.array(idxs, dtype=np.uint32)
                out_shape = idxs_inp.shape
                idxs_inp = idxs_inp.reshape(-1)
        elif isinstance(idxs, (list, tuple)):
            out_format = 'list'
            idxs_inp = np.array(idxs, dtype=np.uint32)
        # TODO: iterables, np.ints, etc.?

        if len(self.codecs) == 1:
            result = self.codecs[0]._lookup(idxs_inp)
        else:
            handlers, inv = np.unique(np.searchsorted(self.offsets, idxs_inp, 'right') - 1, return_inverse=True)
            docnos_and_masks = []
            for i, h in enumerate(handlers):
                mask = inv == i
                these_docnos = self.codecs[h]._lookup(idxs_inp[mask] - self.offsets[h])
                docnos_and_masks.append((these_docnos, mask))
            if len(docnos_and_masks) == 1:
                result = docnos_and_masks[0][0] # short circuit -- no need to merge them since they all came from one part
            else:
                max_docno_size = max((d.dtype for d, m in docnos_and_masks), key=lambda d: d.itemsize)
                result = np.empty(idxs_inp.shape, dtype=max_docno_size)
                for d, m in docnos_and_masks:
                    result[m] = d

        if not as_bytes:
            result = np.char.decode(result, encoding='utf8')

        if out_format == 'single':
            return result[0]
        if out_format == 'numpy':
            return result.reshape(out_shape)
        if out_format == 'list':
            return result.tolist()

    def close(self):
        pass

    def __len__(self):
        return self._count

    def describe(self):
        for codec, offset in zip(self.codecs, self.offsets):
            print(f'[{offset}, {offset+len(codec)}): {codec.fmt.NAME} {codec.fmt.config()}')



class InvLookup:
    def __init__(self, codec):
        self.codec = codec

    def __getitem__(self, keys):
        return self.lookup(keys)

    def lookup(self, keys):
        out_format = None
        keys_inp = None
        if isinstance(keys, (str, bytes)):
            out_format = 'single'
            keys_inp = np.array([keys], dtype='S')
        elif isinstance(keys, (list, tuple)):
            out_format = 'list'
            keys_inp = np.array(keys, dtype='S')
        elif hasattr(keys, 'dtype'):
            if keys.shape == tuple():
                out_format = 'single'
                keys_inp = np.array([keys], dtype='S')
            else:
                out_format = 'numpy'
                keys_inp = np.array(keys, dtype='S')
        elif isinstance(keys, (Iter)):
            out_format = 'list'
            keys_inp = np.array(list(keys), dtype='S')

        res = self.codec._lookup(keys_inp)
        if out_format == 'single':
            if res[0] == -1:
                raise LookupError(f'{repr(keys)} not found')
            return res.item()
        elif out_format == 'list':
            return res.tolist()
        return res

    def close(self):
        pass
