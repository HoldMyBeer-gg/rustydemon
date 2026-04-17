# D4 TACT Key Memory Scanning

Research from scanning D4 (Fenris/fenristest) process memory for KMT decryption keys.
Build tested: live 2.x and PTR 2.6.0.70501 (Season 12 preview, build-comments: `2_6_0ptr - Season 12 PTR - RC5`).

---

## Background

D4 uses TACT/BLTE for content storage. Encrypted files (e.g. `EncryptedNameDict-0x<id>.dat`)
require 16-byte Salsa20 keys distributed at runtime via the Key Management Table (KMT).
The KMT is fetched from Blizzard's CDN/auth infrastructure on game startup — it is **not**
embedded in the client and does **not** appear in the public CDN KeyRing field
(`fenristest` builds have an empty `KeyRing` in the NGDP response).

Key IDs are 8-byte values. The filename hex (`0x8a223842ed92fa53`) is the raw bytes of the
key ID in big-endian order as written to disk.

---

## BLTE Byte-Order Quirk

BLTE reads the 8-byte key name from the encrypted block header as `u64::from_le_bytes`.
This means the key ID that BLTE looks up at runtime is the **little-endian integer
interpretation** of the raw filename bytes — i.e. byte-swapped relative to the filename hex.

Example:
- Filename: `EncryptedNameDict-0x8a223842ed92fa53.dat`
- Raw bytes in order: `[8A, 22, 38, 42, ED, 92, FA, 53]`
- BLTE lookup key: `u64::from_le_bytes([8A,...]) = 0x53FA92ED4238228A`

When building a key table for rustydemon (or any BLTE decoder), store keys under the
**byte-swapped** ID, not the filename hex.

In Python:
```python
filename_hex = "8a223842ed92fa53"
blte_key_id  = struct.unpack("<Q", bytes.fromhex(filename_hex))[0]
# blte_key_id = 0x53FA92ED4238228A  <-- use this as the lookup key
```

---

## Live Game KMT In-Memory Layout

Confirmed on the live D4 game process (Windows x64, ASLR).

The KMT is stored as a **hash table** with entries of the form:

```
[key_name (8 bytes)] [key_name (8 bytes)] [key_value (16 bytes)]
```

The key name is duplicated — this is a hash table artifact (the key is stored inline in
the bucket, duplicated as part of the open-addressing or chaining scheme).

To scan: search for the 16-byte pattern `key_name_bytes + key_name_bytes`, then read the
16 bytes immediately following as the Salsa20 key value.

The 16-byte duplicate needle is highly specific and produces very few false positives
compared to searching for the 8-byte name alone.

**Confirmed working keys (live game, build 2.x):**
```
0E5332FB2D834BBD 3ED1F79569C1E7B89941F8D358C31140
1C3AC80099C0F009 9D77FE7CDE388BB382E7FABDAF0EEFE9
53FA92ED4238228A 55C6C0A89CD8B608E8FCA6F68E3D261B
5960ADCB89D029CC F382EB7DB08F21000600000081938EC1
665198D2E8358929 DF8968D3D37D7DBA3763E4184D8F1933
A1DFDA1AB6F6A163 F382EB7DB08F21000600000081938EC1
F159F1F70EABAAB1 E00294A2AF86C537E088A086BABE3D82
```
(Keys are in BLTE format: the lookup ID, not the filename hex.)

---

## PTR Preview Client: KMT Is Never Loaded

The S12 PTR (2.6.0.70501, `fenristest`) is a **preview client** — Blizzard pushed the
encrypted game data for dataminers but did not operate a game server for this build.

Observed behaviour:
- Battle.net auth succeeds (`error_code: 0` in FenrisDebug.txt)
- CDN nodes resolve and rotate normally
- `[Sigma] [casc] [:0]: Scanning for KMT sequence numbers.` is logged at startup
- **No KMT fetch is ever initiated** — no further KMT log entries appear
- The game reaches the title screen and shows "servers down"

The KMT fetch is gated on a fully-initialized game state (game server connection), which
never happens when gameplay servers are absent. Auth alone is not sufficient.

As a result, **the 16-byte Salsa20 keys for PTR-specific content are inaccessible**
through memory scanning on a preview client. They require either:
- A server-backed PTR (real game servers running)
- Blizzard partnership / press key distribution

---

## CASC VFS False Positive Source

When scanning PTR memory for key name patterns, hits are found at addresses like
`0x000002CB03CF....` in a dense region of 24-byte entries:

```
[key_name (8 bytes)] [null or ptr (8 bytes)] [ptr-to-metadata (8 bytes)]
```

These are **CASC VFS entries**, not KMT entries. The CASC virtual filesystem stores
each encrypted file's key ID as a lookup value (to map file path → content), so the
key ID bytes appear in memory even when the corresponding Salsa20 key is not loaded.

The pointer at `+16` leads to file metadata (not a key value). Dereferencing it and
reading 16 bytes produces high-entropy binary data that passes naive entropy filters
but fails BLTE decryption.

**Distinguishing VFS from KMT:**
- KMT entries use the duplicate-key pattern (`key + key + value`) — search for this first
- If the duplicate pattern produces 0 results, KMT is not loaded; all other hits are VFS
- VFS entries follow the `[key][null_or_ptr][metadata_ptr]` 24-byte layout

---

## Scanner Implementation

See `tact_scan.py` in this repo. Key design decisions:

1. Build `pat = struct.pack("<Q", int(filename_hex, 16))` for memory search
2. Build `blte_key = struct.unpack("<Q", bytes.fromhex(filename_hex))[0]` for output
3. Search for `pat + pat` (16-byte duplicate needle) — not just `pat`
4. Read 16 bytes at `needle_match + 16` as the key value
5. Validate with `looks_like_key()`: reject all-zero, low entropy, heap pointers,
   and values whose second 8 bytes are themselves a known key name

When the duplicate pattern produces results, they are reliable. When it produces 0
results, KMT is not loaded and no amount of fallback scanning will find real keys.
