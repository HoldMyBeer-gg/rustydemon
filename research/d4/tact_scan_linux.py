"""
tact_scan_linux.py — Linux/Proton port of tact_scan.py.

Scans a running process for TACT key name→value pairs by reading
`/proc/<PID>/maps` + `/proc/<PID>/mem` directly, so it works against a
Diablo IV client running under Proton/Wine (the game is a Windows binary but
its address space lives in an ordinary Linux process).

Usage:
    sudo python3 tact_scan_linux.py <PID> [--names d4_key_names.txt] [--out d4_tact_keys.txt] [--all-regions]

Reading another process's /proc/<pid>/mem needs ptrace permission.  Under the
default `ptrace_scope=1` a same-uid, non-child target is blocked, so run under
sudo (or `echo 0 | sudo tee /proc/sys/kernel/yama/ptrace_scope` first).

The scan/match logic is identical to the Windows scanner — only the memory
access primitives differ.  Key names default to the embedded PTR set but should
be overridden with --names for the current build (generate it from the install:
`rustydemon-cli export -a <D4> -p '**/EncryptedNameDict*' -o /tmp/x --dry-run`
then extract the 16-hex IDs).
"""

import argparse
import os
import re
import struct
import sys

# ── Key names ────────────────────────────────────────────────────────────────
# Minimal embedded fallback (the reporter's key + a couple more).  Real runs
# should pass --names with the full current-build list; this is only so the
# script does something useful with no arguments.
EMBEDDED_KEY_IDS_HEX = [
    "0a5015374c0f9fc7",
]


def load_key_ids(names_path):
    """Read filename-hex key IDs, one per line.  Tolerates full
    `EncryptedNameDict-0x<ID>.dat` lines as well as bare 16-hex IDs."""
    ids = []
    with open(names_path) as f:
        for line in f:
            line = line.strip().lower()
            if not line or line.startswith("#"):
                continue
            if "0x" in line:
                line = line.split("0x", 1)[1]
            line = line[:16]
            if len(line) == 16:
                try:
                    int(line, 16)
                    ids.append(line)
                except ValueError:
                    pass
    return ids


def build_patterns(key_ids_hex):
    """Same construction as the Windows scanner (verified against the known
    filename 0a5015374c0f9fc7 → BLTE lookup C79F0F4C3715500A).

    In memory the key name is stored as a native LE u64 of int(h,16); BLTE
    looks it up as u64::from_le_bytes(raw filename bytes), i.e. byte-swapped.
    """
    patterns = {}
    for h in key_ids_hex:
        key_int = int(h, 16)
        pat = struct.pack("<Q", key_int)                       # search this in memory
        blte_key = struct.unpack("<Q", bytes.fromhex(h))[0]    # what BLTE looks up
        patterns[pat] = format(blte_key, "016X")               # output in BLTE format
    return patterns


# ── Value heuristics (unchanged from tact_scan.py) ───────────────────────────
def is_ptr(v: int) -> bool:
    """True if a u64 looks like a user-space pointer (Windows *or* Linux/Wine).
    Used to reject key-value candidates whose halves are actually pointers."""
    if v == 0:
        return True
    hi = v >> 48
    # Windows ASLR user space: 0x0000..0x0007.  Linux/Wine: 0x0000 (heap/mmap
    # low) or 0x5500../0x7f00 (PIE / stack / high mmap).
    return hi in (0x0000, 0x0001, 0x0002, 0x0003, 0x0004, 0x0005, 0x0006,
                  0x0007, 0x5500, 0x5555, 0x7f00, 0x7ffc, 0x7fff)


def looks_like_key(b: bytes, all_key_pats, reject_name_halves: bool = True) -> bool:
    if len(b) < 16:
        return False
    if all(x == 0 for x in b):
        return False
    if len(set(b)) < 6:
        return False
    if all(0x20 <= x < 0x7f for x in b):
        return False
    hi1 = struct.unpack_from("<Q", b, 0)[0]
    hi2 = struct.unpack_from("<Q", b, 8)[0]
    if is_ptr(hi1) or is_ptr(hi2):
        return False
    if reject_name_halves and b[8:16] in all_key_pats:
        return False
    return True


# ── Linux memory access ──────────────────────────────────────────────────────
def enum_regions(pid, all_regions):
    """Yield (start, size) for candidate regions from /proc/<pid>/maps.

    By default only writable+readable anonymous mappings (where the KMT heap
    hash table lives).  --all-regions widens to every readable mapping.
    """
    regions = []
    with open(f"/proc/{pid}/maps") as f:
        for line in f:
            parts = line.split()
            if len(parts) < 2:
                continue
            addr_range, perms = parts[0], parts[1]
            path = parts[5] if len(parts) >= 6 else ""
            if "r" not in perms:
                continue
            if not all_regions:
                # KMT is writable heap; skip read-only and file-backed maps.
                if "w" not in perms:
                    continue
                if path.startswith("/") or path in ("[vvar]", "[vsyscall]"):
                    continue
            if path in ("[vvar]", "[vsyscall]", "[vdso]"):
                continue
            start_s, end_s = addr_range.split("-")
            start, end = int(start_s, 16), int(end_s, 16)
            if end > start:
                regions.append((start, end - start))
    return regions


