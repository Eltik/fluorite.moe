# Arknights: Endfield — Asset Extraction Backend

Rust toolkit for extracting and decrypting game assets from the Arknights: Endfield CDN. Downloads, decrypts, and parses VFS archives, Unity asset bundles, textures, game data tables, and more — all without needing the game installed.

## Quick Start

```bash
# Build
cargo build --release

# Show game version and CDN info
cargo run --release --bin endfield -- info

# Extract everything (JSON configs, textures, gamedata)
cargo run --release --bin endfield -- extract --all --scan-all --output ./output

# Decrypt all ChaCha20-encrypted VFS directories (tables, configs, Lua)
cargo run --release --bin endfield -- decrypt --output ./output/decrypted

# List all VFS directories and files on the CDN
cargo run --release --bin endfield -- list
```

## CLI Reference — `endfield`

The unified CLI for all extraction tasks. All subcommands fetch data directly from the CDN.

### `endfield info`

Display game configuration, version, patch info, and CDN resource URLs.

```bash
cargo run --release --bin endfield -- info
```

### `endfield extract`

Extract Unity assets (TextAssets, Texture2D, MonoBehaviour) from AES-encrypted VFS directories.

```bash
# Extract all asset types from all directories
cargo run --release --bin endfield -- extract --all --scan-all

# Extract only JSON/TextAssets
cargo run --release --bin endfield -- extract --json --scan-all

# Extract only textures (PNG)
cargo run --release --bin endfield -- extract --textures --scan-all

# Extract only MonoBehaviour gamedata
cargo run --release --bin endfield -- extract --gamedata --scan-all

# Extract from a specific VFS directory
cargo run --release --bin endfield -- extract --all --dir 7064D8E2

# Custom output directory
cargo run --release --bin endfield -- extract --all --output ./my-output
```

| Flag | Description |
|------|-------------|
| `--json` | Extract JSON and TextAsset files |
| `--textures` | Extract Texture2D as PNG images |
| `--gamedata` | Extract MonoBehaviour objects as JSON |
| `--all` | Extract all types (default if no flags given) |
| `--dir <HASH>` | Target a specific VFS directory hash |
| `--output <PATH>` | Output directory (default: `./output`) |
| `--scan-all` | Process all `.chk` files (default: smallest per directory) |

**Output structure:**
```
output/
  gamedata/           # TextAssets — JSON configs, text files, binary data
  gamedata/monobehaviour/
    <TypeName>/       # MonoBehaviours grouped by script type
      <ObjectName>.json
  textures/           # Texture2D extracted as PNG
```

### `endfield extract-file`

Extract a single `.chk` file by name or substring match.

```bash
# Extract a specific chunk file
cargo run --release --bin endfield -- extract-file "CAB-abc123"

# From a specific directory
cargo run --release --bin endfield -- extract-file "CAB-abc123" --dir 7064D8E2
```

### `endfield decrypt`

Decrypt all ChaCha20-encrypted VFS directories. These contain the game data tables (SparkBuffer `.bytes`), JSON configs, Lua scripts, and other encrypted content.

```bash
# Decrypt all 9 encrypted directories
cargo run --release --bin endfield -- decrypt

# Decrypt a specific directory
cargo run --release --bin endfield -- decrypt --dir 42A8FCA6

# Custom output
cargo run --release --bin endfield -- decrypt --output ./my-decrypted
```

| Flag | Description |
|------|-------------|
| `--dir <HASH>` | Decrypt a specific directory (default: all 9 encrypted dirs) |
| `--output <PATH>` | Output directory (default: `./output/decrypted`) |

**Encrypted VFS directories:**

| Hash | Contents |
|------|----------|
| `DAFE52C9` | IFixPatchOut |
| `42A8FCA6` | Table (SparkBuffer game data) |
| `775A31D1` | JsonData |
| `19E3AE45` | Lua scripts |
| `D6E622F7` | ExtendData |
| `1CDDBF1F` | BundleManifest |
| `3C9D9D2D` | InitialExtendData |
| `1EBAF5C6` | Audio (English) |
| `E1E7D7CE` | Audio (Chinese) |

**Output structure:**
```
output/decrypted/
  42A8FCA6/              # One folder per directory hash
    blc_metadata.txt     # BLC catalog info (group name, version, file count)
    SomeTable.bytes      # Raw decrypted file
    SomeTable.json       # SparkBuffer .bytes auto-parsed to JSON
    ...
```

SparkBuffer `.bytes` files are automatically parsed into human-readable JSON alongside the raw binary.

### `endfield list`

List all VFS directories on the CDN with file counts, sizes, and patch files.

```bash
cargo run --release --bin endfield -- list
```

### `endfield scan`

Scan VFS archives for asset names, bundle paths, and embedded strings. Searches for keywords like `character`, `texture`, `model`, `weapon`, etc.

```bash
# Quick scan (smallest .chk per directory)
cargo run --release --bin endfield -- scan

# Full scan (all .chk files)
cargo run --release --bin endfield -- scan --scan-all
```

### `endfield survey`

Census of all Unity object types, class IDs, counts, and sizes across VFS directories.

```bash
cargo run --release --bin endfield -- survey --scan-all
```

## Other Binaries

| Binary | Description |
|--------|-------------|
| `auth-test` | Interactive Hypergryph account login, OAuth2 flow, gacha history, and character roster via the web API |
| `resource-test` | CDN resource discovery, launcher API probing, and version diffing across regions/platforms |
| `gameassembly-extract` | Extract files (e.g. `GameAssembly.dll`) from the game's split ZIP archive on the CDN |

