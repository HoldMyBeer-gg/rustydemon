"""
tact_scan.py — scan a running process for TACT key name→value pairs.

Usage:
    python tact_scan.py <PID>

Scans all readable private heap memory for known 8-byte TACT key names and
reads the 16-byte key values adjacent to them.  Prints results in wowdev
space-separated format: KEYNAME_HEX KEYVALUE_HEX
"""

import ctypes
import ctypes.wintypes as wt
import struct
import sys
import re

# ── Win32 constants ─────────────────────────────────────────────────────────
PROCESS_QUERY_INFORMATION = 0x0400
PROCESS_VM_READ           = 0x0010
MEM_COMMIT                = 0x1000
PAGE_NOACCESS             = 0x001
PAGE_GUARD                = 0x100

k32  = ctypes.windll.kernel32
psapi = ctypes.windll.psapi

# ── MEMORY_BASIC_INFORMATION ────────────────────────────────────────────────
class MEMORY_BASIC_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("BaseAddress",       ctypes.c_size_t),
        ("AllocationBase",    ctypes.c_size_t),
        ("AllocationProtect", wt.DWORD),
        ("RegionSize",        ctypes.c_size_t),
        ("State",             wt.DWORD),
        ("Protect",           wt.DWORD),
        ("Type",              wt.DWORD),
    ]

def open_process(pid):
    h = k32.OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, False, pid)
    if not h:
        raise ctypes.WinError()
    return h

def enum_readable_regions(handle):
    """Yield (base, size) for all committed, readable, non-guard regions."""
    addr = 0
    mbi  = MEMORY_BASIC_INFORMATION()
    sz   = ctypes.sizeof(mbi)
    while True:
        ret = k32.VirtualQueryEx(handle, ctypes.c_size_t(addr),
                                  ctypes.byref(mbi), sz)
        if ret == 0:
            break
        if (mbi.State == MEM_COMMIT
                and not (mbi.Protect & PAGE_NOACCESS)
                and not (mbi.Protect & PAGE_GUARD)):
            yield mbi.BaseAddress, mbi.RegionSize
        addr += mbi.RegionSize
        if addr > 0x7FFFFFFFFFFF:   # stay in user space
            break

def read_mem(handle, addr, size):
    buf  = (ctypes.c_char * size)()
    read = ctypes.c_size_t(0)
    ok   = k32.ReadProcessMemory(handle, ctypes.c_size_t(addr),
                                  buf, size, ctypes.byref(read))
    if not ok or read.value == 0:
        return None
    return bytes(buf[:read.value])

