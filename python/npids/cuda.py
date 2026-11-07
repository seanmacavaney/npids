"""Forward lookups (index -> ID) on an NVIDIA GPU, with cupy.

The whole npids file is copied to the device once. Decoding a batch of indices that are already on the
device (e.g. a top-k result, as a cupy, torch or numba array) then runs on the GPU and produces the
Arrow string layout: ``offsets`` (int64, n+1) and ``data`` (uint8), so ID ``i`` is
``data[offsets[i]:offsets[i+1]]``.

    from npids import Lookup
    from npids.cuda import DeviceLookup
    dl = DeviceLookup(Lookup('docnos.npids'))
    offsets, data = dl.lookup(topk_idxs)        # cupy arrays
    offsets, data = dl.lookup_host(topk_idxs)   # numpy arrays
    ids = dl.lookup_list(topk_idxs)             # list of str

Requires ``cupy``. Out-of-range indices decode to ''.
"""
import numpy as np

import cupy as cp

__all__ = ['DeviceLookup']

_SOURCE = r'''
typedef unsigned long long u64;
typedef unsigned int u32;
typedef unsigned char u8;

// meta: [total, chunk_bits]
// chunks[c]: template | field_start << 32
// fields[4f..4f+3]: [enc | bits << 8 | stride << 16 | bit_off << 40, p0, p1, data offset]
// parts[p]: kind | width << 8 | lit_len << 16 | lit_off << 32
// part_start[t]: first part of template t

__device__ __forceinline__ u64 read_bits(const u64* w64, u64 base, u64 pos, u32 bits) {
    if (bits == 0) return 0;
    u64 wi = (base >> 3) + (pos >> 6);
    u32 off = pos & 63;
    u64 v = w64[wi] >> off;
    if (off + bits > 64) v |= w64[wi + 1] << (64 - off);
    if (bits < 64) v &= (1ULL << bits) - 1;
    return v;
}

template <bool WRITE>
__device__ __forceinline__ long long decode(
        u64 idx, const u64* meta, const u64* chunks, const u64* fields, const u64* parts,
        const long long* part_start, const u8* lit_pool, const u8* file, u8* out) {
    const u64 total = meta[0];
    const u32 cb = (u32)meta[1];
    if (idx >= total) return 0;
    const u64* w64 = (const u64*)file;
    const u32* w32 = (const u32*)file;
    u64 c = idx >> cb;
    u64 j = idx & ((1ULL << cb) - 1);
    u64 count = 1ULL << cb;
    if (c == (total - 1) >> cb) count = total - (c << cb);
    u64 ce = chunks[c];
    long long t = ce & 0xFFFFFFFFULL;
    long long fi = ce >> 32;
    long long pos = 0;
    for (long long p = part_start[t]; p < part_start[t + 1]; p++) {
        u64 pe = parts[p];
        u32 kind = pe & 0xFF;
        if (kind == 0) {
            u32 ll = (pe >> 16) & 0xFFFF;
            if (WRITE) { const u8* lit = lit_pool + (pe >> 32); for (u32 q = 0; q < ll; q++) out[pos + q] = lit[q]; }
            pos += ll;
            continue;
        }
        const u64* f = fields + 4 * fi;
        u64 fm = f[0];
        u32 enc = fm & 0xFF;
        if (kind == 5) {
            u64 o = (f[3] >> 2) + j;
            u32 a = w32[o];
            u32 sl = w32[o + 1] - a;
            if (WRITE) { const u8* s = file + f[3] + 4 * (count + 1) + a; for (u32 q = 0; q < sl; q++) out[pos + q] = s[q]; }
            pos += sl;
        } else {
            u64 v;
            if (enc == 0) v = f[1];
            else if (enc == 1) v = f[1] + j * f[2];
            else v = f[1] + read_bits(w64, f[3], j * ((fm >> 16) & 0xFFFFFF) + (fm >> 40), (fm >> 8) & 0xFF);
            int w;
            if (kind == 1) { w = 1; for (u64 x = v; x >= 10; x /= 10) w++; }
            else w = (pe >> 8) & 0xFF;
            if (WRITE) {
                if (kind <= 2) {
                    for (int q = w - 1; q >= 0; q--) { out[pos + q] = '0' + (u8)(v % 10); v /= 10; }
                } else {
                    const char* digits = kind == 3 ? "0123456789abcdef" : "0123456789ABCDEF";
                    for (int q = w - 1; q >= 0; q--) { out[pos + q] = digits[v & 15]; v >>= 4; }
                }
            }
            pos += w;
        }
        fi++;
    }
    return pos;
}

#define THREADS 128

// Pass 1: length of each ID, and the total length per block.
extern "C" __global__ void npids_lengths(
        const long long* idxs, long long n, const u64* meta, const u64* chunks, const u64* fields,
        const u64* parts, const long long* part_start, const u8* lit_pool, const u8* file,
        long long* lens, long long* block_sums) {
    __shared__ long long sh[THREADS];
    long long i = blockIdx.x * (long long)THREADS + threadIdx.x;
    long long len = i < n ? decode<false>((u64)idxs[i], meta, chunks, fields, parts, part_start, lit_pool, file, 0) : 0;
    if (i < n) lens[i] = len;
    sh[threadIdx.x] = len;
    __syncthreads();
    for (int s = THREADS / 2; s > 0; s >>= 1) {
        if (threadIdx.x < s) sh[threadIdx.x] += sh[threadIdx.x + s];
        __syncthreads();
    }
    if (threadIdx.x == 0) block_sums[blockIdx.x] = sh[0];
}

// Pass 2: offsets (sum of earlier blocks + scan within the block), then write the bytes.
extern "C" __global__ void npids_write(
        const long long* idxs, long long n, const u64* meta, const u64* chunks, const u64* fields,
        const u64* parts, const long long* part_start, const u8* lit_pool, const u8* file,
        const long long* lens, const long long* block_sums, long long* offsets, u8* out) {
    __shared__ long long sh[THREADS];
    long long b = 0;
    for (long long k = threadIdx.x; k < blockIdx.x; k += THREADS) b += block_sums[k];
    sh[threadIdx.x] = b;
    __syncthreads();
    for (int s = THREADS / 2; s > 0; s >>= 1) {
        if (threadIdx.x < s) sh[threadIdx.x] += sh[threadIdx.x + s];
        __syncthreads();
    }
    long long base = sh[0];
    __syncthreads();
    long long i = blockIdx.x * (long long)THREADS + threadIdx.x;
    long long len = i < n ? lens[i] : 0;
    sh[threadIdx.x] = len;
    __syncthreads();
    for (int d = 1; d < THREADS; d <<= 1) {   // inclusive scan
        long long v = threadIdx.x >= d ? sh[threadIdx.x - d] : 0;
        __syncthreads();
        sh[threadIdx.x] += v;
        __syncthreads();
    }
    long long start = base + sh[threadIdx.x] - len;
    if (i < n) {
        offsets[i] = start;
        if (i == n - 1) offsets[n] = start + len;
        decode<true>((u64)idxs[i], meta, chunks, fields, parts, part_start, lit_pool, file, out + start);
    }
}
'''

