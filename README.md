# fluorite.moe

Rust toolkit for extracting and decrypting game assets from the Arknights: Endfield CDN. Downloads, decrypts, and parses VFS archives, Unity asset bundles, textures, game data tables, and more without needing the game installed.

## Installation

Requires Rust 2024 edition (nightly or recent stable) and an internet connection. No game installation needed since all data is fetched from the public CDN.

```bash
git clone https://github.com/eltik/fluorite.moe.git
cd fluorite.moe/backend
cargo build --release
```

## How It Works

1. **Config Resolution** queries the Hypergryph launcher API to get the current game version, CDN URLs, and resource manifest.
2. **ZIP Central Directory** is fetched via HTTP range requests from the game's split ZIP archive across multiple CDN files to locate all asset files.
3. **VFS Containers** are downloaded as `.chk` files and decrypted with AES-128 CTR to reveal Unity asset bundles and SerializedFiles.
4. **ChaCha20 Directories** use ChaCha20 encryption with per-file IV seeds stored in BLC catalogs. Nine directories contain game data tables, JSON configs, and Lua scripts.
5. **Asset Extraction** parses Unity SerializedFiles to extract TextAssets (JSON configs), Texture2D (PNG images), and MonoBehaviour objects (gamedata).
6. **SparkBuffer Parsing** converts binary `.bytes` table files into human-readable JSON using the SparkBuffer format parser.

## Usage

### endfield CLI

The unified CLI for all extraction tasks. All subcommands fetch data directly from the CDN.

#### `endfield info`

Display game configuration, version, patch info, and CDN resource URLs.

```bash
cargo run --release --bin endfield -- info
```

#### `endfield extract`

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
| `--json` | Extract TextAssets (JSON, text, and binary files with their real extensions) |
| `--textures` | Extract Texture2D as PNG images |
| `--gamedata` | Extract MonoBehaviour objects as JSON |
| `--meshes` | Export Mesh objects as glTF binary (`.glb`) |
| `--all` | Extract all types (default if no flags given) |
| `--dir <HASH>` | Target a specific VFS directory hash |
| `--output <PATH>` | Output directory (default: `./output`) |
| `--scan-all` | Process all `.chk` files (default: smallest per directory) |

Output structure:
```
output/
  gamedata/           # TextAssets + MonoBehaviours, mirroring the game's asset paths
    beyond/...        # e.g. beyond/dynamicassets/gameplay/guide/guide_group_crop_farm_1.json
    <owner>/          # assets only referenced by a prefab sit in a folder named after it
    _unsorted/        # no container entry; MonoBehaviours grouped by script type
  meshes/             # Mesh as .glb (glTF 2.0), same layout as textures
  textures/           # Texture2D as PNG, mirroring the game's asset paths
    beyond/...        # from assets/beyond/... (e.g. beyond/initialassets/ui/sprites/login/)
    packages/...      # textures shipped in Unity packages
    <owner>/          # textures only referenced by a prefab/material sit in a folder named after it
    _unsorted/        # fallback when a bundle has no container entry (<name>__<pathid>.png)
    index.jsonl       # one line per texture: path, name, container path, placement, size, format
```

