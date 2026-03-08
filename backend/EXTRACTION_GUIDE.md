# Arknights Endfield — Extraction Guide

## Overview

This backend extracts game assets from Arknights Endfield's VFS (Virtual File System).
There are two tracks: **static asset extraction** (offline, from game servers) and
**runtime stat capture** (requires Frida + running game).

## Quick Start

```bash
cd backend

# Show game version, patch info, resource URLs
cargo run --bin endfield --release -- info

# Extract everything (JSON, textures, gamedata) from all VFS dirs
cargo run --bin endfield --release -- extract --all

# Extract only textures
cargo run --bin endfield --release -- extract --textures --output ./output
```

---

## Track A: Static Asset Extraction

### endfield CLI

The unified CLI for all offline asset extraction.

#### `endfield info`

Fetches game config from the launcher API and displays version, domain, patch info,
and resource URLs.

#### `endfield extract`

Full extraction pipeline. Downloads .chk files from VFS directories, decompresses
VFS containers, parses UnityFS bundles, and extracts assets.

```bash
# All asset types (default when no flags given)
endfield extract

# Specific types
endfield extract --json          # TextAssets (JSON, text, binary)
endfield extract --textures      # Texture2D → PNG
endfield extract --gamedata      # MonoBehaviours + managed reference registry

# Options
endfield extract --all --scan-all          # Process ALL .chk files (not just smallest)
endfield extract --dir 19E3AE45           # Specific VFS directory hash
endfield extract --output ./my_output     # Custom output dir (default: ./output)
```

**Output structure:**
```
output/
├── json/           # TextAsset exports
├── textures/       # Texture2D as PNG (BC7, DXT1, etc.)
├── gamedata/       # MonoBehaviour JSON + registry data
└── raw/            # Raw extracted files
```

#### `endfield extract-file <CHK_NAME>`

Extract a single .chk file by name or substring match.

```bash
endfield extract-file CAB-abc123 --output ./output
```

#### `endfield scan`

Scan VFS for asset names and paths. Searches for common patterns
(char_, operator, texture, etc.) across all embedded strings.

```bash
endfield scan --scan-all    # Scan all .chk files
```

#### `endfield survey`

Census of Unity objects across all VFS files — class ID counts, type names,
total objects, resource stats.

### Standalone Extraction Bins

| Binary | Purpose | Usage |
|--------|---------|-------|
| `gameassembly-extract` | Extract files from game ZIP archive | `cargo run --bin gameassembly-extract --release -- GameAssembly.dll` |
| `smiley-decrypt-test` | Decrypt Wwise PCK audio (`:)xD` format) | `cargo run --bin smiley-decrypt-test --release` |
| `resource-test` | Track resource/patch changes between versions | `cargo run --bin resource-test --release` |
| `auth-test` | Account login + gacha record export | `cargo run --bin auth-test --release -- [token]` |

---

## Track B: Runtime Character Stat Capture

Character stats (HP, ATK, DEF, etc.) are **not** in any static asset file.
They exist only in runtime memory. Two approaches:

### B1: Direct Capture (Frida Gadget → char-data-probe)

**Step 1:** Deploy Frida gadget and capture events.

1. **Build the proxy DLL** (requires `x86_64-w64-mingw32-gcc` cross-compiler):

   ```bash
   backend/scripts/build-version-proxy.sh
   ```

   This builds and copies the DLL to `backend/data/game-binaries/spdlog.dll` (~87KB).
   The proxy forwards all 392 spdlog exports to `spdlog_orig.dll` (the real one)
   and loads `frida-gadget.dll` at CRT init time via a `.CRT$XCU` section initializer.

