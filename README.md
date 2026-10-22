# fluorite.moe

Rust toolkit for extracting and decrypting game assets from the Arknights: Endfield CDN. Downloads, decrypts, and parses VFS archives, Unity asset bundles, textures, game data tables, and more without needing the game installed.

## Installation

Requires Rust 2024 edition and an internet connection. No game installation needed since all data is fetched from the public CDN. `backend/rust-toolchain.toml` pins the toolchain (currently 1.98.0, with rustfmt and clippy), so rustup installs that version on the first build and a later update of stable can't break the unattended cron builds. Bump the pin deliberately after checking `cargo clippy --release --all-targets` and `cargo test --release` pass.

```bash
git clone https://github.com/eltik/fluorite.moe.git
cd fluorite.moe/backend
cargo build --release
```

The configuration of the whole project is one file, `backend/.env`;
`backend/.env.example` is the template with every variable and its default.

For a production deployment on a VPS — the API, the web server, the reverse
proxy, pm2 or systemd, and the bucket — see
[docs/deployment.md](docs/deployment.md).

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

TextAssets keep their real type: the extension comes from the container path (Unity's mandatory `.bytes` suffix is dropped, so `hero.skel.bytes` becomes `hero.skel`), then the asset name, then magic bytes (`.glb`, `.webp`, `.png`, `.ogg`, `.bnk`, ...), then JSON/text detection. Textures are flipped to top-down orientation (Unity stores rows bottom-up). Meshes are converted from Unity's left-handed space (X negated, winding reversed, V flipped); skinned meshes are exported in bind pose without bones (use [`endfield-models`](#endfield-models) for rigged models). Meshes stored as Unity `CompressedMesh` (bit-packed vertices, normals, tangents, UVs, colors, skin weights and indices) are decoded too; Endfield also marks some meshes as compressed whose vertices are in the regular vertex data, so the payload decides, not the flag.

#### `endfield extract-file`

Extract a single `.chk` file by name or substring match.

```bash
# Extract a specific chunk file
cargo run --release --bin endfield -- extract-file "CAB-abc123"

# From a specific directory
cargo run --release --bin endfield -- extract-file "CAB-abc123" --dir 7064D8E2
```

#### `endfield decrypt`

Extract every BLC-catalogued VFS directory (everything except the Unity bundle directories), decrypting ChaCha20 entries. Directories and file lists come from the `.blc` catalogs, so new directories are picked up automatically: those in the base game archive, plus those only the launcher's patch index lists (the voice languages). A directory with files in the patch index is read through its hot-update catalog on the patch CDN (`<main resource>/VFS/<HASH>/<HASH>.blc`), so hot-updated chunks are included; its chunks come from the archive when it holds them and from the CDN otherwise (including chunks the patch index does not list, such as AudioEnglish's second stream package). CDN chunks are downloaded as parallel 32 MB range requests straight into a scratch file beside the output rather than held in memory (the voice packages are ~1.5 GB each), and a chunk that is one plain file is moved into place. A failed range request is retried up to four times, continuing from its last written byte; an interrupted run downloads the chunk again (its leftover `.<HASH>-<md5>.chk.part` is deleted when the directory is next extracted). A group's `blc_metadata.txt` is written only after every file extracted without errors.

```bash
# Extract all BLC directories
cargo run --release --bin endfield -- decrypt

# A specific directory, by hash or group name
cargo run --release --bin endfield -- decrypt --dir 42A8FCA6
cargo run --release --bin endfield -- decrypt --dir AudioEnglish
```

| Flag | Description |
|------|-------------|
| `--dir <HASH\|GROUP>` | Extract a specific directory, by hash or by a group name from the table below (default: all BLC directories) |
| `--no-voice` | Skip the four voice-language groups |
| `--output <PATH>` | Output directory (default: `./output/decrypted`) |
| `--state <PATH>` | Update state file; only used to find the MemoryPack schema store beside it (default: `./data/update-state.json`) |

BLC directories (game 1.6):

| Hash | Group | Contents |
|------|-------|----------|
| `42A8FCA6` | Table | SparkBuffer game data tables (parsed to JSON) |
| `775A31D1` | JsonData | Level/AI configs: plain JSON, or MemoryPack under a `.json` name (decoded to JSON; the raw bytes are kept as `.mpk`) |
| `19E3AE45` | Lua | Lua scripts (base64 + XXTEA in the game files; written as decrypted source) |
| `D6E622F7` / `3C9D9D2D` | ExtendData / InitialExtendData | Binary lookup data |
| `1CDDBF1F` | BundleManifest | `manifest.hgmmap` |
| `07A1BB91` / `24ED34CF` | InitAudio / Audio | Wwise `.pck` banks |
| `55FC21C6` | Video | CRI `.usm` videos (~7 GB) |
| `A63D7E6A` / `F84BF5E6` / `C3442D43` / `23D53F5D` | IV / Terrain / Streaming / DynamicStreaming | Engine level data (~7 GB) |
| `DAFE52C9` / `F151B649` | IFixPatchOut / HotfixAudio | Usually empty |
| `E1E7D7CE` / `A31457D0` / `F668D4EE` / `E9D31017` | AudioChinese / AudioEnglish / AudioJapanese / AudioKorean | Voice lines: a `*_banks.pck` (~2 MB) and 1–2 stream `.pck` packages (~1.5–1.9 GB per language). Only in the patch index, not the base archive; stored unencrypted |

Once a voice group is extracted to `output/decrypted/Audio<Language>/`, the audio API serves its clips from disk and stops streaming that language from the CDN.

Output structure:
```
output/decrypted/
  Table/                 # One folder per catalog group
    blc_metadata.txt     # BLC catalog info (group name, version, file count)
    TableCfg/SomeTable.bytes  # Original path, minus the leading `Data/`
    TableCfg/SomeTable.json   # SparkBuffer .bytes auto-parsed to JSON
  Lua/LuaScripts/...
  JsonData/Json/...
    Foo.json             # MemoryPack decoded to JSON (already-plain JSON is left as is)
    Foo.mpk              # The original MemoryPack bytes, re-decoded by later runs
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

Incrementally sync all assets (`extract --all --scan-all` + `decrypt`) to the latest game and hot-update version. Built to run from cron: when nothing changed upstream it exits in ~1s after two launcher API calls; otherwise it re-processes only the `.chk` files and encrypted directories whose CRC32/size or patch MD5s changed. A directory read through its CDN catalog (hot-updated, or only in the patch index like the voice languages) is tracked by that catalog's bytes plus its patch MD5s, since the catalog can name CDN chunks the patch index leaves out; such directories re-extract once after upgrading from a version that read the archive catalog. When the CDN has no catalog for a hot-updated directory (HTTP 403/404), the archive catalog is used and the digest records `cdn-blc=missing`, so the directory re-extracts if a CDN catalog appears later.

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
| `--skip-dir <HASH\|GROUP,...>` | Leave these BLC directories alone (hashes or group names such as `Video`); the version fingerprint is not recorded, so a later run without it fetches them |
| `--no-voice` | Shorthand for `--skip-dir AudioChinese,AudioEnglish,AudioJapanese,AudioKorean` (~6.5 GB of voice packages), with the same fingerprint behaviour |
| `--no-schema` | Don't regenerate the MemoryPack schema even if the client binary changed |

`FLUORITE_DOWNLOAD_CONCURRENCY` (default `4`) sets how many `.chk` chunks download at once; lower it to `1` or `2` on a slow link so each download finishes before the CDN stalls it.

Exit codes: `0` success or up to date, `1` some entries failed (they are retried next run; the version fingerprint only advances after a clean run), `10` update available (`--check` only). A lock file next to the state prevents overlapping runs. Each run appends a summary line to `<output>/update-log.jsonl`: version and `res_version`, chunk/sort/BLC counts (`chk_dirty`, `chk_ok`, `chk_failed`, `sort_*`, `blc_*`), asset counts from the chunk phase (`tex_extracted`, `tex_failed`, `mesh_exported`, `mesh_failed`), decode counts summed over the BLC directories it extracted (`lua_decrypted`, `memorypack_decoded`, `memorypack_failed`, and `memorypack_schemas`, which counts decoded files per schema), the game version of the newest MemoryPack schema in use (`schema_version`), `schema_regenerated` plus `schema_decoded`/`schema_failed` (JsonData totals after re-decoding) or `schema_error` when a regeneration was attempted, and `duration_secs`.

After the BLC phase, if the archive's `GameAssembly.dll` (CRC32 and size) differs from the one recorded in the state (`memorypack_client`), `update` runs the `schema-gen` flow below. A failed or refused regeneration doesn't fail the update. It logs `schema_error` and keeps the current schemas, `health` warns, and the next run tries again.

#### `endfield schema-gen`

Regenerates the MemoryPack schema that decodes JsonData, straight from the client's IL2CPP files. It downloads `GameAssembly.dll` (~320 MB) and `global-metadata.dat` (~74 MB) from the game archive into memory, compiles the schema, and learns each directory's root types from the extracted `JsonData/Json` tree. Then it decodes every file with the new schema and with the schemas in use. It refuses to install a schema that decodes fewer files than the newest one in use. Otherwise it installs the schema and re-decodes JsonData from the `.mpk` sources. It takes about 5 seconds plus the download, and peaks around 0.75 GB of memory.

```bash
# For the live client (downloads the two files)
cargo run --release --bin endfield -- schema-gen

# From local client files
cargo run --release --bin endfield -- schema-gen --dll GameAssembly.dll \
  --metadata global-metadata.dat --game-version 1.6.4
```

| Flag | Description |
|------|-------------|
| `--state <PATH>` | State file; schemas go to `memorypack-schema/` beside it (default: `./data/update-state.json`) |
| `--output <PATH>` | Output directory holding `decrypted/JsonData` (default: `./output`) |
| `--dll <PATH>` / `--metadata <PATH>` / `--game-version <V>` | Use local client files instead of downloading (all three together; the state is not touched) |
| `--json-dir <PATH>` | `Json/` tree to learn from and validate against (default: the JsonData group's) |
| `--force` | Install even if the new schema decodes fewer files |

Exit codes: `0` installed or already current, `2` refused because it would decode fewer files, `3` the client files have a layout the IL2CPP reader doesn't know (it needs updating), `1` other errors.

Schemas are stored as `data/memorypack-schema/<game version>-<GameAssembly.dll crc32>.json.gz`. Every command that decodes JsonData (`update`, `decrypt`, `repair`, `schema-gen`) loads all stored schemas, newest first by game version and then generation time, followed by the schema embedded in the binary. Each file is decoded by the first schema that consumes it exactly, so a new schema never loses a file an older one could decode.

#### `endfield health`

Reads `<output>/update-log.jsonl` and prints one `OK:` / `WARN:` / `FAIL:` line per check. Exits `0` if everything is OK, `1` on any warning, `2` on any failure. Old log lines without the newer counters are fine, and blank or garbled lines are skipped.

```bash
cargo run --release --bin endfield -- health --output ./output --max-age-hours 26
```

| Check | WARN | FAIL |
|-------|------|------|
| Freshness | | Log missing or empty, or the newest run is older than `--max-age-hours` (default 26) |
| Run failures | Newest run had `chk_failed + sort_failed + blc_failed > 0` | The last 3 runs all had failures |
| MemoryPack | Newest run that decoded any JsonData files had more than 1% fail (after any regeneration); the game version's major.minor differs from that of the schema in use (`schema_version`); or the newest regeneration attempt failed (`schema_error`). Run `endfield schema-gen` and see `backend/scripts/memorypack-schema/README.md` | |
| Textures / meshes | `tex_failed` or `mesh_failed` exceeds the previous run that logged it by more than max(50, 10%) | |

When the texture layout changes, the next `update` sorts existing PNGs instead of re-extracting them. For each chunk that is unchanged upstream, it downloads the chunk and reads only its bundle metadata, then moves the old flat PNGs (`textures/<name>.png`) into the new folders. Nothing is decoded and no PNG is rewritten. Chunks that did change upstream are extracted normally. When a name matches several textures, the file's dimensions pick one; if they can't, it goes to `textures/_ambiguous/` with its candidate paths listed in `candidates.jsonl`. State is saved only after the sort finishes, so an interrupted run starts the sort again on the next run. Leftover top-level PNGs (textures no longer in the game) are deleted once every chunk succeeds.

When the extraction output format changes (`EXTRACT_VERSION` in `update.rs`), every bundle chunk is re-downloaded and re-extracted. PNGs from unchanged chunks that already match the current texture layout are kept instead of decoded again. Once every chunk succeeds, the legacy flat gamedata (top-level files and `gamedata/monobehaviour/`) and `textures/_ambiguous/` are removed.

#### `endfield repair`

Decrypts Lua scripts that older versions wrote still encrypted and decodes JsonData MemoryPack files left binary (re-decoding from `.mpk` sources with the stored schemas), then flips PNGs extracted before the orientation fix in place, using each chunk's texture index, and marks those chunks as current in the state file. It needs no network access, and a per-chunk journal makes it safe to interrupt and rerun.

#### Fixing an existing output

`backend/scripts/repair-assets.sh` brings output from older versions up to date: it runs `repair`, then `update` (re-extracting gamedata into the new layout, exporting meshes, extracting textures that went missing, and downloading the BLC directories). It skips the engine level-data directories unless you pass `--all`, and warns when free disk space looks too low. The voice-language groups (~6.5 GB) are included; pass `--no-voice` to leave them out. Rerun it after an interruption.

#### Scheduling

`backend/scripts/auto-update.sh` rebuilds the binary if needed, runs `endfield update`, and logs to `backend/data/logs/update.log` (rotated at 10MB). Add it to `crontab -e`:

```cron
*/15 * * * * /abs/path/to/fluorite.moe/backend/scripts/auto-update.sh
0 9 * * * FLUORITE_ALERT_WEBHOOK=https://discord.com/api/webhooks/... /abs/path/to/fluorite.moe/backend/scripts/health-check.sh >> /abs/path/to/fluorite.moe/backend/data/logs/health.log 2>&1
```

`backend/scripts/health-check.sh` rebuilds the binary if needed and runs `endfield health` (arguments pass through, e.g. `--max-age-hours 30`). When the result isn't OK, it posts a macOS notification (when `osascript` is available) and, if `FLUORITE_ALERT_WEBHOOK` is set, POSTs `{"content": ..., "text": ...}` to it, which works for both Discord and Slack incoming webhooks. It exits with the health exit code. Both scripts build with the toolchain pinned in `backend/rust-toolchain.toml`.

### endfield-models

Exports rigged, skinned GLBs (skeleton, skin weights, materials with embedded textures) for operators, NPCs, enemies and weapons. `endfield extract --meshes` only writes bind-pose geometry per mesh; this tool follows each entity prefab through its renderers, materials and avatar to build one self-contained model. Output goes to `output/models/<kind>/<id>.glb` (`<kind>` is `operator`, `npc`, `enemy` or `weapon`) plus `output/models/index.json`, which the API reads as the `models` root. Entries from earlier runs with a different `--filter` are kept in `index.json`.

**The scan store.** A prefab, its meshes, materials and textures live in different bundles of different ~1 GB chunks, most of it texture pixels. `scan` downloads each chunk once and keeps only what rig export needs: the SerializedFiles (a few MB per chunk), the resource streams of bundles holding meshes, and a per-chunk index. Exports then resolve cross-bundle references from the store without downloading anything. The store defaults to `<output>/.store` (`sf/`, `res/`, `index/`). `export` scans missing chunks on its own unless `--no-auto-scan` is set. Textures come from the existing extraction output (`--data-root`, default `./output`), so run `endfield extract --textures` first.

```bash
# Download chunks once into the store (default: every bundle chunk not yet scanned)
cargo run --release --bin endfield-models -- scan --store output/models/.store --jobs 3

# Export operators; --filter is a substring of the prefab container path
cargo run --release --bin endfield-models -- export \
  --filter postmodels/characters --output output/models

# Add idle animations. Keywords are comma-separated and tried in order; the first
# that matches any clip wins. --max-anims caps clips per entity (default 4)
cargo run --release --features acl --bin endfield-models -- export \
  --filter postmodels/characters --output output/models \
  --anims uiteam_idle,dialog_single_idle,idle --max-anims 2

# Every clip of every exported operator/NPC/enemy as its own animation-only GLB
cargo run --release --features acl --bin endfield-models -- clips \
  --store output/models/.store --models output/models

# Delete stored files the store's relevance rules no longer keep
cargo run --release --bin endfield-models -- prune --store output/models/.store

# Summarize objects and container paths of a chunk (--chk is repeatable)
cargo run --release --bin endfield-models -- inspect --chk <HASH> --filter postmodels

# Summarize AnimationClip / Animator data in a store
cargo run --release --bin endfield-models -- anim-survey --store output/models/.store
```

| Subcommand | Purpose |
|---|---|
| `chunks` | List the bundle `.chk` files with their sizes |
| `scan` | Download chunks into the store (`--chk`, `--jobs`, `--cache-dir`) |
| `export` | Write GLBs for prefabs matching `--filter` (see below) |
| `clips` | Write one animation-only GLB per clip of every exported model (see below; `--kind`, `--id`, `--jobs`, `--skip-existing`, `--no-root-motion`) |
| `prune` | Delete stored files the relevance rules no longer keep, and print the space freed |
| `inspect` | Summarize Unity objects and container paths of chunks |
| `anim-survey` | Summarize clips and animators in a store, as JSON |
| `dump` | Print objects of a stored SerializedFile as JSON (debugging) |

Useful `export` flags: `--filter` (required) picks prefabs by container path, e.g. `postmodels/characters` (operators and NPCs), `postmodels/enemies`, `prefabs/weapons`; `--id` (repeatable) limits to entity ids containing a string; `--all-textures` embeds every referenced texture instead of base color and normal maps; `--images auto|png|webp`; `--max-texture-size`; `--all-lods`; `--source-dir` reads pre-extracted files instead of the store.

**Animations.** Endfield clips are ACL-compressed, so `--anims` needs the decompressor, which is compiled from C++ only with the `acl` feature and needs a C++17 compiler:

```bash
cargo build --release --features acl --bin endfield-models
```

ACL v2.1 (and RTM) is vendored in `backend/vendor/acl` under the MIT license (`LICENSE-acl`, `LICENSE-rtm`); `VERSION.txt` records the pinned commits. Default builds need no C++ and export models without clips. Clips are found by container path (`.../<code>/animations/**/*<keyword>*.fbx`, where `<code>` is the entity id's last `_` segment, e.g. `chr_0005_chen` is `chen`).

**Per-clip GLBs.** `clips` reads `<models>/index.json` and writes, for each operator, NPC and enemy (not weapons), every clip found the same way as `--anims` (minus additive clips) to `<models>/clips/<kind>/<id>/<clip>.glb`, plus `index.json` (one entry per clip: `file`, display `name`, `clip`, `source` (`3c`, `battle`, `dialog`, `interact`, `ui` or `other`, from the animation folder), `duration`, `samples`, `fps`, `loop` (the clip's Loop Time), `humanoid`, `tracks`, `matched`, `size`, `rootMotion`, `rootNode`, `rootStart`, `rootEnd`). A clip GLB has no meshes: only the nodes its channels target (and their ancestors below the Animator's node) and one animation, so three.js' `AnimationMixer` on the loaded model plays it by node name. Node names are the ones three.js gives the model's nodes (it suffixes repeated names `_1`, `_2`…), so tracks resolve to exactly one node. Most Endfield clips are Unity Humanoid clips: the body is stored as muscle curves, which are turned into bone rotations through the model's Avatar (Endfield's legs have three extra muscles each). Root motion is played by `rootNode`: for humanoid clips it becomes translation/rotation channels on the Animator's node, for generic clips it is already the travel of the Avatar's root motion bone (`Root`). To play in place, hold `rootNode`'s position at `rootStart`. Keys are reduced within 0.1 mm / 0.001 rad and rotations stored as normalized shorts; clips identical across models (NPC variants of operators, enemy variants) are hard-linked. `<models>/clips/summary.json` lists counts and every skipped clip with its reason (additive, other skeleton, plain Mecanim curves, Avatar not found).

**In the catalog.** The API links a rigged GLB as `model.rigged` (path relative to the `models` root, served at `/files/models/...`) on the catalog detail of operators, enemies and weapons; the Studio loads it when present. The lookup matches the id against the GLB file names listed in `index.json`, then tries `<kind>/<id>.glb` (also the singular and `characters/`). If neither exists it falls back to a variant prefab of the id: templates such as `eny_0054_hsmino` only have `eny_0054_hsmino1` or `eny_0054_hsmino_chrdg`, and some weapons only `<id>_refined`. A digit suffix counts only after a letter (`wpn_x_0009` never matches `wpn_x_00091`); digit variants win over `_` variants, then the shortest name. Without `index.json`, no model is linked. The API re-reads the index when its mtime changes. When `clips` wrote a non-empty clip index for that GLB, `model.clips` is its path (e.g. `clips/operator/chr_0005_chen/index.json`, served at `/files/models/clips/...`, with each clip's `file` next to it).

### auth-test

Interactive Hypergryph account login, OAuth2 flow, gacha history, and character roster via the web API.

```bash
cargo run --release --bin auth-test
```

## Web app

The extracted output is served by two processes: `fluorite-api` (Rust, axum), a
read-only HTTP API over the output directory, and the frontend (TanStack Start on
Bun), which renders pages server-side and proxies the API so the site needs one
public origin. `endfield update` keeps the output current underneath them.

```
                      ┌────────────── browser ──────────────┐
                      │  pages, /assets/*  │  /api /files    │
                      │                    │  /thumb /audio/*│
                      └─────────┬──────────┴────────┬────────┘
                                │ https (reverse proxy: TLS, gzip)
                                ▼                   ▼
                     ┌───────────────────────────────────────┐
                     │ frontend  (Bun, .output/server)  :3000│
                     │  SSR pages ── fetch ──┐               │
                     │  same-origin proxy ───┤ (streams,     │
                     └───────────────────────┼─Range, 304)───┘
                                             ▼ FLUORITE_API_URL
                     ┌───────────────────────────────────────┐
                     │ fluorite-api  (Rust)   127.0.0.1:8787 │
                     │  /api/* JSON, /files, /thumb, /audio  │
                     └───────┬───────────────────────┬───────┘
                   read-only │                       │ read/write
                             ▼                       ▼
          backend/output/  (~80 GB)         FLUORITE_CACHE_DIR
          backend/data/update-state.json    (thumbs, audio transcodes)
                             ▲
                             │ writes (cron, every 15 min)
                endfield update  (backend/scripts/auto-update.sh)
```

The API polls `<output>/update-log.jsonl` (every `FLUORITE_INDEX_POLL_SECS`) and
rebuilds its search index when an update run appends to it, so neither server needs a
restart when new game data lands.

### Running it

Requirements: the Rust toolchain (pinned in `backend/rust-toolchain.toml`), Bun
>= 1.3.14 (older versions can't parse the bundled React server renderer), an output
directory produced by `endfield update`, and `vgmstream-cli` + `ffmpeg` on `PATH` for
audio playback.

```bash
# Development: API (release build) + Vite dev server with HMR on :3000.
# Reuses an API already answering on :8787. Ctrl-C stops both.
scripts/dev.sh

# Production: builds what is missing or stale, then runs both. Site on :3000.
PORT=3000 HOST=127.0.0.1 scripts/serve.sh
scripts/serve.sh --rebuild     # force both builds

# Or by hand
(cd backend && FLUORITE_OUTPUT=$PWD/output cargo run --release --bin fluorite-api)
(cd frontend && bun install && bun run build && PORT=3000 bun run start)
```

For a server, `scripts/systemd/` has example units: `fluorite-api.service`,
`fluorite-web.service`, and `fluorite-update.service` + `.timer` (the systemd
equivalent of the cron line under [Scheduling](#scheduling)). They assume a checkout
at `/var/www/fluorite.moe` and run as root; adjust paths and build both parts once
before enabling them. Put a reverse proxy (Caddy, nginx) in front of the web port for
TLS, HSTS and compression of HTML; the API already compresses its JSON.

### Configuration

Everything lives in one file, `backend/.env`; `backend/.env.example` is the
template with every variable and its default. The API, the unpacker and the
scripts all read it — the Rust binaries load it themselves, the shell scripts
source it — so one file configures the whole project:

```bash
cp backend/.env.example backend/.env   # then fill in the blanks
```

A variable that is already in the environment wins over the file, so a systemd
`Environment=`, a `FOO=1 ./endfield` or a `docker run -e` overrides it without
anyone having to edit the file. `FLUORITE_ENV_FILE` points the binaries at a
file somewhere else.

**fluorite-api** (see `backend/src/api/config.rs`):

| Variable | Default | Meaning |
|---|---|---|
| `FLUORITE_API_ADDR` | `127.0.0.1:8787` | Listen address |
| `FLUORITE_OUTPUT` | `./output`, else `<crate>/output` | Extraction output to serve (read-only) |
| `FLUORITE_CACHE_DIR` | `<output>/.cache` | Thumbnail and audio cache (must be writable) |
| `FLUORITE_STATE` | `<output>/../data/update-state.json` | Update state, for the version shown in `/api/status` |
| `FLUORITE_INDEX_POLL_SECS` | `60` | How often to check `update-log.jsonl` and rebuild the search index |
| `FLUORITE_AUDIO_REMOTE` | `1` | Read voice packages from the game CDN (`0` disables) |
| `FLUORITE_AUDIO_DIRS` | unset | Extra directories of `.pck` files (path-list separated) |
| `FLUORITE_AUDIO_CACHE_MB` | `5120` (5 GB) | Cap of the transcoded clip cache; swept hourly, oldest clips first |
| `FLUORITE_VGMSTREAM` / `FLUORITE_FFMPEG` | found on `PATH` | Audio transcoder binaries |
| `RUST_LOG` | `info` | Log filter |

**frontend** (see `frontend/README.md`):

| Variable | Default | Meaning |
|---|---|---|
| `PORT` / `HOST` | `3000` / all interfaces | Listen address of the production server |
| `FLUORITE_API_URL` | `http://127.0.0.1:8787` | Where the web server reaches the API (SSR and proxy) |
| `FLUORITE_PUBLIC_API_URL` | same origin (prod), `FLUORITE_API_URL` (dev) | API base handed to browsers |
| `FLUORITE_PROXY` | on | `0` disables the `/api`, `/files`, `/thumb`, `/audio/*` proxy |
| `VITE_API_URL` | unset | Build-time fallback for both API URLs |

Both API URLs are read at server start and passed to the browser in the page, so one
build runs anywhere. By default browsers use the web origin and the proxy; to publish
the API on its own origin instead (e.g. behind a CDN), set
`FLUORITE_PUBLIC_API_URL=https://api.example` and `FLUORITE_PROXY=0`. The CSP follows
that value automatically.

### S3 storage

The API can read the heavy roots from an S3-compatible bucket instead of the local
disk: Cloudflare R2, Backblaze B2, DigitalOcean Spaces, or a bucket of your own
(SeaweedFS, Garage, MinIO) on a spare machine.
It is off by default — with no `S3_*` variable set, every root is read from
`<output>` exactly as before.

[docs/s3-providers.md](docs/s3-providers.md) compares the services for this
project's data (68 GB in 167 k objects), has a recipe per provider, and has the
routes that need neither a card nor a budget — the bucket on your own hardware,
reached through a Cloudflare Tunnel;
[docs/oracle-cloud-s3.md](docs/oracle-cloud-s3.md) is the free-tier route with
MinIO on an Oracle A1.

The names are the ones the storage service uses for itself, so the values from a
dashboard can be pasted in as they are. `backend/.env.example` lists them all.

| Variable | Default | Meaning |
|---|---|---|
| `S3_TYPE` | from the endpoint | The service: `cloudflare`, `b2`, `wasabi`, `digitalocean`, `aws`, `minio`, `seaweedfs`, `s3` |
| `S3_ENDPOINT` | unset | Endpoint, e.g. `https://<account>.r2.cloudflarestorage.com` |
| `S3_BUCKET` | unset | Bucket name |
| `S3_ACCESS_KEY_ID` | unset | Access key |
| `S3_SECRET_ACCESS_KEY` | unset | Secret key |
| `S3_REGION` | from the provider | Signing region (`auto` for R2, `us-east-1` elsewhere) |
| `S3_SESSION_TOKEN` | unset | Token of temporary (STS) credentials |
| `S3_PREFIX` | `fluorite` | Key prefix inside the bucket |
| `S3_ROOTS` | `textures,meshes,models` | Roots served from the bucket |
| `S3_MODE` | `proxy` | `proxy` streams the object, `redirect` answers `302` |
| `S3_PUBLIC_BASE_URL` | unset | Public base URL of the bucket; needed by redirect mode |
| `S3_THUMBS` | `1` | Serve pre-generated thumbnails from the bucket (`0` disables it) |
| `S3_PATH_STYLE` | `1` | `0` addresses the bucket as a host prefix instead of a path segment |
| `S3_CACHE_CONTROL` | unset | `Cache-Control` the objects are stored with |
| `S3_KEEP_LOCAL` | `1` | `0` removes the local copy of a file once it is in the bucket |
| `S3_UPLOAD_CONCURRENCY` | `16` | How many files the publisher uploads (and prunes) at once |

For Cloudflare R2 the dashboard names are read as well, so the values of the
account can be pasted in unchanged:

| Variable | Meaning |
|---|---|
| `CLOUDFLARE_ACCOUNT_ID` | The account id; it builds the endpoint when `CLOUDFLARE_S3_API` is not given |
| `CLOUDFLARE_S3_API` | The endpoint of the account |
| `CLOUDFLARE_API_ACCESS_KEY` | The R2 access key id |
| `CLOUDFLARE_API_SECRET_KEY` | The R2 secret access key |

The generic `S3_*` name wins when both are set, and the endpoint is guessed from
its host when `S3_TYPE` is not given.

The first four are required together: a half-filled configuration is a startup
error, not a silent fallback to the disk.

**The keys.** The bucket mirrors the output directory: the key of
`textures/ui/logo.png` is `fluorite/textures/ui/logo.png` (below `S3_PREFIX`), so
the sync is a plain directory mirror and the API needs no mapping table.
`gamedata` stays on the disk: it is 1.5 M tiny JSON files, and a bucket of that
many objects is slow to list and expensive to keep.

**The two modes.** `proxy` (the default) asks the bucket for the object and streams
it back, so the bucket can stay private. `redirect` answers `302` with the object's
public URL, so the bytes never travel through the server; it needs
`S3_PUBLIC_BASE_URL` and a bucket (or a CDN in front of it) that serves
them.

**The fallback.** A read that fails — the bucket is empty, unreachable, not synced
yet — logs a warning and serves the local copy instead, so the site keeps working
while the bucket catches up.

**What still needs the disk.** The file explorer, the images and the search index
work from the bucket: the index is built from the listing of the bucket when a root
is in it, so the search finds the files it does not have on the disk. The
*catalog* (the operators, the weapons, the enemies) still looks at the local
`textures`, `meshes` and `models` roots: it checks which mesh folder and which
icon file exists, which is a local lookup. Keep those three roots on the disk
until the catalog reads the bucket too.

**The publisher.** The unpacker fills the bucket itself: `endfield update` writes
the new files to the disk and puts them in the bucket right away, so an update
reaches the site without a second tool. The same run can be repeated by hand:

```bash
cd backend
target/release/endfield publish --dry-run   # what would be uploaded
target/release/endfield publish             # upload it
target/release/endfield publish --remove-local   # and drop the local copy
```

The settings come from `backend/.env`, which the binary reads itself.

A file is uploaded when the bucket has no object for it, when the sizes differ, or
when the local file is newer than the object — so a run with nothing to do costs
one listing and no traffic. `--prune` deletes the objects that are gone locally
(only with a complete local tree), `--remove-local` deletes the local copy of
what was uploaded and of what the bucket already held, which is what keeps the
disk small. With `S3_KEEP_LOCAL=0`,
`endfield update` publishes and removes each chunk's files as soon as the chunk is
extracted, so the disk only needs room for one chunk's output.

**The bulk sync.** `backend/scripts/sync-s3.sh` mirrors the configured roots into the
bucket with `rclone`; it reads the same variables as the API and guesses the
provider from the endpoint (override with `S3_TYPE`). It is the tool for the
*first* fill of a bucket — tens of thousands of files at once — while the
publisher is the tool for everything after it.

```bash
cd backend
S3_ENDPOINT=https://<account>.r2.cloudflarestorage.com \
S3_BUCKET=assets \
S3_PREFIX=fluorite \
S3_ACCESS_KEY_ID=... S3_SECRET_ACCESS_KEY=... \
  scripts/sync-s3.sh --dry-run     # report what would change
  scripts/sync-s3.sh               # upload
  scripts/sync-s3.sh --delete      # also remove what is gone locally
```

By default the sync only adds and overwrites, so an interrupted extraction can
never delete the good copy in the bucket; `--delete` turns it into a full mirror.
The thumbnail cache is mirrored too, under `fluorite/thumbs/`, so the servers
share the thumbnails they generated instead of generating them again.

`scripts/auto-update.sh` publishes after an update that changed something (the
update state file is the signal, so an up-to-date check does not list the whole
bucket every fifteen minutes). `S3_SYNC_ALWAYS=1` forces it on every run,
and `S3_BULK_SYNC=1` adds the `rclone` mirror on top of it.

A complete walkthrough — Oracle Cloud (Always Free Ampere A1), MinIO, the tunnel,
the users and the policies — is in [docs/oracle-cloud-s3.md](docs/oracle-cloud-s3.md).

**Ports and caching.** `:3000` web (public, via the reverse proxy), `:8787` API
(loopback only unless the API is published separately). Hashed bundles under
`/assets/` are `immutable` for a year; HTML is not cached. API responses carry their own
`Cache-Control` and `ETag`s: files 1 hour, thumbnails 7 days, audio clips 1 day,
answering `If-None-Match` with 304 and `Range` with 206 (also through the proxy).

### Operations

- [ ] **Smoke test**: after an update or deploy, `scripts/smoke.sh` checks the running API and web routes end to end (`--api-only`, `--web-only`; override targets with `API=` and `WEB=`); it exits 1 on any failure.
- [ ] **Updates**: schedule `backend/scripts/auto-update.sh` every 15 minutes (cron or
  `fluorite-update.timer`). The servers keep running; the API re-indexes on its own.
- [ ] **Health**: schedule `backend/scripts/health-check.sh` daily with
  `FLUORITE_ALERT_WEBHOOK` set, and probe the site itself, e.g.
  `curl -fsS https://<site>/api/status` from your uptime monitor (checks web, proxy and
  API in one request).
- [ ] **Cache growth**: `FLUORITE_CACHE_DIR` holds thumbnails (no size cap) and
  transcoded audio clips (capped by `FLUORITE_AUDIO_CACHE_MB`, 5 GB by default, swept
  hourly). Watch it with `du -sh`; to reclaim space, stop the API, delete it, and start
  the API again (entries are regenerated on demand). Keep it off the output volume if
  that is near full.
- [ ] **Bucket publish**: with an S3 backend, `endfield update` publishes the new
  files into the bucket on its own. `backend/scripts/sync-s3.sh` is the bulk fill of
  a fresh bucket (`S3_BULK_SYNC=1` runs it after an update). Run
  `endfield publish --dry-run` by hand after a manual extraction, and check
  `backend/data/logs/sync-s3.log` when a file is missing from the bucket.
- [ ] **Schema regeneration**: `update` regenerates the MemoryPack schema when the
  client binary changes. If `health` warns about MemoryPack, run `endfield schema-gen`
  (see [above](#endfield-schema-gen)).
- [ ] **Deploying a new frontend**: `git pull && scripts/serve.sh` (or `bun run build`
  then `systemctl restart fluorite-web`). A new API binary needs
  `cargo build --release --bin fluorite-api` and a restart of `fluorite-api`.
- [ ] **Disk**: the output is ~80 GB and grows with each game version; keep headroom
  for `update` (see [Fixing an existing output](#fixing-an-existing-output)). With an
  S3 backend the heavy roots can be deleted from the disk once the bucket has them:
  the API falls back to the local copy, and re-extracts them if the bucket is
  unreachable.

## Project Structure

```
backend/
├── Cargo.toml
├── src/
│   ├── lib.rs                  # Library root (pub mod core, extraction)
│   ├── main.rs
│   ├── bin/
│   │   ├── endfield.rs         # Unified extraction CLI
│   │   ├── endfield-models.rs  # Rigged GLB exporter (scan store, ACL animations)
│   │   └── auth_test.rs        # Account/gacha/roster tool
│   ├── core/
│   │   ├── endfield/
│   │   │   ├── config.rs       # Game config resolution
│   │   │   └── constants.rs    # App codes, channels, endpoints
│   │   ├── hypergryph/
│   │   │   ├── account_service.rs  # OAuth2 login, token management
│   │   │   ├── binding.rs      # Account binding/roles
│   │   │   ├── launcher.rs     # Launcher API (versions, resources)
│   │   │   ├── launcher_web.rs # Launcher web endpoints
│   │   │   ├── u8.rs           # U8 platform (server list, channel tokens)
│   │   │   └── webview.rs      # Webview API (gacha, character data)
│   │   ├── roots.rs            # The read-only sub-trees of the output
│   │   └── s3/
│   │       ├── client.rs       # S3 client: head, get, list, put, delete
│   │       ├── provider.rs     # The services the API knows (R2, B2, MinIO, ...)
│   │       ├── publish.rs      # Publisher: local tree → bucket
│   │       └── settings.rs     # The S3_* variables
│   └── extraction/
│       ├── blc.rs              # BLC catalog parser + ChaCha20 VFS extraction
│       ├── chacha20.rs         # ChaCha20 (RFC7539) cipher
│       ├── crypto_constants.rs # Encryption keys and constants
│       ├── fair_guard_utils.rs # FairGuard anti-cheat obfuscation helpers
│       ├── gamedata.rs         # TextAsset/MonoBehaviour export by container path
│       ├── health.rs           # `health` checks over the update run log
│       ├── lua.rs              # Lua script decryption (base64 + XXTEA)
│       ├── lz4_inv.rs          # Custom LZ4 decompression
│       ├── memorypack.rs       # JsonData MemoryPack → JSON (schema-driven)
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
- **memorypack** decodes the `JsonData` group's MemoryPack files to JSON. Layouts come from compiled schemas: the ones in the runtime store (`data/memorypack-schema/`, see `schema-gen`), newest first, then the one embedded at build time (`backend/schema/memorypack/<version>.json.gz`). Strings are indexes into the group's `Json/StringPool/` pool. Each file is tried against its directory's known root types, then every plausible root, and only a decode that consumes every byte counts. Decoded files keep their raw bytes beside them as `.mpk`; files no type decodes are left as stored.
- **il2cpp**, **memorypack_schema** and **memorypack_store** generate those schemas: an IL2CPP reader for `GameAssembly.dll` + `global-metadata.dat`, the schema compiler and root learner, and the store plus the generate/validate/install flow behind `schema-gen`.
- **lz4_inv** is a custom LZ4 decompression variant used by some game assets.
- **fair_guard_utils** provides helpers for handling FairGuard anti-cheat obfuscated data.
- **utils** contains shared utilities: `format_bytes()`, `hex_dump()`, `sanitize_filename()`.

## Conclusion

fluorite.moe provides complete access to Arknights: Endfield game assets through reverse-engineered encryption and file format parsers. The `endfield` CLI handles the full pipeline from CDN download through decryption to structured output.
