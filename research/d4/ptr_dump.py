"""
Scan PTR process for a KNOWN key value to understand the in-memory structure.
We know: key 53FA92ED4238228A -> value 55C6C0A89CD8B608E8FCA6F68E3D261B
         key 0E5332FB2D834BBD -> value 3ED1F79569C1E7B89941F8D358C31140
         key 665198D2E8358929 -> value DF8968D3D37D7DBA3763E4184D8F1933
Search for the value bytes and dump context to understand PTR KMT layout.
"""

import ctypes
import ctypes.wintypes as wt
import struct
import sys

PROCESS_QUERY_INFORMATION = 0x0400
PROCESS_VM_READ           = 0x0010
MEM_COMMIT                = 0x1000
PAGE_NOACCESS             = 0x001
PAGE_GUARD                = 0x100

k32 = ctypes.windll.kernel32

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
    addr = 0
    mbi  = MEMORY_BASIC_INFORMATION()
    sz   = ctypes.sizeof(mbi)
    while True:
        ret = k32.VirtualQueryEx(handle, ctypes.c_size_t(addr), ctypes.byref(mbi), sz)
        if ret == 0:
            break
        if (mbi.State == MEM_COMMIT
                and not (mbi.Protect & PAGE_NOACCESS)
                and not (mbi.Protect & PAGE_GUARD)):
            yield mbi.BaseAddress, mbi.RegionSize
        addr += mbi.RegionSize
        if addr > 0x7FFFFFFFFFFF:
            break

def read_mem(handle, addr, size):
    buf  = (ctypes.c_char * size)()
    read = ctypes.c_size_t(0)
    ok   = k32.ReadProcessMemory(handle, ctypes.c_size_t(addr), buf, size, ctypes.byref(read))
    if not ok or read.value == 0:
        return None
    return bytes(buf[:read.value])

KNOWN_KEYS = {
    bytes.fromhex("55C6C0A89CD8B608E8FCA6F68E3D261B"): ("53FA92ED4238228A", "8a223842ed92fa53"),
    bytes.fromhex("3ED1F79569C1E7B89941F8D358C31140"): ("0E5332FB2D834BBD", "bd4b832dfb32530e"),
    bytes.fromhex("DF8968D3D37D7DBA3763E4184D8F1933"): ("665198D2E8358929", "298935e8d2985166"),
    bytes.fromhex("9D77FE7CDE388BB382E7FABDAF0EEFE9"): ("1C3AC80099C0F009", "09f0c09900c83a1c"),
    bytes.fromhex("E00294A2AF86C537E088A086BABE3D82"): ("F159F1F70EABAAB1", "b1aaab0ef7f159f1"),
}

CHUNK = 4 * 1024 * 1024

if __name__ == "__main__":
    pid = int(sys.argv[1])
    handle = open_process(pid)
    print(f"Scanning PID {pid} for known key values...", file=sys.stderr)

    for base, size in enum_readable_regions(handle):
        offset = 0
        while offset < size:
            chunk_size = min(CHUNK, size - offset)
            data = read_mem(handle, base + offset, chunk_size)
            if data is None:
                offset += chunk_size
                continue

            for val_bytes, (blte_id, filename_id) in KNOWN_KEYS.items():
                pos = 0
                while True:
                    idx = data.find(val_bytes, pos)
                    if idx < 0:
                        break
                    # Dump context around this hit
                    ctx_start = max(0, idx - 32)
                    ctx_end   = min(len(data), idx + 32 + 16)
                    ctx       = data[ctx_start:ctx_end]
                    addr = base + offset + idx
                    print(f"\n=== Found value for key {blte_id} (file 0x{filename_id}) at 0x{addr:016X} ===")
                    # Print in 8-byte rows
                    for i in range(0, len(ctx), 8):
                        row = ctx[i:i+8]
                        row_addr = base + offset + ctx_start + i
                        offset_from_val = (ctx_start + i) - idx
                        marker = " <-- VALUE" if ctx_start + i == idx else f" [{offset_from_val:+d}]"
                        print(f"  0x{row_addr:016X} ({offset_from_val:+4d}): {row.hex().upper():16s} {marker}")
                    pos = idx + 1

            offset += chunk_size

    k32.CloseHandle(handle)