2. **Download** `frida-gadget-<version>-windows-x86_64.dll` from
   [Frida releases](https://github.com/frida/frida/releases) (look for `frida-gadget-<version>-windows-x86_64.dll.xz`).

3. **Prepare the game directory** (next to `Endfield.exe`, usually
   `C:\Program Files\GRYPHLINK\games\Endfield Game`):

   ```
   # Rename the game's real spdlog.dll so the proxy can forward to it
   ren spdlog.dll spdlog_orig.dll
   ```

4. **Copy into the game directory**:

   ```
   spdlog.dll                     ← proxy DLL (from step 1)
   frida-gadget.dll               ← renamed from frida-gadget-<version>-windows-x86_64.dll
   frida-gadget.dll.config        ← config (see below)
   frida_char_data_gadget.js      ← copied from backend/scripts/frida/
   ```

5. **Create `frida-gadget.dll.config`**:

   ```json
   {
     "interaction": {
       "type": "script",
       "path": "./frida_char_data_gadget.js",
       "on_change": "ignore"
     }
   }
   ```

6. **Launch the game.** The gadget outputs:
   - `il2cpp_char_events.jsonl`
   - `il2cpp_char_data.log`
   - `il2cpp_char_summary.json`

Gadget scripts:
- `backend/scripts/frida/frida_char_data_gadget.js` — Passive hooks (10 hooks, 569 lines)
- `backend/scripts/frida/frida_stat_capture_gadget.js` — Phase S2 unified capture (5 hooks, 1040 lines)

To clean up, remove `spdlog.dll`, `frida-gadget.dll`, `frida-gadget.dll.config`,
and the gadget `.js` from the game directory, then rename `spdlog_orig.dll` back
to `spdlog.dll`.

**Step 2:** Process captured events.

```bash
cargo run --bin char-data-probe --release -- \
  --il2cpp-dump il2cpp_dump.json \
  --capture-log il2cpp_char_events.jsonl \
  --output-dir ./output/char_stats
```

**Output:** Character stat tables (CSV + JSON) with HP, ATK, DEF, STR, AGI, etc.

### B2: Offline Computation (Calibration → stat-engine)

**Step 1:** Run one-time calibration capture.

```bash
# Deploy frida_calibration.js as gadget
# Navigate all characters in-game
# Outputs: calibration_data.json
```

**Step 2:** Compute stats offline.

```bash
# Summary of all calibrated characters
cargo run --bin stat-engine --release -- --calibration calibration_data.json

# Query specific character at level
cargo run --bin stat-engine --release -- \
  --calibration calibration_data.json \
  --char chr_0004_pelica --level 60

# Export full stat tables
cargo run --bin stat-engine --release -- \
  --calibration calibration_data.json \
  --export-csv all_stats.csv \
  --export-json all_stats.json
```

### B3: VFS Decrypt Interception

For the 4 encrypted VFS directories (unknown cipher), intercept decryption at runtime:

```bash
# Deploy frida_vfs_decrypt_gadget.js, capture events, then:

# Analyze decrypt events
cargo run --bin vfs-decrypt-probe --release -- \
  --events vfs_decrypt_events.jsonl

# Reconstruct full files from captured chunks
cargo run --bin vfs-reconstruct --release -- \
  vfs_decrypt_summary.json ./vfs_dumps ./vfs_reconstructed
```

---

## Extraction Modules

The `extraction/` library provides reusable components:

| Module | Purpose |
|--------|---------|
| `pipeline` | VFS download: config resolution, .chk file listing, HTTP download |
| `unity_extract` | SerializedFile parsing, TextAsset/MonoBehaviour extraction |
| `unityfs_header` | UnityFS bundle format parsing + full asset pipeline |
| `texture` | Texture2D decoding (BC7, BC5, DXT1, Alpha8, RGBA32, etc.) → PNG |
| `vfs_decrypt` | VFS AES-128-CTR header validation + decryption |
| `vfs_aes` | Custom AES-128 with non-standard S-box |
| `wwise_pck` | Wwise PCK `:)xD` encrypted audio format |
| `zip_range` | HTTP range-request ZIP extraction |
| `lz4_inv` | Custom LZ4 decompression (game-specific variant) |
| `fair_guard_utils` | FairGuard anti-cheat obfuscation handling |
| `crypto_constants` | All encryption keys, IVs, S-boxes |
| `utils` | format_bytes, hex_dump, sanitize_filename |

---

## Known Values

- **4 VFS directories:** `19E3AE45`, `1CDDBF1F`, `42A8FCA6`, `775A31D1`
- **24 playable characters:** chr_0002 through chr_0029
- **Texture formats:** 95% BC7, also BC5, DXT1, Alpha8, RGBA32, ARGB32, R8
- **2,139 textures** extracted successfully (zero decode failures)
- **656 .resS** resource stream files (1.57 GB)
- **Character meshes:** body/cloth/face/hair/iris submeshes, 4 LOD levels
- **Stats in runtime only:** STR, AGI, ATK, DEF, HP computed via per-character scalar coefficients

## Frida Scripts Reference

| Script | Purpose |
|--------|---------|
| `frida_char_data_gadget.js` | Passive character data hooks (gadget mode) |
| `frida_stat_capture_gadget.js` | Unified stat capture with IL2CPP class navigation |
| `frida_calibration.js` | One-time calibration: scalars, modifiers, calculator interceptions |
| `frida_vfs_decrypt_gadget.js` | VFS NtReadFile interception + direct-write bypass |
| `frida_il2cpp_scan.js` | IL2CPP metadata scanner (classes, methods, fields) |
| `frida_hooks.js` | Legacy injection-mode hooks (decrypt func, HGP, VFS) |
| `frida_hooks_net.js` | Network-layer send/recv interception |