# ── Key names from D4 EncryptedNameDict filenames ───────────────────────────
# Each is a u64 LE; we search for these 8-byte patterns in memory.
# Generated from: base/EncryptedNameDict-0x<ID>.dat
# Full PTR (2.6.0.70501) EncryptedNameDict key set — 184 files
KEY_IDS_HEX = [
    "00b9b2a6cb806a3d","02819b3a245a767c","0296d1f07ca1df56","02d64e000d128ac5",
    "04af7358e24ff07b","093b6416e2835509","09f0c09900c83a1c","0a5015374c0f9fc7",
    "0ac43975994b4106","0c22c1f954a336f0","0c2931d39ff685ad","0c585874e53841a4",
    "0cea68beb6ec803e","0dc8f43051340411","0e45c5b3f080c6fb","0e9b08fa35ddc6d8",
    "0f73ba902a27279e","117fa68bf73b4f46","11d2784edc1c3769","19ae70d4f8ce4833",
    "1c6299566314a81d","1e15e556129dd6a5","207ce929ca10d688","20b887ab86b29eff",
    "23994777a02901f1","23f52090bd6414e0","25bb457c64653cd4","268ecbb3cd39c706",
    "271f26c4d4a64875","27c4a114536506d8","298935e8d2985166","2ae07585052a28de",
    "2babd3fba391b64a","2eba52c766dd5133","30481734ca1beb09","3159e78ad558d81e",
    "33d3455983ca5be5","35fe77f664722b93","363463c7ad46e5ba","39f4bf6898b8afb7",
    "3b5646cd2a44ed23","3bc501404c5ea17a","3c37e237f2ea13f8","3ea0f19a64f57cfd",
    "3fe9ca51458d7074","41c2c06e9734ae99","44549befd43628e1","49c1973b9187cee2",
    "4a2698166bd0eef8","4a80a460e28f06dd","4cd01eb4ba52b1d3","4d3cadbaeb53462b",
    "4e63648c952aad61","4e6dde2436600f88","52651493ad4fde83","53301688d6060182",
    "5357e6fd5313bb80","53a9eed970275d93","56c11d5703b1cd62","570c993d29e0f053",
    "5a4ed91f5fe14479","5e00ef0ad3a6c7fa","60eb8622b471cabf","637c0b945226609a",
    "63a1f6b61adadfa1","64547c3be707c47b","647062defcac4560","64b7b2fc65ec01d4",
    "65a09c95c5c03a10","65d1e3457b9473af","65fcf70aea2bcd39","6612264f51ca5ba2",
    "66e3f6a1c8ce6bfe","6bbcbaeb32c26d98","6f065be757bd4723","6f3132d3eeaf2a1f",
    "6f34731e2bcb1945","6f5010d43388052f","6ffb7de0b9b8c50d","7071e0478c2f0622",
    "75876ef74f32014e","75dc4391e8b03f92","76bf8ce5f9f6eb49","76f517c7b6896375",
    "77602f74e704b849","7763703d3588429d","796bbc238b12ea3c","7a52b345239dabe8",
    "7a989ac15a9a3c26","7d3ef238d7d524be","7da29f856ac29ab7","7e4c9506710a54b5",
    "7efaf89332a0c06e","7f5e97a90853c568","80a99a0d2e48c1a0","8833e2af6105637d",
    "899a59b6c756ddb7","8a134765c03ee270","8a223842ed92fa53","8ccee4def3880104",
    "8efadde626f7dc66","8feb63c7d24d20e9","911a1b309ad5cdc9","92a40362cc12955a",
    "92d22b8b23c99f93","93399ccf91519776","9588a3afcc74db0f","9588b155114b5b42",
    "969796b620592968","99f77a097ed90eb8","9afa34c766a9fcc5","9bed681f0dc6c164",
    "9dc9af77a803eb97","9dcf3c25bf3b6afb","9f762d72da44beed","9fcd1b6af0b4cf8b",
    "a21546af07fc0820","a365324771a9b9b7","a486d013627eb6a4","a5b1a3b4a325d636",
    "a5fe26a62054a2ce","a6927e313d2e1231","a6a382c4771ffc7c","a77ed1d8068fd2ab",
    "aa89ee6a2722de91","acacdd907969ab03","aeba2a3a58de7854","aebe3fb3330c35fe",
    "b1af1db9f17fa6d7","b1aaab0ef7f159f1","b23b0175d97aca6d","b2f3a7f548cbd50a",
    "b4da1b8ad9141078","b76560768ae38b8a","b86956d6d91d443b","bc95bd45e558f14a",
    "bd4b832dfb32530e","c00f1fd390a4aa96","c03f22e1b898322b","c04b6ae2d0e3fbf6",
    "c0a20b37e510d2b1","c14dc08060f9511d","c25cf2becc3b013e","c2b90a22cc6e486d",
    "c6269e2b13409ed7","c6674c942e38e929","c8273ec77b455370","c9a2f1dd0070603d",
    "ca9a3eb527f86eff","cac06c7b567326b7","cc29d089cbad6059","ce19c6558cefdb2b",
    "ce74442b4c1fb82b","d573abc49816a246","d9547915664390a4","d9aebc355721a591",
    "dda7ef59c165e490","ddaaa6375b245536","e0861787e6650df0","e1a5e0d54038e62e",
    "e2cd1b98025480ca","e31f0ca32b5a48be","e5326863682422ad","e8090a2bd5e6e7b8",
    "e96f69e932031e83","ea2a1a8784018d99","eb1eff0b64416e57","eb4d907876a15042",
    "ed843ad87633f47c","eda9b641b34aff74","edcfa4e92857c6ed","eed9a8c332e13e38",
    "f221dc10f30817bf","f22db741197babd4","f37c618f8d8e780a","f53e3a480013b59f",
    "f61dc328a49e2b27","f796f9e481aabec9","f7d8eb38d48cdfd9","f81378acae8096b8",
    "f850e87bf4bb815f","f9c3ce895d6e413a","fb45e6b23dc6ca40","fcdf1a78a63e10a6",
    "fdd1c709131c45b7","ef133e99f633fb1f",
]