```bash
# Login and pull gacha history
cargo run --release --bin auth-test

# Probe CDN resources across all platforms/regions
cargo run --release --bin resource-test

# Extract GameAssembly.dll (or any file by pattern)
cargo run --release --bin gameassembly-extract
cargo run --release --bin gameassembly-extract -- "global-metadata.dat"
```

## Project Structure

```
backend/
├── Cargo.toml
├── src/
│   ├── lib.rs                  # Library root (pub mod core, extraction)
│   ├── main.rs
│   ├── bin/
│   │   ├── endfield.rs         # Unified extraction CLI
│   │   ├── auth_test.rs        # Account/gacha/roster tool
│   │   ├── resource_test.rs    # CDN resource prober
│   │   └── gameassembly_extract.rs  # Split ZIP file extractor
│   ├── core/
│   │   ├── endfield/
│   │   │   ├── config.rs       # Game config resolution
│   │   │   └── constants.rs    # App codes, channels, endpoints
│   │   └── hypergryph/
│   │       ├── account_service.rs  # OAuth2 login, token management
│   │       ├── binding.rs      # Account binding/roles
│   │       ├── launcher.rs     # Launcher API (versions, resources)
│   │       ├── launcher_web.rs # Launcher web endpoints
│   │       ├── u8.rs           # U8 platform (server list, channel tokens)
│   │       └── webview.rs      # Webview API (gacha, character data)
│   └── extraction/
│       ├── blc.rs              # BLC catalog parser + ChaCha20 VFS extraction
│       ├── chacha20.rs         # ChaCha20 (RFC7539) cipher
│       ├── crypto_constants.rs # Encryption keys and constants
│       ├── fair_guard_utils.rs # FairGuard anti-cheat obfuscation helpers
│       ├── lz4_inv.rs          # Custom LZ4 decompression
│       ├── pipeline.rs         # VFS download pipeline (resolve, list, download)
│       ├── sparkbuffer.rs      # SparkBuffer binary table → JSON parser
│       ├── texture.rs          # Texture2D extraction (BC7, ASTC, etc. → PNG)
│       ├── unity_extract.rs    # Unity SerializedFile + .chk container parsing
│       ├── unityfs_header.rs   # UnityFS bundle header parsing + decompression
│       ├── utils.rs            # Shared utilities (format_bytes, hex_dump, etc.)
│       ├── vfs_aes.rs          # Custom AES-128 with non-standard S-box
│       ├── vfs_decrypt.rs      # VFS AES-128 CTR decryption
│       ├── wwise_pck.rs        # Wwise audio PCK format decryption
│       └── zip_range.rs        # HTTP range-based split ZIP extraction
├── data/                       # Cached/extracted game data
└── output/                     # Default extraction output
```

## Extraction Modules

### Encryption & Decryption
- **`chacha20`** — RFC7539 ChaCha20 stream cipher used for VFS file encryption. Static 32-byte key.
- **`blc`** — BLC catalog parser. Each encrypted VFS directory has a `.blc` file listing all chunks with offsets, sizes, encryption flags, and IV seeds. Handles full download-decrypt-extract pipeline.
- **`vfs_aes`** — Custom AES-128 implementation with a non-standard S-box permutation used by the game's VFS layer.
- **`vfs_decrypt`** — AES-128 CTR mode decryption for VFS container headers and asset bundle data.
- **`crypto_constants`** — All encryption keys, S-boxes, and cipher constants.
- **`wwise_pck`** — Decryption for Wwise audio `.pck` files (`:)xD` magic format).

### Asset Parsing
- **`unityfs_header`** — UnityFS bundle format parser. Handles LZMA/LZ4 decompression, block info, directory entries.
- **`unity_extract`** — `.chk` container extraction (VFS header → decrypted assets → SerializedFile parsing). TextAsset and MonoBehaviour extraction.
- **`sparkbuffer`** — Binary table format used by Endfield for game data (character stats, item tables, quest configs, etc.). Parses type definitions, enums, beans, arrays, maps → JSON.
- **`texture`** — Texture2D extraction with GPU format decoding (BC7, ASTC, ETC2, DXT, etc.) → PNG output. Resolves `.resS` streaming data from VFS resources.

### Infrastructure
- **`pipeline`** — Shared download pipeline. Resolves game config → ZIP central directory → chunk download. Used by all extraction commands.
- **`zip_range`** — HTTP range-request based extraction from the game's split ZIP archive (multiple `.zip` parts on CDN).
- **`lz4_inv`** — Custom LZ4 decompression variant used by some game assets.
- **`fair_guard_utils`** — Helpers for handling FairGuard anti-cheat obfuscated data.
- **`utils`** — Shared utilities: `format_bytes()`, `hex_dump()`, `sanitize_filename()`.

## How It Works

1. **Config Resolution** — Queries the Hypergryph launcher API to get the current game version, CDN URLs, and resource manifest.
2. **ZIP Central Directory** — The game's assets are stored as a split ZIP archive across multiple CDN files. The pipeline fetches the central directory via HTTP range requests to know what files exist and where.
3. **VFS Containers** — Individual `.chk` files are downloaded and decrypted (AES-128 CTR) to reveal Unity asset bundles and SerializedFiles.
4. **ChaCha20 Directories** — 9 VFS directories use ChaCha20 encryption with per-file IV seeds stored in BLC catalogs. These contain game data tables, JSON configs, and Lua scripts.
5. **Asset Extraction** — Unity SerializedFiles are parsed to extract TextAssets (JSON configs), Texture2D (PNG images), and MonoBehaviour objects (gamedata).
6. **SparkBuffer Parsing** — Binary `.bytes` table files are automatically converted to human-readable JSON using the SparkBuffer format parser.

## Requirements

- Rust 2024 edition (nightly or recent stable)
- Internet connection (all data is fetched from the public CDN)
- No game installation required
