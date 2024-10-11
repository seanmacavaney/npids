import numpy as np


def chunked(it, n):
    assert n > 0
    batch = []
    for i in it:
        batch.append(i)
        if len(batch) >= n:
            yield batch
            batch = []
    if batch:
        yield batch


def fnv1_32(batch):
    basis, prime = 2166136261, 16777619
    hashes = np.full(batch.shape, basis, dtype=np.uint32)
    batch = batch.view(np.uint8).reshape(len(batch), -1)
    for i in range(batch.shape[1]):
        mask = batch[:, i] != 0
        np.bitwise_xor(hashes, batch[:, i], out=hashes, where=mask)
        np.multiply(hashes, prime, out=hashes, where=mask)
    return hashes


class InvHash:
    NAME = 'hash'
    def __init__(self, hash_offsets, dids, hash_bits, hash_fn='fnv1_32'):
        self.hash_offsets = hash_offsets
        self.dids = dids
        self.hash_bits = hash_bits
        self.hash_mask = (1 << hash_bits) - 1
        self.fwd = None
        self.hash_fn = hash_fn

    def _lookup(self, docnos: np.array) -> np.array:
        hashes = fnv1_32(docnos) & self.hash_mask
        pos, end = self.hash_offsets[hashes], self.hash_offsets[hashes+1]
        result = np.full(docnos.shape, -1, dtype=np.int64)
        todo = pos < end
        while todo.any():
            cands = self.fwd.lookup(self.dids[pos[todo]], as_bytes=True)
            matches = cands == docnos[todo]
            mask = np.zeros_like(todo)
            mask[todo] = matches
            result[mask] = self.dids[pos[mask]]
            todo[todo] = ~matches
            pos[todo] += 1
            todo[todo] = pos[todo] < end[todo]
        return result

    @staticmethod
    def build(fwd, writer):
        num_buckets_bits = (len(fwd) - 1).bit_length()
        num_buckets = 1 << num_buckets_bits
        hash_mask = num_buckets - 1
        buckets = np.empty(len(fwd), dtype=np.uint32)
        bidx = 0
        for start_idx in range(0, len(fwd), 10_000):
            end_idx = min(start_idx+10_000, len(fwd))
            batch = fwd.lookup(np.arange(start_idx, end_idx), as_bytes=True)
            hashes = fnv1_32(batch)
            buckets[bidx:bidx+batch.shape[0]] = hashes & hash_mask
            bidx += batch.shape[0]
        bucket_counts = np.zeros(num_buckets, dtype=np.uint32)
        idxs, counts = np.unique(buckets, return_counts=True)
        bucket_counts[idxs] = counts.astype(np.uint32)
        bucket_offsets = np.cumsum(bucket_counts).astype(np.uint32)
        writer.write_header(1, len(fwd), {
            'format': 'hash',
            'hash_bits': num_buckets_bits,
            'hash_fn': 'fnv1_32',
        })
        writer.write(np.array([0], dtype=np.uint32).tobytes())
        writer.write(bucket_offsets.tobytes())
        writer.write(np.argsort(buckets).astype(np.uint32).tobytes())
