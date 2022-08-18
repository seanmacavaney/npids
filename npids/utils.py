import json
import warnings
import struct
from pathlib import Path
import numpy as np

def wrap_mmap(mm, dtype):
    descr = np.dtype(dtype)
    return np.ndarray.__new__(np.memmap, shape=(len(mm) // descr.itemsize,), dtype=descr, buffer=mm, order='C')


V0_HEADER_FORMAT = "<qqII" # next_ptr, doc_count, config_len, type_id
V0_HEADER_SIZE = struct.calcsize(V0_HEADER_FORMAT)
V0_PREFIX_SIZE = 0
V0_PREFIX2_SIZE = struct.calcsize(V0_HEADER_FORMAT[:2])
HEADER_FORMAT = "<IqqI" # type_id, next_ptr, doc_count, config_len
HEADER_SIZE = struct.calcsize(HEADER_FORMAT)
PREFIX_SIZE = struct.calcsize(HEADER_FORMAT[:2])
PREFIX2_SIZE = struct.calcsize(HEADER_FORMAT[:3])
MAGIC_TYPE = 1145655374 # type_id=b'NPID'
MAGIC_DOC_COUNT = 0
THIS_VERSION = 1


class FileManager:
    def __init__(self, path, mode='r'):
        assert mode in ('r', 'a'), "only r and a modes supported"
        self._mode = mode
        self._path = Path(path)
        self._file = None
        self._last_ptr = None
        if not self._path.exists():
            if mode == 'a':
                # init file
                self._file = self._path.open('w+b')
                self.config = {'version': THIS_VERSION}
                enc_config = json.dumps(self.config).encode()
                self._write_header(MAGIC_TYPE, 0, len(enc_config))
                self.write(enc_config)
                self._last_ptr = 0
            else:
                # raise error
                raise FileNotFoundError(str(self._path) + ' not found')
        else:
            self._file = self._path.open('r+b' if mode == 'a' else 'rb')
            type_id, next_ptr, doc_count, config_len = self._read_header()
            if type_id != MAGIC_TYPE and doc_count != MAGIC_DOC_COUNT:
                warnings.warn('magic header not found; assuming version 0 format. The old format may become deprecated in a future version.')
                self.config = {'version': 0}
                self._read_header = self._read_header_v0
                self._write_header = self._write_header_v0
                self._file.seek(0)
            else:
                self.config = json.loads(self._file.read(config_len))
                if self.config['version'] > THIS_VERSION:
                    raise RuntimeError(f'This file was created with a newer version of npids (file format version={config["version"]}, max supported version={THIS_VERSION}). Please upgrade npids to read this file.')
            if mode == 'a':
                pos = [0]
                while pos[-1] != -1:
                    self._file.seek(pos[-1])
                    _, ptr, _, _ = self._read_header()
                    pos.append(ptr)
                self._last_ptr = pos[-2]
                self._file.seek(0, 2) # end of file

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()

    def close(self):
        self._file.close()
        self._file = None

    def write_header(self, type_id, doc_count=0, config=None):
        if config is not None:
            config_enc = json.dumps(config).encode()
        else:
            config_enc = b''
        self._write_header(type_id, doc_count, len(config_enc))
        if config_enc:
            self.write(config_enc)

    def write_doc_count(self, doc_count):
        here = self._file.tell()
        self._file.seek(self._last_ptr + (PREFIX2_SIZE if self.config['version'] >= 1 else V0_PREFIX2_SIZE))
        self._file.write(struct.pack('<q', doc_count))
        self._file.seek(here)

    def read_header(self):
        type_id, next_ptr, doc_count, config_len = self._read_header()
        if config_len > 0:
            config = json.loads(self._file.read(config_len))
        else:
            config = None
        return type_id, next_ptr, doc_count, config

    def write(self, bytes):
        self._file.write(bytes)

    def _read_header(self):
        chunk = self._file.read(HEADER_SIZE)
        type_id, next_ptr, doc_count, config_len = struct.unpack(HEADER_FORMAT, chunk)
        return type_id, next_ptr, doc_count, config_len

    def _read_header_v0(self):
        chunk = self._file.read(V0_HEADER_SIZE)
        next_ptr, doc_count, config_len, type_id = struct.unpack(V0_HEADER_FORMAT, chunk)
        return type_id, next_ptr, doc_count, config_len

    def _write_header(self, type_id, doc_count, config_len):
        here = self._file.tell()
        if self._last_ptr is not None:
            self._file.seek(self._last_ptr + PREFIX_SIZE)
            self._file.write(struct.pack('<q', here))
            self._file.seek(here)
        self.write(struct.pack(HEADER_FORMAT, type_id, -1, doc_count, config_len))
        self._last_ptr = here

    def _write_header_v0(self, type_id, doc_count, config_len):
        here = self._file.tell()
        if self._last_ptr is not None:
            self._file.seek(self._last_ptr + V0_PREFIX_SIZE)
            self._file.write(struct.pack('<q', here))
            self._file.seek(here)
        self.write(struct.pack(V0_HEADER_FORMAT, -1, doc_count, config_len, type_id))
        self._last_ptr = here

    def flush(self):
        self._file.flush()

    def fileno(self):
        return self._file.fileno()

    def tell(self):
        return self._file.tell()

    def seek(self, pos):
        return self._file.seek(pos)


_marker = object()
class peekable:
    """Simplified version of more_itertools.peekable.
    Based on <https://more-itertools.readthedocs.io/en/stable/_modules/more_itertools/more.html#peekable>
    """

    def __init__(self, iterable):
        self._it = iter(iterable)
        self._cache = _marker

    def __iter__(self):
        return self

    def __bool__(self):
        try:
            self.peek()
        except StopIteration:
            return False
        return True

    def peek(self, default=_marker):
        if self._cache is _marker:
            try:
                self._cache = next(self._it)
            except StopIteration:
                if default is _marker:
                    raise
                return default
        return self._cache

    def __next__(self):
        if self._cache is not _marker:
            result = self._cache
            self._cache = _marker
            return result
        return next(self._it)
