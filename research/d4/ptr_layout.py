"""
Scan PTR memory for known key names and dump raw context (hex only, no binary).
Goal: understand what the in-memory layout around key names looks like in the PTR build.
Only prints lines — no binary data to terminal.
"""

import ctypes
import ctypes.wintypes as wt
import struct
import sys
from tact_scan import PATTERNS, open_process, enum_readable_regions, read_mem

CHUNK = 4 * 1024 * 1024

# Only dump the first few finds per key to keep output manageable
MAX_HITS_PER_KEY = 3

if __name__ == "__main__":
    pid = int(sys.argv[1])
    handle = open_process(pid)
    hits = {}  # name_hex -> count

    for base, size in enum_readable_regions(handle):
        offset = 0
        while offset < size:
            chunk_size = min(CHUNK, size - offset)
            data = read_mem(handle, base + offset, chunk_size)
            if data is None:
                offset += chunk_size
                continue

            for pat, name_hex in PATTERNS.items():
                if hits.get(name_hex, 0) >= MAX_HITS_PER_KEY:
                    continue
                pos = 0
                while True:
                    idx = data.find(pat, pos)
                    if idx < 0:
                        break
                    if hits.get(name_hex, 0) < MAX_HITS_PER_KEY:
                        # Dump -32..+48 bytes as hex rows only
                        ctx_start = max(0, idx - 32)
                        ctx_end   = min(len(data), idx + 8 + 48)
                        ctx = data[ctx_start:ctx_end]
                        addr = base + offset + idx
                        print(f"\n=== {name_hex} at 0x{addr:016X} ===")
                        for i in range(0, len(ctx), 8):
                            row = ctx[i:i+8]
                            row_addr = base + offset + ctx_start + i
                            rel = (ctx_start + i) - idx
                            dup = " <KEY>" if row == pat else ""
                            print(f"  0x{row_addr:016X} ({rel:+4d}): {row.hex().upper():16s}{dup}")
                        hits[name_hex] = hits.get(name_hex, 0) + 1
                    pos = idx + 1

            offset += chunk_size

    ctypes.windll.kernel32.CloseHandle(handle)