# Build binary search patterns.
# In memory, the TACT key table stores the key name as a native u64 (LE on x64),
# so we search for the LE bytes of int(h, 16).
# But BLTE reads the 8 key-name bytes from the block as u64::from_le_bytes,
# where the block stores the raw hex bytes (big-endian order).
# So BLTE's lookup key = u64::from_le_bytes(bytes.fromhex(h)) = byte-swapped int.
PATTERNS = {}
for h in KEY_IDS_HEX:
    key_int = int(h, 16)                          # 0xED843AD87633F47C
    pat = struct.pack("<Q", key_int)               # search for this in memory
    blte_key = struct.unpack("<Q", bytes.fromhex(h))[0]  # what BLTE will look up
    PATTERNS[pat] = format(blte_key, '016X')       # output in BLTE's format

# Set of all 8-byte key-name patterns (for rejecting values that embed another key name)
ALL_KEY_PATS = set(PATTERNS.keys())

# ── Scanning ─────────────────────────────────────────────────────────────────
CHUNK = 4 * 1024 * 1024   # 4 MiB per read

def is_heap_ptr(v: int) -> bool:
    """True if a u64 looks like a Windows heap/user-space pointer."""
    if v == 0:
        return True
    hi = v >> 40  # top 24 bits
    # typical Windows ASLR user-space: 0x000001xx, 0x000002xx, ..., 0x00007Fxx
    return (v >> 48) in (0x0000, 0x0001, 0x0002, 0x0003, 0x0004, 0x0005,
                          0x0006, 0x0007)

def looks_like_key(b: bytes) -> bool:
    """True if 16 bytes look like a TACT key: non-zero, high entropy, not a pointer pair."""
    if len(b) < 16:
        return False
    if all(x == 0 for x in b):
        return False
    if len(set(b)) < 6:          # need reasonable byte diversity
        return False
    if all(0x20 <= x < 0x7f for x in b):
        return False
    # Both halves must NOT look like heap pointers
    hi1 = struct.unpack_from("<Q", b, 0)[0]
    hi2 = struct.unpack_from("<Q", b, 8)[0]
    if is_heap_ptr(hi1) or is_heap_ptr(hi2):
        return False
    # Reject if the second 8-byte half is a known key name pattern
    # (hash table slot stores [something][key_name], not a real key value)
    if b[8:16] in ALL_KEY_PATS:
        return False
    return True

def scan(pid):
    handle = open_process(pid)
    found  = {}   # key_name_hex → key_value_hex

    print(f"Scanning PID {pid}...", file=sys.stderr)
    regions = list(enum_readable_regions(handle))
    print(f"  {len(regions)} readable regions", file=sys.stderr)

    for base, size in regions:
        offset = 0
        while offset < size:
            chunk_size = min(CHUNK, size - offset)
            data = read_mem(handle, base + offset, chunk_size)
            if data is None:
                offset += chunk_size
                continue

            for pat, name_hex in PATTERNS.items():
                if name_hex.lower() in found:
                    continue
                pos = 0
                while True:
                    idx = data.find(pat, pos)
                    if idx < 0:
                        break
                    # PTR KMT layout: [key_name(8)][null_or_ptr(8)][value_ptr(8)]
                    # The actual 16-byte value is pointed to by the u64 at +16.
                    # Do NOT deref +8 — that's a hash-chain pointer, not the value.
                    if idx + 24 <= len(data):
                        raw_ptr = struct.unpack_from("<Q", data, idx + 16)[0]
                        if 0x10000 < raw_ptr < 0x7FFFFFFFFFFF:
                            deref = read_mem(handle, raw_ptr, 16)
                            if deref and looks_like_key(deref):
                                val_hex = deref.hex().upper()
                                if name_hex.lower() not in found:
                                    found[name_hex.lower()] = val_hex
                                    print(f"  FOUND {name_hex} {val_hex}", file=sys.stderr)
                    pos = idx + 1

            offset += chunk_size

    k32.CloseHandle(handle)
    return found

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python tact_scan.py <PID> [output_file]", file=sys.stderr)
        sys.exit(1)

    pid = int(sys.argv[1])
    out_file = sys.argv[2] if len(sys.argv) > 2 else "d4_tact_keys.txt"

    found = scan(pid)

    lines = [f"{name.upper()} {val}" for name, val in sorted(found.items())]
    with open(out_file, "w") as f:
        f.write("\n".join(lines) + "\n")

    print(f"\n--- {len(found)} key(s) written to {out_file} ---", file=sys.stderr)
    for line in lines:
        print(line)