TextAssets keep their real type: the extension comes from the container path (Unity's mandatory `.bytes` suffix is dropped, so `hero.skel.bytes` becomes `hero.skel`), then the asset name, then magic bytes (`.glb`, `.webp`, `.png`, `.ogg`, `.bnk`, ...), then JSON/text detection. Textures are flipped to top-down orientation (Unity stores rows bottom-up). Meshes are converted from Unity's left-handed space (X negated, winding reversed, V flipped); skinned meshes are exported in bind pose without bones.

#### `endfield extract-file`

Extract a single `.chk` file by name or substring match.

```bash
# Extract a specific chunk file
cargo run --release --bin endfield -- extract-file "CAB-abc123"

# From a specific directory
cargo run --release --bin endfield -- extract-file "CAB-abc123" --dir 7064D8E2
```

#### `endfield decrypt`

Extract every BLC-catalogued VFS directory (everything except the Unity bundle directories), decrypting ChaCha20 entries. Directories and file lists come from the CDN's `.blc` catalogs, so new directories are picked up automatically.

```bash
# Extract all BLC directories
cargo run --release --bin endfield -- decrypt

# A specific directory
cargo run --release --bin endfield -- decrypt --dir 42A8FCA6
```

| Flag | Description |
|------|-------------|
| `--dir <HASH>` | Extract a specific directory (default: all BLC directories) |
| `--output <PATH>` | Output directory (default: `./output/decrypted`) |

BLC directories (game 1.6):

| Hash | Group | Contents |
|------|-------|----------|
| `42A8FCA6` | Table | SparkBuffer game data tables (parsed to JSON) |
| `775A31D1` | JsonData | Level/AI configs (`.json` names, but a binary encoding) |
| `19E3AE45` | Lua | Lua scripts (base64 + XXTEA in the game files; written as decrypted source) |
| `D6E622F7` / `3C9D9D2D` | ExtendData / InitialExtendData | Binary lookup data |
| `1CDDBF1F` | BundleManifest | `manifest.hgmmap` |
| `07A1BB91` / `24ED34CF` | InitAudio / Audio | Wwise `.pck` banks |
| `55FC21C6` | Video | CRI `.usm` videos (~7 GB) |
| `A63D7E6A` / `F84BF5E6` / `C3442D43` / `23D53F5D` | IV / Terrain / Streaming / DynamicStreaming | Engine level data (~7 GB) |
| `DAFE52C9` / `F151B649` | IFixPatchOut / HotfixAudio | Usually empty |

Output structure:
```
output/decrypted/
  Table/                 # One folder per catalog group
    blc_metadata.txt     # BLC catalog info (group name, version, file count)
    TableCfg/SomeTable.bytes  # Original path, minus the leading `Data/`
    TableCfg/SomeTable.json   # SparkBuffer .bytes auto-parsed to JSON
  Lua/LuaScripts/...
```

#### `endfield list`

List all VFS directories on the CDN with file counts, sizes, and patch files.

```bash
cargo run --release --bin endfield -- list
```

#### `endfield scan`

Scan VFS archives for asset names, bundle paths, and embedded strings.

```bash
# Quick scan (smallest .chk per directory)
cargo run --release --bin endfield -- scan

# Full scan (all .chk files)
cargo run --release --bin endfield -- scan --scan-all
```

#### `endfield survey`

Census of all Unity object types, class IDs, counts, and sizes across VFS directories.

```bash
cargo run --release --bin endfield -- survey --scan-all
```

#### `endfield update`

Incrementally sync all assets (`extract --all --scan-all` + `decrypt`) to the latest game and hot-update version. Built to run from cron: when nothing changed upstream it exits in ~1s after two launcher API calls; otherwise it re-processes only the `.chk` files and encrypted directories whose CRC32/size or patch MD5s changed.

```bash
# Sync everything (first run is a full extraction)
cargo run --release --bin endfield -- update

# Only check whether an update is available
cargo run --release --bin endfield -- update --check
```

| Flag | Description |
|------|-------------|
| `--output <PATH>` | Output directory (default: `./output`) |
| `--state <PATH>` | State file tracking processed versions and entries (default: `./data/update-state.json`) |
| `--force` | Re-process everything regardless of state |
| `--check` | Report only; exit `10` if an update is available, `0` if up to date |
| `--no-textures` | Skip texture extraction (those chunks are re-processed by a later run with textures) |
| `--no-meshes` | Skip mesh export |
| `--skip-dir <HASH,...>` | Leave these BLC directories alone; the version fingerprint is not recorded, so a later run without it fetches them |

Exit codes: `0` success or up to date, `1` some entries failed (they are retried next run; the version fingerprint only advances after a clean run), `10` update available (`--check` only). A lock file next to the state prevents overlapping runs. Each run appends a summary line to `<output>/update-log.jsonl`.

When the texture layout changes, the next `update` sorts existing PNGs instead of re-extracting them. For each chunk that is unchanged upstream, it downloads the chunk and reads only its bundle metadata, then moves the old flat PNGs (`textures/<name>.png`) into the new folders. Nothing is decoded and no PNG is rewritten. Chunks that did change upstream are extracted normally. When a name matches several textures, the file's dimensions pick one; if they can't, it goes to `textures/_ambiguous/` with its candidate paths listed in `candidates.jsonl`. State is saved only after the sort finishes, so an interrupted run starts the sort again on the next run. Leftover top-level PNGs (textures no longer in the game) are deleted once every chunk succeeds.

When the extraction output format changes (`EXTRACT_VERSION` in `update.rs`), every bundle chunk is re-downloaded and re-extracted. PNGs from unchanged chunks that already match the current texture layout are kept instead of decoded again. Once every chunk succeeds, the legacy flat gamedata (top-level files and `gamedata/monobehaviour/`) and `textures/_ambiguous/` are removed.

#### `endfield repair`

Decrypts Lua scripts that older versions wrote still encrypted, then flips PNGs extracted before the orientation fix in place, using each chunk's texture index, and marks those chunks as current in the state file. It needs no network access, and a per-chunk journal makes it safe to interrupt and rerun.

#### Fixing an existing output

`backend/scripts/repair-assets.sh` brings output from older versions up to date: it runs `repair`, then `update` (re-extracting gamedata into the new layout, exporting meshes, extracting textures that went missing, and downloading the BLC directories). It skips the engine level-data directories unless you pass `--all`, and warns when free disk space looks too low. Rerun it after an interruption.

#### Scheduling

`backend/scripts/auto-update.sh` rebuilds the binary if needed, runs `endfield update`, and logs to `backend/data/logs/update.log` (rotated at 10MB). Add it to `crontab -e`:

```cron
*/15 * * * * /abs/path/to/fluorite.moe/backend/scripts/auto-update.sh
```

### auth-test

Interactive Hypergryph account login, OAuth2 flow, gacha history, and character roster via the web API.

```bash
cargo run --release --bin auth-test
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
│   │   └── auth_test.rs        # Account/gacha/roster tool
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
│       ├── gamedata.rs         # TextAsset/MonoBehaviour export by container path
│       ├── lua.rs              # Lua script decryption (base64 + XXTEA)
│       ├── lz4_inv.rs          # Custom LZ4 decompression
│       ├── mesh.rs             # Unity Mesh → glTF binary (.glb)
│       ├── pipeline.rs         # VFS download pipeline (resolve, list, download)
│       ├── sparkbuffer.rs      # SparkBuffer binary table → JSON parser
│       ├── texture.rs          # Texture2D extraction (BC7, ASTC, etc. → PNG)
│       ├── unity_extract.rs    # Unity SerializedFile + .chk container parsing
│       ├── unityfs_header.rs   # UnityFS bundle header parsing + decompression
│       ├── update.rs           # Incremental update state + change detection
│       ├── utils.rs            # Shared utilities (format_bytes, hex_dump, etc.)
│       ├── vfs_aes.rs          # Custom AES-128 with non-standard S-box
│       ├── vfs_decrypt.rs      # VFS AES-128 CTR decryption
│       ├── wwise_pck.rs        # Wwise audio PCK format decryption
│       └── zip_range.rs        # HTTP range-based split ZIP extraction
├── data/                       # Cached/extracted game data
└── output/                     # Default extraction output
```

## Extraction Modules

### Encryption and Decryption

- **chacha20** is an RFC7539 ChaCha20 stream cipher used for VFS file encryption with a static 32-byte key.
- **blc** parses BLC catalogs. Each encrypted VFS directory has a `.blc` file listing all chunks with offsets, sizes, encryption flags, and IV seeds. Handles the full download-decrypt-extract pipeline.
- **vfs_aes** is a custom AES-128 implementation with a non-standard S-box permutation used by the game's VFS layer.
- **vfs_decrypt** handles AES-128 CTR mode decryption for VFS container headers and asset bundle data.
- **crypto_constants** stores all encryption keys, S-boxes, and cipher constants.
- **wwise_pck** decrypts Wwise audio `.pck` files using the `:)xD` magic format.

### Asset Parsing

- **unityfs_header** parses the UnityFS bundle format, handling LZMA/LZ4 decompression, block info, and directory entries.
- **unity_extract** handles `.chk` container extraction (VFS header to decrypted assets to SerializedFile parsing) with TextAsset and MonoBehaviour extraction.
- **sparkbuffer** parses the binary table format used by Endfield for game data (character stats, item tables, quest configs). Converts type definitions, enums, beans, arrays, and maps to JSON.
- **texture** extracts Texture2D with GPU format decoding (BC7, ASTC, ETC2, DXT) to PNG output, resolving `.resS` streaming data from VFS resources.

### Infrastructure

- **pipeline** is the shared download pipeline that resolves game config to ZIP central directory to chunk download, used by all extraction commands.
- **zip_range** handles HTTP range-request based extraction from the game's split ZIP archive (multiple `.zip` parts on CDN).
- **lua** decrypts the `Lua` group's scripts: each is base64 text wrapping an XXTEA payload whose last word is the plaintext length.
- **lz4_inv** is a custom LZ4 decompression variant used by some game assets.
- **fair_guard_utils** provides helpers for handling FairGuard anti-cheat obfuscated data.
- **utils** contains shared utilities: `format_bytes()`, `hex_dump()`, `sanitize_filename()`.

## Conclusion

fluorite.moe provides complete access to Arknights: Endfield game assets through reverse-engineered encryption and file format parsers. The `endfield` CLI handles the full pipeline from CDN download through decryption to structured output.