CHUNK = 4 * 1024 * 1024
OVERLAP = 32


def plausible_value(b: bytes) -> bool:
    """Sanity check for the 16 bytes sitting in a confirmed KMT value slot.

    The 16-byte name+name needle already guarantees we're at a real hash-table
    entry, so this only rejects unpopulated/degenerate slots — an all-zero or
    near-constant slot means the game hasn't streamed that key yet."""
    if len(b) != 16:
        return False
    if b.count(0) > 8:          # real Salsa20 keys aren't mostly zero
        return False
    if len(set(b)) < 6:
        return False
    return True


def scan(pid, patterns, all_regions):
    found = {}
    conflicts = {}   # name_hex → set of distinct values seen (should be 1)

    # Search for the 16-byte INLINE needle: [name][name].  The live KMT stores
    # each entry as [name(8)][name(8)][value(16)] (research/d4/kmt-memory-scanning.md),
    # so a duplicated-name hit is ~impossible to forge by coincidence — unlike an
    # 8-byte name, which litters memory inside UTF-16 strings and unrelated data.
    needle_to_name = {pat + pat: name for pat, name in patterns.items()}
    combined = re.compile(b"|".join(re.escape(n) for n in needle_to_name))

    fd = os.open(f"/proc/{pid}/mem", os.O_RDONLY)
    try:
        regions = enum_regions(pid, all_regions)
        total_bytes = sum(sz for _, sz in regions)
        print(f"Scanning PID {pid}: {len(regions)} regions, "
              f"{total_bytes / 1_048_576:.0f} MiB", file=sys.stderr)

        scanned = 0
        next_report = 256 * 1024 * 1024
        for base, size in regions:
            offset = 0
            while offset < size:
                read_len = min(CHUNK + OVERLAP, size - offset)
                try:
                    data = os.pread(fd, read_len, base + offset)
                except OSError:
                    offset += CHUNK
                    continue
                if data:
                    for m in combined.finditer(data):
                        name_hex = needle_to_name.get(m.group(0))
                        if name_hex is None:
                            continue
                        idx = m.start()
                        if idx + 32 > len(data):
                            continue        # value spills past chunk; caught next chunk
                        value = data[idx + 16:idx + 32]
                        if not plausible_value(value):
                            continue
                        val_hex = value.hex().upper()
                        conflicts.setdefault(name_hex.lower(), set()).add(val_hex)
                        if name_hex.lower() not in found:
                            found[name_hex.lower()] = val_hex
                            print(f"  FOUND {name_hex} {val_hex}", file=sys.stderr)

                offset += CHUNK
                scanned += read_len
                if scanned >= next_report:
                    print(f"  … {scanned / 1_048_576:.0f} MiB scanned, "
                          f"{len(found)} found", file=sys.stderr)
                    next_report += 256 * 1024 * 1024
    finally:
        os.close(fd)

    dupes = {k: v for k, v in conflicts.items() if len(v) > 1}
    if dupes:
        print(f"  WARNING: {len(dupes)} key(s) had conflicting values "
              f"(kept first): {', '.join(sorted(dupes)[:5])}…", file=sys.stderr)

    return found


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pid", type=int)
    ap.add_argument("--names", help="file of filename-hex key IDs (one per line)")
    ap.add_argument("--out", default="d4_tact_keys.txt")
    ap.add_argument("--all-regions", action="store_true",
                    help="scan every readable region, not just writable heap")
    args = ap.parse_args()

    key_ids = load_key_ids(args.names) if args.names else EMBEDDED_KEY_IDS_HEX
    if not key_ids:
        print("no key IDs to search for", file=sys.stderr)
        sys.exit(1)
    patterns = build_patterns(key_ids)
    print(f"searching for {len(patterns)} key name(s)", file=sys.stderr)

    found = scan(args.pid, patterns, args.all_regions)

    lines = [f"{name.upper()} {val}" for name, val in sorted(found.items())]
    with open(args.out, "w") as f:
        f.write("\n".join(lines) + ("\n" if lines else ""))

    total = len(patterns)
    pct = (100.0 * len(found) / total) if total else 0.0
    print(f"\n--- {len(found)}/{total} key(s) recovered ({pct:.1f}%) "
          f"written to {args.out} ---", file=sys.stderr)
    for line in lines:
        print(line)


if __name__ == "__main__":
    main()
