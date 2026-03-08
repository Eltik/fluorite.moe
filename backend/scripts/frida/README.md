# Phase P: Frida Runtime Hooks

## Prerequisites

1. Install [Frida](https://frida.re/) on your Windows machine:
   ```
   pip install frida-tools
   ```

2. Ensure you have the game files:
   - `GameAssembly.dll` (in the game directory)
   - `HGP.dll` (in the game directory)

## Usage

### 1. Start the game normally

### 2. Attach Frida and capture events

```powershell
# Find the game process
frida-ps | findstr Endfield

# Attach by PID
frida -p <PID> -l frida_hooks.js > capture.log 2>&1

# Or attach by name
frida -n "EndfieldClient" -l frida_hooks.js > capture.log 2>&1
```

### 3. Trigger encrypted content loading

Navigate in-game to areas that load encrypted assets (character models, etc.).
The hooks will automatically capture decrypt function calls and VFS loads.

### 4. Stop capture

Press `Ctrl+C` to detach Frida and save the capture log.

### 5. Analyze with companion tool

```bash
# From the backend directory
cargo run --bin frida-companion --release -- ../output/frida/capture.log
```

## What Gets Captured

| Event Type | Description |
|---|---|
| `DECRYPT_REGISTER` | When the decrypt function pointer is written to the cache |
| `DECRYPT_CALL` | Each call to the decrypt function with input/output buffers |
| `DECRYPT_CALL_EXTRA` | Additional register dumps for first few calls |
| `HGP_CALL` | Calls to HGP.dll exports (GetConst, NoteCache, etc.) |
| `VFS_LOAD` | VFS file path accesses (especially encrypted directories) |

## Output Format

Events are block-delimited for easy parsing:

```
=== FRIDA_EVENT type=DECRYPT_CALL ===
timestamp=1708000000000
func_addr=0x18XXXXXXXXX
arg0_ptr=0xNNNN
arg0_size=NNNN
input_hex=aabbccdd...
output_hex=eeff0011...
=== END_EVENT ===
```

## Hooks Overview

1. **Decrypt Registration Watcher**: Polls `[GameAssembly.dll + 0x0F60CD80]` for the cached decrypt function pointer. Also uses `MemoryAccessMonitor` if available.

2. **Decrypt Function Interceptor**: Once the decrypt function is found, hooks it to capture input/output buffers with Windows x64 calling convention (rcx, rdx, r8, r9).

3. **HGP.dll Export Hooks**: All 5 exports are intercepted with argument and return value logging.

4. **VFS Loading Monitor**: Hooks `CreateFileW` to catch file paths containing encrypted directory hashes. Also hooks `Unsafe.VFS.dll` exports if loaded.

## Troubleshooting

- **"GameAssembly.dll not found"**: The game hasn't loaded yet. Wait for the game to fully start.
- **No DECRYPT_CALL events**: The decrypt function may not be registered yet. Play through loading screens.
- **Access violations**: Some memory regions may be protected. The hooks handle this gracefully.
