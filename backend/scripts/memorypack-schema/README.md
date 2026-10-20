# MemoryPack schema for JsonData

Most `.json` files in the `JsonData` BLC group are not JSON. They are
[MemoryPack](https://github.com/Cysharp/MemoryPack) binaries written from generated
`Beyond.MemoryPack.*ForMemoryPack` wrapper types, with every string replaced by an index into
the global pool in `Json/StringPool/`. MemoryPack stores no field names, so decoding needs the
game's type definitions. These scripts derive them from the IL2CPP build and compile them into
`backend/schema/memorypack/<version>.json.gz`, which `src/extraction/memorypack.rs` embeds.

Regenerate the schema after a client update changes the data types (more files will fail to
decode). Everything here runs offline against files from the game package.

## Wire format (as observed in 1.6.4)

| Type | Encoding |
|---|---|
| object | member count byte (255 = null), then members in wrapper declaration order, base wrapper's members first |
| union (`[MemoryPackUnion]`) | tag byte (255 = null; 250 = a `u16` tag follows), then the case's object |
| `string` | `i32` pool index; -1 = null, -2 = empty |
| `StringPathHash` | `i64` path hash only |
| `T[]` | `i32` count (-1 = null), elements; a blittable `T` is copied raw |
| `List<T>`, `HashSet<T>`, ... | `i32` count, each element through its formatter (wrapped types as objects) |
| `Dictionary<K,V>` | `i32` count, then key/value pairs |
| `SerializeFieldDictionary<K,V>` | object header byte, then as `Dictionary` |
| blittable struct | raw bytes at the IL2CPP field offsets, IL2CPP instance size, when the member is `[MemoryPackAllowSerialize]` or the type has no wrapper |
| `Nullable<T>` (blittable `T`) | raw: `hasValue`, padding to `T`'s alignment, `T` |
| enum | its underlying integer |
| `AnimationCurve` | header byte, `postWrapMode`, `preWrapMode`, `i32` count, 28-byte keyframes |

A file's root type is not recorded. The decoder tries the types that decoded other files in the
same directory, then every type whose member count matches the first byte, and accepts the first
that consumes the file exactly.

## Regenerating

1. Download `GameAssembly.dll` and `Endfield_Data/il2cpp_data/Metadata/global-metadata.dat`
   from the game package (both are plain entries in the CDN ZIP).
2. Build [Il2CppDumper](https://github.com/Perfare/Il2CppDumper) at `4741d46` with
   `il2cppdumper.patch` applied (`dotnet build -c Release -p:TargetFrameworks=net10.0`). The patch
   adds the extra 4-byte field Endfield's 92-byte type definitions carry, and lets you pass the
   registration addresses, which its auto-search gets wrong for this binary.
3. Find the registrations: `python3 find_registration.py GameAssembly.dll global-metadata.dat`
   prints `MetadataRegistration` (16 qwords, with `fieldOffsetsCount == typeDefinitionsSizesCount`
   == the type definition count). `CodeRegistration` is the struct whose `codeGenModulesCount`
   equals the image count; Il2CppDumper's guess was two qwords early in 1.6.4.
4. Dump: `IL2CPP_CODEREG=<hex> IL2CPP_METAREG=<hex> dotnet Il2CppDumper.dll GameAssembly.dll
   global-metadata.dat dump/` (set `RequireAnyKey` to false in its `config.json`).
5. Build the type table and attach IL2CPP struct sizes:
   ```sh
   python3 parse_dump.py dump/dump.cs types.json
   python3 sizes.py GameAssembly.dll <MetadataRegistration hex> instance_sizes.json
   python3 -c "import json; s=json.load(open('instance_sizes.json')); t=json.load(open('types.json'))
   [v.__setitem__('size', s[v['idx']] - 16) for v in t.values() if v['kind'] == 'struct' and s[v['idx']] >= 16]
   json.dump(t, open('types.json', 'w'))"
   ```
6. Learn each directory's root types from an extracted `JsonData/Json` tree (writes `winners.json`
   next to the script): `python3 batch.py types.json <output>/decrypted/JsonData/Json`
7. Compile and install:
   ```sh
   python3 compile.py types.json winners.json <game version> schema.json
   gzip -9 -c schema.json > ../../schema/memorypack/<game version>.json.gz
   ```
   and point `memorypack.rs` at the new file.
8. Check: `python3 refdecode.py schema.json <Json dir> <out dir>` is the reference decoder; the
   Rust decoder's ignored oracle test compares against its output.

`mpdecode.py` is the schema-driven prototype the compiler is built on, and `pool.py` reads the
string pool.
