"""Synthetic ClueWeb22-style docnos: `clueweb22-en%04d-%02d-%05d`.

The last field counts up from 00000 within a source file (5k-40k docs, random), then restarts when the
middle field increments; the middle field wraps after 48 files and `en%04d` increments.
"""
import numpy as np


def cw22_ids(n, seed=0, lang='en'):
    rng = np.random.default_rng(seed)
    out = []
    a = b = 0
    while len(out) < n:
        run = int(rng.integers(5_000, 40_001))
        run = min(run, n - len(out))
        prefix = f'clueweb22-{lang}{a:04d}-{b:02d}-'
        out.extend(f'{prefix}{i:05d}' for i in range(run))
        b += 1
        if b == 48:
            a, b = a + 1, 0
    return out


if __name__ == '__main__':
    import sys
    for i in cw22_ids(int(sys.argv[1]) if len(sys.argv) > 1 else 10):
        print(i)