_module = None
_THREADS = 128


def _kernels():
    global _module
    if _module is None:
        _module = cp.RawModule(code=_SOURCE, options=('-std=c++14',), name_expressions=None)
    return _module.get_function('npids_lengths'), _module.get_function('npids_write')


class DeviceLookup:
    """A copy of an npids file on the GPU for batched forward lookups."""

    def __init__(self, lookup):
        L = lookup._reader.layout()
        self.total = int(L['total'])
        self.max_len = max(int(L['max_len']), 1)
        U = np.uint64
        raw = np.fromfile(lookup.path, dtype=np.uint8)
        raw = np.concatenate([raw, np.zeros(-len(raw) % 8 + 8, dtype=np.uint8)])
        cs = L['c_field_start'][:-1].astype(U)
        chunks = L['c_template'].astype(U) | (cs << U(32))
        fm = (L['f_enc'].astype(U) | (L['f_bits'].astype(U) << U(8))
              | (L['f_stride'].astype(U) << U(16)) | (L['f_bit_off'].astype(U) << U(40)))
        fields = np.stack([fm, L['f_p0'], L['f_p1'], L['f_data']], axis=1).astype(U).reshape(-1)
        parts = (L['p_kind'].astype(U) | (L['p_width'].astype(U) << U(8))
                 | (L['p_lit_len'].astype(U) << U(16)) | (L['p_lit_off'].astype(U) << U(32)))
        nonempty = lambda a, dt: a if len(a) else np.zeros(1, dtype=dt)
        self._tables = (
            cp.asarray(np.array([self.total, int(L['chunk_bits'])], dtype=U)),
            cp.asarray(nonempty(chunks, U)),
            cp.asarray(nonempty(fields, U)),
            cp.asarray(nonempty(parts, U)),
            cp.asarray(L['t_part_start'].astype(np.int64)),
            cp.asarray(nonempty(L['lit_pool'], np.uint8)),
            cp.asarray(raw),
        )
        self._len_kernel, self._write_kernel = _kernels()

    def lookup(self, idxs):
        """Decode indices (any device array, or a host array). Returns cupy ``(offsets, data)``, where
        ``data`` may be longer than ``offsets[-1]``. Runs asynchronously on the current stream."""
        idxs = cp.asarray(idxs)
        if idxs.dtype != cp.int64 or not idxs.flags.c_contiguous or idxs.ndim != 1:
            idxs = cp.ascontiguousarray(idxs.reshape(-1), dtype=cp.int64)
        n = idxs.shape[0]
        blocks = max((n + _THREADS - 1) // _THREADS, 1)
        scratch = cp.empty(n + blocks, dtype=cp.int64)
        lens, block_sums = scratch[:n], scratch[n:]
        offsets = cp.empty(n + 1, dtype=cp.int64)
        data = cp.empty(max(n * self.max_len, 1), dtype=cp.uint8)
        if n == 0:
            offsets.fill(0)
            return offsets, data
        nn = np.int64(n)
        self._len_kernel((blocks,), (_THREADS,), (idxs, nn, *self._tables, lens, block_sums))
        self._write_kernel((blocks,), (_THREADS,), (idxs, nn, *self._tables, lens, block_sums, offsets, data))
        return offsets, data

    def lookup_host(self, idxs):
        """Like :meth:`lookup`, but returns numpy ``(offsets, data)`` with ``data`` trimmed."""
        offsets, data = self.lookup(idxs)
        offsets = cp.asnumpy(offsets)
        return offsets, cp.asnumpy(data[:offsets[-1]])

    def lookup_list(self, idxs):
        """Like :meth:`lookup_host`, but returns a list of str."""
        offsets, data = self.lookup_host(idxs)
        b = data.tobytes()
        return [b[offsets[i]:offsets[i + 1]].decode() for i in range(len(offsets) - 1)]
