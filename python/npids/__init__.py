"""Time- and space-efficient bidirectional lookups between dense indices and identifiers."""
import contextlib
import json
import os
from abc import ABCMeta
from collections.abc import Iterable
from typing import Optional, Union

import numpy as np

from ._npids import Reader as _Reader, Writer as _Writer, compact as _compact
from . import legacy

try:
    from importlib.metadata import version as _version
    __version__ = _version('npids')
except Exception:  # pragma: no cover
    __version__ = 'unknown'

__all__ = ['Lookup', 'Builder', 'upgrade']

_MAGIC_V2 = b'NPIDSv2\n'
_ITER_BATCH = 4096


def _is_v2(path) -> bool:
    with open(path, 'rb') as f:
        return f.read(8) == _MAGIC_V2


def _is_int(x) -> bool:
    return isinstance(x, (int, np.integer)) and not isinstance(x, bool)


class Lookup(metaclass=ABCMeta):
    """A memory-mapped, bidirectional map between indices ``0..len-1`` and string IDs.

    ``lookup.fwd[i]`` gives the ID at index ``i``; ``lookup.inv[id]`` gives the first index of ``id``.
    ``lookup[x]`` routes to one or the other based on the type of ``x``.

    Opening a file written by npids < 0.2 (format v0/v1) returns a ``npids.legacy.Lookup``, which
    supports the same lookups but cannot be appended to; see :func:`upgrade`.
    """

    def __new__(cls, path):
        if cls is Lookup and os.path.exists(path) and not _is_v2(path):
            return legacy.Lookup(path)
        return super().__new__(cls)

    def __init__(self, path):
        self.path = path
        self._reader = _Reader(os.fspath(path))
        self.fwd = FwdLookup(self._reader)
        self.inv = InvLookup(self._reader)

    def __getitem__(self, key):
        return self.lookup(key)

    def lookup(self, key, as_bytes=False):
        """Forward lookup for ints/slices/int arrays, inverse lookup for str/bytes/str arrays."""
        probe = key
        if isinstance(key, (list, tuple)):
            if len(key) == 0:
                return []
            probe = key[0]
        elif isinstance(key, np.ndarray):
            if key.dtype.kind in 'iu':
                return self.fwd.lookup(key, as_bytes=as_bytes)
            if key.dtype.kind in 'SUO':
                return self.inv.lookup(key)
        elif not isinstance(key, (str, bytes, slice)) and not _is_int(key) and isinstance(key, Iterable):
            key = list(key)
            if not key:
                return []
            probe = key[0]
        if isinstance(probe, (str, bytes)):
            return self.inv.lookup(key)
        if _is_int(probe) or isinstance(probe, slice):
            return self.fwd.lookup(key, as_bytes=as_bytes)
        raise TypeError('Cannot tell whether this is a forward or inverse lookup; use .fwd or .inv directly.')

    def __len__(self) -> int:
        return len(self._reader)

    def __iter__(self):
        return iter(self.fwd)

    def __contains__(self, item) -> bool:
        if _is_int(item):
            return item in self.fwd
        if isinstance(item, (str, bytes)):
            return item in self.inv
        raise TypeError(f'expected int, str or bytes, not {type(item).__name__}')

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()

    def close(self):
        """Release the file. Lookups made after closing fail."""
        self._reader = self.fwd._reader = self.inv._reader = None

    @property
    def meta(self) -> Optional[dict]:
        """The JSON metadata stored with the file, if any."""
        raw = self._reader.meta()
        return json.loads(raw) if raw else None

    def describe(self) -> str:
        return f'{self!r}\n' + self._reader.describe()

    def verify(self, deep: bool = False):
        """Check checksums; with ``deep``, also check that every ID round-trips (slow)."""
        self._reader.verify(deep)

    def __repr__(self) -> str:
        return f'npids.Lookup({os.fspath(self.path)!r})'

    @staticmethod
    @contextlib.contextmanager
    def builder(path, build_inv: bool = True, chunk_bits: int = 16, meta: Optional[dict] = None, min_block=None):
        """Create a file, or append to an existing one. Nothing is committed if the block raises.

        ``min_block`` is accepted for compatibility with npids < 0.2 and ignored.
        """
        b = Builder(path, build_inv=build_inv, chunk_bits=chunk_bits, meta=meta)
        try:
            yield b
        except BaseException:
            b.abort()
            raise
        b.close()

    @staticmethod
    def build(ids, path, build_inv: bool = True, chunk_bits: int = 16, meta: Optional[dict] = None,
              min_block=None, return_self: bool = True):
        """Write ``ids`` to ``path`` (appending if it exists) and return the opened lookup."""
        with Lookup.builder(path, build_inv=build_inv, chunk_bits=chunk_bits, meta=meta) as b:
            b.add_many(ids)
        if return_self:
            return Lookup(path)

    @staticmethod
    def compact(path, chunk_bits: Optional[int] = None):
        """Rewrite the file without space left over from appends, merging its inverse indexes."""
        _compact(os.fspath(path), chunk_bits)


class Builder:
    """Adds IDs to a file. Use via :meth:`Lookup.builder`."""

    def __init__(self, path, build_inv=True, chunk_bits=16, meta=None):
        meta_bytes = json.dumps(meta).encode() if meta is not None else None
        self._w = _Writer(os.fspath(path), chunk_bits=chunk_bits, build_inv=build_inv, meta=meta_bytes)

    def add(self, id: Union[str, bytes]):
        self._w.add(id)

    def add_many(self, ids: Iterable[Union[str, bytes]]):
        if isinstance(ids, np.ndarray):
            ids = ids.tolist()
        self._w.add_many(ids)

    def __len__(self):
        return len(self._w)

    def close(self):
        """Commit everything added."""
        self._w.finish()

    def abort(self):
        """Discard everything added since the builder was opened."""
        self._w.abort()


class FwdLookup:
    """Index -> ID lookups. Supports ints (negative ones count from the end), slices, lists and
    integer numpy arrays (which return numpy arrays of the same shape)."""

    def __init__(self, reader):
        self._reader = reader

    def __len__(self):
        return len(self._reader)

    def __getitem__(self, idx):
        return self.lookup(idx)

    def lookup(self, idx, as_bytes=False):
        if _is_int(idx):
            return self._reader.get(int(idx), as_bytes)
        if isinstance(idx, slice):
            return self._reader.get_many(np.arange(*idx.indices(len(self)), dtype=np.int64), as_bytes)
        if isinstance(idx, np.ndarray):
            if idx.dtype.kind not in 'iu':
                raise TypeError(f'index arrays must be integers, not {idx.dtype}')
            res = self._reader.get_many(np.ascontiguousarray(idx, dtype=np.int64).reshape(-1), as_bytes)
            if idx.ndim == 0:
                return res[0]
            out = np.array(res, dtype='S' if as_bytes else 'U') if res else np.empty(0, dtype='S1' if as_bytes else 'U1')
            return out.reshape(idx.shape)
        if isinstance(idx, (list, tuple)) or isinstance(idx, Iterable):
            idx = np.asarray(list(idx), dtype=np.int64)
            return self._reader.get_many(idx.reshape(-1), as_bytes)
        raise TypeError(f'unsupported index type {type(idx).__name__}')

    def __iter__(self):
        n = len(self)
        for start in range(0, n, _ITER_BATCH):
            yield from self._reader.get_many(np.arange(start, min(start + _ITER_BATCH, n), dtype=np.int64))

    def __contains__(self, item) -> bool:
        if not _is_int(item):
            raise TypeError(f'expected int, not {type(item).__name__}')
        return 0 <= item < len(self)


class InvLookup:
    """ID -> index lookups. A single missing ID raises KeyError; lists and arrays give -1 for missing IDs."""

    def __init__(self, reader):
        self._reader = reader

    def __getitem__(self, key):
        return self.lookup(key)

    def lookup(self, key):
        if isinstance(key, (str, bytes)):
            res = self._reader.find(key)
            if res is None:
                raise KeyError(key)
            return res
        if isinstance(key, np.ndarray):
            return self._reader.find_many(key.reshape(-1).tolist()).reshape(key.shape)
        if isinstance(key, (list, tuple)):
            return self._reader.find_many(key).tolist()
        if isinstance(key, Iterable):
            return self._reader.find_many(list(key)).tolist()
        raise TypeError(f'unsupported key type {type(key).__name__}')

    def get(self, key, default=None):
        res = self._reader.find(key)
        return default if res is None else res

    def __contains__(self, item) -> bool:
        if not isinstance(item, (str, bytes)):
            raise TypeError(f'expected str or bytes, not {type(item).__name__}')
        return self._reader.find(item) is not None


Lookup.register(legacy.Lookup)


def upgrade(path, new_path=None, chunk_bits: int = 16):
    """Convert a v0/v1 file to the v2 format, in place unless ``new_path`` is given."""
    if _is_v2(path):
        raise ValueError(f'{path} is already a v2 file')
    target = new_path if new_path is not None else f'{os.fspath(path)}.v2-tmp'
    if os.path.exists(target):
        os.remove(target)
    old = legacy.Lookup(path)
    try:
        build_inv = old.inv is not None
        with Lookup.builder(target, build_inv=build_inv, chunk_bits=chunk_bits) as b:
            for i in range(0, len(old), _ITER_BATCH):
                b.add_many(old.fwd.lookup(np.arange(i, min(i + _ITER_BATCH, len(old))), as_bytes=True).tolist())
    except BaseException:
        if os.path.exists(target):
            os.remove(target)
        raise
    finally:
        old.close()
    if new_path is None:
        os.replace(target, path)
