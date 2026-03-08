/**
 * VFS Decrypt Interception — Gadget Mode (v17)
 *
 * KEY FIX (v15): DUAL-CONTEXT GUARD
 *   Frida gadget mode creates TWO separate V8 script contexts. The setInterval
 *   timer alternates between them. Context A ("rich") has all interceptor data
 *   (VFS handles, GA base, VFS_READ events). Context B ("empty") only has
 *   READFILE_DIAG data. v14 discovered this via DIAG_TIMER alternation.
 *
 *   FIX (v15): writeSummary() checks for meaningful data before writing.
 *   The empty context skips the write. handleVfsRead() triggers summary
 *   writes every 200 reads as insurance (always runs in the rich context).
 *
 * v16: FULL DECRYPTED BUFFER CAPTURE
 *   - Per-handle offset tracking for complete file reconstruction
 *   - Full buffer dumps on ANY mutation
 *   - Only postcheck ReadFile source (NtReadFile fires pre-decrypt)
 *   - handleReadMap in summary for Rust probe file reconstruction
 *
 * v17: CAPTURE ALL 4 ENCRYPTED DIRS
 *   - Postcheck decoupled from MAX_READ_DETAILED — uses lightweight 64-byte
 *     pre-snapshot for ALL ReadFile VFS reads, not just first 2000
 *   - Captures all 4 dirs (19E3AE45, 1CDDBF1F, 42A8FCA6, 775A31D1)
 *   - Stops dumping at MAX_FILE_DUMPS (2000)
 *
 * Hooks:
 *   Track A: xlua.dll + 0x35D0 disassembly + 0x588 callsite scan → hook callsites
 *   Track B: GA + 0x0F60CD80 cache pointer — fallback, skips if same as +0x35D0
 *   Track C: HGP.dll exports — independent polling
 *   Track D: CreateFileW filter — VFS file tracking
 *   Track E: Unsafe.VFS.dll probe — lazy load detection
 *   Track F: ReadFile + NtReadFile → return address clustering → dynamic loader discovery
 *
 * Outputs:
 *   vfs_decrypt_events.jsonl  — structured events for vfs_decrypt_probe.rs
 *   vfs_decrypt.log           — human-readable log
 *   vfs_decrypted/            — raw binary dumps of decrypted data
 *   vfs_decrypt_summary.json  — periodic summary (written from rich context only)
 */

"use strict";

// ══════════════════════════════════════════════════════════════
// Configuration
// ══════════════════════════════════════════════════════════════

var GA_MODULE = "GameAssembly.dll";
var XLUA_MODULE = "xlua.dll";
var XLUA_REGISTRATION_OFFSET = 0x35D0; // beyond_setDecryptionFunc

var DECRYPT_CACHE_RVA = 0x0F60CD80;

// The 4 encrypted VFS directory hashes
var ENCRYPTED_DIRS = ["19E3AE45", "1CDDBF1F", "42A8FCA6", "775A31D1"];
var ENCRYPTED_DIRS_LOWER = ENCRYPTED_DIRS.map(function (d) { return d.toLowerCase(); });

// HGP.dll export names
var HGP_EXPORTS = ["GetConst", "NoteCache", "ReverseStr", "Store", "TestFrom"];

// Limits
var MAX_RECON_CALLS = 50;       // Full arg dump for first N calls
var MAX_FILE_DUMPS = 2000;      // v16: raised from 200 — need full file capture
var MAX_DUMP_BYTES = 262144;    // v16: raised from 64KB to 256KB — capture full read chunks
var MAX_SNAPSHOT_BYTES = 256;   // Bytes for pre/post hex snapshot in events
var MAX_HGP_CALLS_PER = 20;    // Max logged calls per HGP export
var MAX_VFS_DLL_CALLS = 10;    // Max logged calls per Unsafe.VFS.dll export
var MAX_EVENTS = 50000;

// v5/v6: ReadFile + Loader discovery limits
var MAX_READ_EVENTS = 500;        // Max VFS_READ events to emit
var MAX_READ_DETAILED = 2000;     // v16: raised from 50 — postcheck more reads for full capture
var MAX_READ_LOG = 20;            // Log to console for first N reads
var LOADER_DISCOVERY_THRESHOLD = 50;  // v9: raised from 5 — need more samples for reliable clustering
var MAX_LOADER_CALLS = 100;       // Max VFS_LOADER_RETURN events
var MAX_LOADER_DUMPS = 200;       // Max decrypted data dumps from loader
var MAX_LOADER_RATE = 5000;       // v9: detach loader hook if exceeded in first second

// v6/v8: Buffered I/O interval
var EVENT_FLUSH_INTERVAL = 2000;      // Flush buffered events every 2s

// v7: ReadFile diagnostics + NtReadFile
var MAX_READ_DIAG = 200;             // v14: raised from 30 — need diagnostics AFTER VFS handles open

// Disassembly scan size for beyond_setDecryptionFunc
var DISASM_SCAN_SIZE = 200;

// Known plaintext signatures
var MAGIC_UNITYFS = [0x55, 0x6E, 0x69, 0x74, 0x79, 0x46, 0x53, 0x00]; // "UnityFS\0"
var MAGIC_SERIALIZED = [0x00, 0x00, 0x00, 0x14]; // SerializedFile header
var MAGIC_FLATBUF = [0x08, 0x00, 0x00, 0x00];    // FlatBuffer prefix (common)

// ══════════════════════════════════════════════════════════════
// Consolidated State — v13: ALL mutable state as top-level vars
// (fixes Frida gadget mode interceptor bug: nested S.X[key] writes
//  from Interceptor callbacks are silently lost, but top-level
//  var references persist correctly)
// ══════════════════════════════════════════════════════════════

var S_gaBase = null;
var S_gaSize = 0;
var S_xluaBase = null;
var S_decryptFuncAddr = null;
var S_decryptCallCount = 0;
var S_decryptSuccessCount = 0;
var S_fileDumpCount = 0;
var S_hgpCallCounts = {};
var S_vfsDllCallCounts = {};
var S_vfsFileHandles = {};
var S_recentVfsOpens = [];
var S_allEvents = [];
var S_eventCount = {};
var S_readFileHooked = false;
var S_readFileCallCount = 0;
var S_readReturnAddrs = {};
var S_loaderHooked = false;
var S_loaderFuncAddr = null;
var S_loaderCallCount = 0;
var S_loaderDecryptCount = 0;
var S_readDumpCount = 0;
var S_pendingEvents = [];
var S_flushCount = 0;
var S_gaDiscoveredViaCallsite = false;
var S_readFileDiagCount = 0;
var S_ntReadFileHooked = false;
var S_writeErrors = 0;
var S_loaderStartTime = 0;
var S_loaderDetached = false;
var S_xluaHooked = false;
var S_gaHooked = false;
var S_hgpHooked = false;
var S_decryptHooked = false;
var S_createFileWHooked = false;
var S_vfsDllProbed = false;
var S_hookedAddrs = {};
var S_xluaRegistrationAddr = null;
var S_postcheckResults = [];
var S_vfsReadSample = [];
var S_callsiteFunctions = [];
var S_loaderInfo = null;
// v16: Per-handle read offset tracking for file reconstruction
// { handle: { path, dirHash, totalRead, chunks: [{offset, size, dumpFile}] } }
var S_handleReadMap = {};
var S_handleDumpCount = 0;  // total chunks dumped via postcheck

// ══════════════════════════════════════════════════════════════
// Logging + Output (v9: open-write-close per flush — avoids null byte corruption)
// ══════════════════════════════════════════════════════════════

var S_pendingLogs = [];

// v7: Buffered log — safe from I/O thread callbacks
function log(msg) {
    S_pendingLogs.push("[vfs-decrypt] " + msg);
}

// v9: Direct log — open-write-close each time for boot messages
function logDirect(msg) {
    console.log("[vfs-decrypt] " + msg);
    try {
        var f = new File("vfs_decrypt.log", "a");
        f.write(msg + "\n");
        f.flush();
        f.close();
    } catch (e) {}
}

// v10: Flush pending logs — write each line individually (prevents silent truncation)
function flushPendingLogs() {
    if (S_pendingLogs.length === 0) return;
    var batch = S_pendingLogs;
    S_pendingLogs = [];
    for (var i = 0; i < batch.length; i++) {
        console.log(batch[i]);
    }
    try {
        var f = new File("vfs_decrypt.log", "a");
        for (var i = 0; i < batch.length; i++) {
            f.write(batch[i] + "\n");
        }
        f.flush();
        f.close();
    } catch (e) {}
}

// v11: Critical event types that are lost via the batched pipeline (threading issue)
var CRITICAL_EVENT_TYPES = {
    "VFS_READ": true,
    "VFS_READ_POSTCHECK": true,
    "VFS_LOADER_DETECTED": true,
    "CALLSITE_FUNC_HOOKED": true,
    "CALLSITE_SCAN": true,
    "HGP_LOADED": true,
};

// v11: Write event DIRECTLY to file — bypasses S_pendingEvents entirely.
// Called from the SAME thread as the hook callback, avoiding the race condition
// between game I/O threads and the V8 timer thread.
function writeEventDirect(jsonLine) {
    try {
        var f = new File("vfs_decrypt_events.jsonl", "a");
        f.write(jsonLine);
        f.flush();
        f.close();
        S_flushCount++;
    } catch (e) {
        S_writeErrors++;
    }
    // Also write to backup file for diagnostic comparison
    try {
        var f2 = new File("vfs_critical_events.jsonl", "a");
        f2.write(jsonLine);
        f2.flush();
        f2.close();
    } catch (e) {}
}

function emitEvent(data) {
    if (S_allEvents.length >= MAX_EVENTS) return;
    var evType = data.type || "UNKNOWN";
    S_eventCount[evType] = (S_eventCount[evType] || 0) + 1;
    data.timestamp = Date.now();
    S_allEvents.push(data);

    // v8: Pre-serialize to JSON string IMMEDIATELY
    var jsonLine;
    try {
        jsonLine = JSON.stringify(data) + "\n";
    } catch (e) {
        jsonLine = '{"type":"SERIALIZE_ERROR","originalType":"' + evType + '"}\n';
    }

    // v11: Critical events write DIRECTLY (bypass batched pipeline)
    if (CRITICAL_EVENT_TYPES[evType]) {
        writeEventDirect(jsonLine);
    } else {
        S_pendingEvents.push(jsonLine);
    }
}

// v10: Flush events — write each line individually to prevent silent truncation
function flushPendingEvents() {
    if (S_pendingEvents.length === 0) return;
    var batch = S_pendingEvents;
    S_pendingEvents = [];

    try {
        var f = new File("vfs_decrypt_events.jsonl", "a");
        for (var i = 0; i < batch.length; i++) {
            f.write(batch[i]);
        }
        f.flush();
        f.close();
        S_flushCount += batch.length;
    } catch (e) {
        S_writeErrors += batch.length;
    }
}

// ══════════════════════════════════════════════════════════════
// Safe hook helper — prevents double-hooking same address
// ══════════════════════════════════════════════════════════════

function safeAttach(addr, callbacks, label) {
    var key = addr.toString();
    if (S_hookedAddrs[key]) {
        log("  SKIP hook at " + key + " (" + label + ") — already hooked as " + S_hookedAddrs[key]);
        return false;
    }
    try {
        Interceptor.attach(addr, callbacks);
        S_hookedAddrs[key] = label;
        return true;
    } catch (e) {
        log("  FAILED to hook " + label + " at " + addr + ": " + e.message);
        return false;
    }
}

// ══════════════════════════════════════════════════════════════
// Module Discovery (gadget-mode safe)
// ══════════════════════════════════════════════════════════════

function findModuleByName(name) {
    var mods = Process.enumerateModules();
    for (var i = 0; i < mods.length; i++) {
        if (mods[i].name.toLowerCase() === name.toLowerCase()) return mods[i];
    }
    try {
        var base = Module.findBaseAddress(name);
        if (base) return { name: name, base: base };
    } catch (e) {}
    return null;
}

function findGAModule() {
    var ga = findModuleByName(GA_MODULE);
    if (ga) return ga;
    // Fallback: find module containing IL2CPP exports
    try {
        var probe = Module.findExportByName(null, "il2cpp_class_get_name");
        if (probe) {
            var mod = Process.findModuleByAddress(probe);
            if (mod) {
                log("  Found IL2CPP in module: " + mod.name + " @ " + mod.base);
                return mod;
            }
        }
    } catch (e) {}
    return null;
}

// Safe export enumeration for gadget mode
function safeEnumerateExports(mod) {
    // Try instance method first (works in gadget mode)
    if (mod && typeof mod.enumerateExports === "function") {
        try { return mod.enumerateExports(); } catch (e) {}
    }
    // Fallback: static method with module name
    try { return Module.enumerateExports(mod.name); } catch (e) {}
    return [];
}

// ══════════════════════════════════════════════════════════════
// Buffer Analysis Helpers
// ══════════════════════════════════════════════════════════════

function hexDump(addr, size) {
    if (!addr || addr.isNull()) return "";
    var n = Math.min(size, 512);
    try {
        var buf = addr.readByteArray(n);
        if (!buf) return "";
        var arr = new Uint8Array(buf);
        var parts = [];
        for (var i = 0; i < arr.length; i++) {
            parts.push(("0" + arr[i].toString(16)).slice(-2));
        }
        return parts.join("");
    } catch (e) { return "(read error: " + e.message + ")"; }
}

function computeEntropy(addr, size) {
    if (!addr || addr.isNull()) return -1;
    var n = Math.min(size, 256);
    try {
        var buf = addr.readByteArray(n);
        if (!buf) return -1;
        var arr = new Uint8Array(buf);
        var freq = new Array(256);
        for (var i = 0; i < 256; i++) freq[i] = 0;
        for (var j = 0; j < arr.length; j++) freq[arr[j]]++;
        var entropy = 0;
        for (var k = 0; k < 256; k++) {
            if (freq[k] === 0) continue;
            var p = freq[k] / arr.length;
            entropy -= p * Math.log(p) / Math.LN2;
        }
        return Math.round(entropy * 1000) / 1000;
    } catch (e) { return -1; }
}

function classifyArg(arg) {
    if (arg.isNull()) return "null";
    var val = arg.toInt32();
    // Heuristic: if value is in plausible pointer range (> 64KB, < high user addr)
    try {
        if (arg.compare(ptr(0x10000)) > 0 && arg.compare(ptr("0x7FFFFFFFFFFF")) < 0) {
            // Try to read — if it succeeds, it's a pointer
            arg.readU8();
            return "pointer";
        }
    } catch (e) {}
    return "int:" + val;
}

function classifyArgDetailed(arg) {
    // Returns { type, moduleName, moduleOffset, value }
    var result = { type: "unknown", moduleName: null, moduleOffset: null, value: arg.toString() };
    if (arg.isNull()) {
        result.type = "null";
        return result;
    }
    try {
        if (arg.compare(ptr(0x10000)) > 0 && arg.compare(ptr("0x7FFFFFFFFFFF")) < 0) {
            arg.readU8(); // test readability
            result.type = "pointer";
            // Check if it points into an executable module
            try {
                var mod = Process.findModuleByAddress(arg);
                if (mod) {
                    result.moduleName = mod.name;
                    result.moduleOffset = "0x" + arg.sub(mod.base).toString(16);
                    // Check if it's in an executable section (likely function pointer)
                    try {
                        var prot = Process.findRangeByAddress(arg);
                        if (prot && prot.protection.indexOf("x") !== -1) {
                            result.type = "function_pointer";
                        }
                    } catch (e2) {}
                }
            } catch (e) {}
            return result;
        }
    } catch (e) {}
    result.type = "int";
    result.value = arg.toInt32().toString();
    return result;
}

function matchesMagic(addr, magic) {
    try {
        for (var i = 0; i < magic.length; i++) {
            if (addr.add(i).readU8() !== magic[i]) return false;
        }
        return true;
    } catch (e) { return false; }
}

function detectFormat(addr) {
    if (!addr || addr.isNull()) return null;
    try {
        if (matchesMagic(addr, MAGIC_UNITYFS)) return "UnityFS";
        if (matchesMagic(addr, MAGIC_SERIALIZED)) return "SerializedFile";
        var first = addr.readU8();
        if (first === 0x7B) return "JSON";
        if (first === 0x5B) return "JSON_ARRAY";
        if (matchesMagic(addr, MAGIC_FLATBUF)) return "FlatBuffer";
        if (first === 0x50 && addr.add(1).readU8() === 0x4B) return "ZIP";
        if (first === 0x89 && addr.add(1).readU8() === 0x50) return "PNG";
    } catch (e) {}
    return null;
}

function writeDecryptedDump(addr, size, index) {
    if (S_fileDumpCount >= MAX_FILE_DUMPS) return null;
    var dumpSize = Math.min(size, MAX_DUMP_BYTES);
    if (dumpSize <= 0) return null;
    try {
        var data = addr.readByteArray(dumpSize);
        if (!data) return null;
        var filename = "vfs_decrypted/decrypt_" + ("0000" + index).slice(-4) + ".bin";
        var f = new File(filename, "wb");
        f.write(data);
        f.flush();
        f.close();
        S_fileDumpCount++;
        return filename;
    } catch (e) {
        log("  WARN: dump write failed: " + e.message);
        return null;
    }
}

// ══════════════════════════════════════════════════════════════
// Correlate decrypt with recent VFS file opens
// ══════════════════════════════════════════════════════════════

function findRecentVfsOpen(timeMs) {
    for (var i = S_recentVfsOpens.length - 1; i >= 0; i--) {
        var open = S_recentVfsOpens[i];
        var delta = timeMs - open.time;
        if (delta >= 0 && delta < 100) return open;
        if (delta > 100) break;
    }
    return null;
}

// ══════════════════════════════════════════════════════════════
// x64 Disassembly Engine — find stored callback pointer
// ══════════════════════════════════════════════════════════════

function readSigned32(addr) {
    // Read a 32-bit signed integer (little-endian)
    var val = addr.readU32();
    if (val >= 0x80000000) {
        return val - 0x100000000;
    }
    return val;
}

function disassembleRegistration(baseAddr) {
    // Read first DISASM_SCAN_SIZE bytes of beyond_setDecryptionFunc
    log("DISASM: Reading " + DISASM_SCAN_SIZE + " bytes at " + baseAddr);

    var rawHex = hexDump(baseAddr, DISASM_SCAN_SIZE);
    if (!rawHex || rawHex.indexOf("read error") !== -1) {
        log("  DISASM: Could not read memory at " + baseAddr);
        return [];
    }

    // Emit raw dump for offline analysis
    emitEvent({
        type: "DISASM_DUMP",
        address: baseAddr.toString(),
        size: DISASM_SCAN_SIZE,
        hex: rawHex,
    });

    log("  DISASM hex: " + rawHex.substring(0, 80) + "...");

    // Read into byte array for pattern scanning
    var buf = baseAddr.readByteArray(DISASM_SCAN_SIZE);
    var bytes = new Uint8Array(buf);

    // Scan for x64 MOV-to-global and JMP patterns
    // Each pattern: { opcode_desc, opcode_bytes, instruction_len, reg }
    var patterns = [
        { desc: "MOV [rip+disp32], RCX", prefix: [0x48, 0x89, 0x0D], len: 7, reg: "rcx" },
        { desc: "MOV [rip+disp32], RDX", prefix: [0x48, 0x89, 0x15], len: 7, reg: "rdx" },
        { desc: "MOV [rip+disp32], RAX", prefix: [0x48, 0x89, 0x05], len: 7, reg: "rax" },
        { desc: "MOV [rip+disp32], R8",  prefix: [0x4C, 0x89, 0x05], len: 7, reg: "r8" },
        { desc: "MOV [rip+disp32], RBX", prefix: [0x48, 0x89, 0x1D], len: 7, reg: "rbx" },
        { desc: "MOV [rip+disp32], RSI", prefix: [0x48, 0x89, 0x35], len: 7, reg: "rsi" },
        { desc: "MOV [rip+disp32], RDI", prefix: [0x48, 0x89, 0x3D], len: 7, reg: "rdi" },
        { desc: "JMP [rip+disp32]",      prefix: [0xFF, 0x25],       len: 6, reg: "jmp_indirect" },
        { desc: "MOV RCX, [rip+disp32]", prefix: [0x48, 0x8B, 0x0D], len: 7, reg: "load_rcx" },
        { desc: "MOV RAX, [rip+disp32]", prefix: [0x48, 0x8B, 0x05], len: 7, reg: "load_rax" },
        { desc: "LEA RCX, [rip+disp32]", prefix: [0x48, 0x8D, 0x0D], len: 7, reg: "lea_rcx" },
        { desc: "LEA RAX, [rip+disp32]", prefix: [0x48, 0x8D, 0x05], len: 7, reg: "lea_rax" },
        { desc: "CALL [rip+disp32]",     prefix: [0xFF, 0x15],       len: 6, reg: "call_indirect" },
    ];

    var discoveries = [];

    for (var offset = 0; offset < bytes.length - 7; offset++) {
        for (var p = 0; p < patterns.length; p++) {
            var pat = patterns[p];
            if (offset + pat.len > bytes.length) continue;

            var match = true;
            for (var b = 0; b < pat.prefix.length; b++) {
                if (bytes[offset + b] !== pat.prefix[b]) {
                    match = false;
                    break;
                }
            }
            if (!match) continue;

            // Found a pattern — extract disp32
            var dispOffset = offset + pat.prefix.length;
            var instrAddr = baseAddr.add(offset);
            var disp32 = readSigned32(baseAddr.add(dispOffset));

            // RIP-relative: target = instruction_addr + instruction_len + disp32
            var targetAddr = instrAddr.add(pat.len).add(disp32);

            log("  DISASM [+" + ("0000" + offset.toString(16)).slice(-4) + "] " +
                pat.desc + " → disp32=0x" + (disp32 >= 0 ? "" : "-") +
                Math.abs(disp32).toString(16) + " → target=" + targetAddr);

            var storedValue = null;
            var storedValueStr = null;
            var isExecutable = false;
            var targetModule = null;
            var targetModuleOffset = null;

            try {
                storedValue = targetAddr.readPointer();
                storedValueStr = storedValue.toString();

                if (!storedValue.isNull()) {
                    // Check if the stored pointer points to executable memory
                    try {
                        var range = Process.findRangeByAddress(storedValue);
                        if (range && range.protection.indexOf("x") !== -1) {
                            isExecutable = true;
                        }
                    } catch (e2) {}

                    // Check which module it's in
                    try {
                        var containingMod = Process.findModuleByAddress(storedValue);
                        if (containingMod) {
                            targetModule = containingMod.name;
                            targetModuleOffset = "0x" + storedValue.sub(containingMod.base).toString(16);
                        }
                    } catch (e3) {}

                    log("    stored=" + storedValueStr +
                        (targetModule ? " (" + targetModule + " @ " + targetModuleOffset + ")" : "") +
                        (isExecutable ? " [EXECUTABLE]" : ""));
                } else {
                    log("    stored=NULL");
                }
            } catch (e) {
                log("    target not readable: " + e.message);
            }

            discoveries.push({
                offset: offset,
                instruction: pat.desc,
                register: pat.reg,
                disp32: disp32,
                instrAddr: instrAddr.toString(),
                targetAddr: targetAddr.toString(),
                storedValue: storedValueStr,
                isExecutable: isExecutable,
                targetModule: targetModule,
                targetModuleOffset: targetModuleOffset,
            });
        }
    }

    // Emit all discoveries
    emitEvent({
        type: "DISASM_RESULT",
        address: baseAddr.toString(),
        discoveryCount: discoveries.length,
        discoveries: discoveries,
    });

    return discoveries;
}

// ══════════════════════════════════════════════════════════════
// v4: Callsite Scanner — find instructions that access [reg+0x588]
// ══════════════════════════════════════════════════════════════

function scanFor0x588Callsites(mod) {
    var moduleName = mod.name;
    var moduleBase = mod.base;
    var moduleSize = mod.size || 0;

    log("CALLSITE SCAN: Scanning " + moduleName + " (" + moduleSize + " bytes) for 0x588 offset");

    // Find executable ranges within this module
    var ranges = [];
    if (moduleSize > 0) {
        var allRanges = Process.enumerateRanges("r-x");
        for (var i = 0; i < allRanges.length; i++) {
            var r = allRanges[i];
            var rStart = r.base;
            var rEnd = r.base.add(r.size);
            var mStart = moduleBase;
            var mEnd = moduleBase.add(moduleSize);
            // Check overlap
            if (rStart.compare(mEnd) < 0 && rEnd.compare(mStart) > 0) {
                // Clamp to module bounds
                var start = rStart.compare(mStart) > 0 ? rStart : mStart;
                var end = rEnd.compare(mEnd) < 0 ? rEnd : mEnd;
                ranges.push({ base: start, size: end.sub(start).toInt32() });
            }
        }
    }

    if (ranges.length === 0) {
        log("  No executable ranges found in " + moduleName);
        return [];
    }

    var totalBytes = 0;
    for (var ri = 0; ri < ranges.length; ri++) totalBytes += ranges[ri].size;
    log("  " + ranges.length + " executable ranges, " + totalBytes + " bytes total");

    // Scan for the byte pattern "88 05 00 00" — the LE encoding of 0x588
    // This appears in instructions like MOV reg,[reg+0x588], CMP [reg+0x588],X etc.
    var matches = [];
    for (var si = 0; si < ranges.length; si++) {
        var scanBase = ranges[si].base;
        var scanSize = ranges[si].size;
        try {
            var hits = Memory.scanSync(scanBase, scanSize, "88 05 00 00");
            for (var hi = 0; hi < hits.length; hi++) {
                matches.push(hits[hi]);
            }
        } catch (e) {
            log("  Scan error in range " + scanBase + ": " + e.message);
        }
    }

    log("  Found " + matches.length + " raw pattern matches for 0x588");

    // Classify each match by reading the preceding bytes
    var classified = [];
    for (var mi = 0; mi < matches.length; mi++) {
        var matchAddr = matches[mi].address;
        // The 0x588 displacement starts at matchAddr
        // The instruction prefix is 2 bytes before (modrm at M-1, opcode at M-2)
        // For REX.W prefix instructions, REX is at M-3
        try {
            var modrm = matchAddr.sub(1).readU8();
            var opcode = matchAddr.sub(2).readU8();
            var rex = matchAddr.sub(3).readU8();

            // Validate modrm: mod=10 (bits 7:6 = 0b10) for disp32
            var mod_bits = (modrm >> 6) & 3;
            var rm_bits = modrm & 7;
            if (mod_bits !== 2) continue; // Must be mod=10 (disp32)
            if (rm_bits === 4) continue;  // SIB byte present = different encoding

            var instrType = "UNKNOWN";
            var baseReg = ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi"][rm_bits];
            var reg_bits = (modrm >> 3) & 7;

            // REX.W prefix check (0x48-0x4F)
            var hasRex = (rex & 0xF0) === 0x40;
            // REX.B extends rm field
            if (hasRex && (rex & 1)) {
                baseReg = ["r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15"][rm_bits];
            }
            // REX.R extends reg field
            var dstReg = ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi"][reg_bits];
            if (hasRex && (rex & 4)) {
                dstReg = ["r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15"][reg_bits];
            }

            if (opcode === 0x8B) {
                instrType = "MOV_READ";  // MOV reg, [base+0x588]
            } else if (opcode === 0x89) {
                instrType = "MOV_WRITE"; // MOV [base+0x588], reg
            } else if (opcode === 0xFF) {
                // reg_bits: 2=CALL, 4=JMP
                if (reg_bits === 2) instrType = "CALL_INDIRECT";
                else if (reg_bits === 4) instrType = "JMP_INDIRECT";
                else instrType = "FF_OTHER";
            } else if (opcode === 0x83) {
                instrType = "CMP_IMM8"; // CMP [base+0x588], imm8
            } else if (opcode === 0x3B) {
                instrType = "CMP_REG";  // CMP reg, [base+0x588]
            } else if (opcode === 0x39) {
                instrType = "CMP_RM";   // CMP [base+0x588], reg
            } else if (opcode === 0x81) {
                instrType = "CMP_IMM32"; // CMP [base+0x588], imm32
            } else {
                instrType = "OP_" + ("0" + opcode.toString(16)).slice(-2).toUpperCase();
            }

            // Read context bytes around the match for the event
            var contextStart = matchAddr.sub(4);
            var contextHex = hexDump(contextStart, 16);

            var moduleOffset = matchAddr.sub(moduleBase);
            var instrOffset = matchAddr.sub(3); // approximate instruction start (with REX)

            var entry = {
                address: matchAddr.toString(),
                moduleOffset: "0x" + moduleOffset.toString(16),
                instrAddr: instrOffset.toString(),
                instrType: instrType,
                baseReg: baseReg,
                dstOrSrcReg: dstReg,
                opcode: "0x" + ("0" + opcode.toString(16)).slice(-2),
                modrm: "0x" + ("0" + modrm.toString(16)).slice(-2),
                rex: hasRex ? ("0x" + ("0" + rex.toString(16)).slice(-2)) : null,
                contextHex: contextHex,
            };

            classified.push(entry);

            var marker = "";
            if (instrType === "MOV_READ" || instrType === "CALL_INDIRECT" || instrType === "JMP_INDIRECT") {
                marker = " ** CALLSITE CANDIDATE **";
            }

            log("  [" + entry.moduleOffset + "] " + instrType +
                " " + dstReg + ", [" + baseReg + "+0x588]" +
                " ctx=" + contextHex + marker);
        } catch (e) {
            // Ignore unreadable matches
        }
    }

    // Emit event with all classified matches
    emitEvent({
        type: "CALLSITE_SCAN",
        module: moduleName,
        moduleBase: moduleBase.toString(),
        moduleSize: moduleSize,
        totalMatches: matches.length,
        classifiedCount: classified.length,
        matches: classified,
    });

    log("  Callsite scan complete: " + classified.length + " classified instructions");
    return classified;
}

function findFunctionBoundary(addr) {
    // Scan backward from addr looking for function start
    // MSVC uses CC CC CC CC (int3 padding) between functions
    var maxBackScan = 4096;
    for (var off = 1; off < maxBackScan; off++) {
        try {
            var checkAddr = addr.sub(off);
            // Look for 4+ consecutive CC bytes (int3 padding)
            var b0 = checkAddr.readU8();
            if (b0 === 0xCC) {
                var ccCount = 1;
                for (var ci = 1; ci < 16; ci++) {
                    if (checkAddr.sub(ci).readU8() === 0xCC) ccCount++;
                    else break;
                }
                if (ccCount >= 4) {
                    // Function starts after the CC padding
                    return checkAddr.add(1);
                }
            }
        } catch (e) {
            break; // Hit unreadable memory
        }
    }

    // Fallback: look for common function prologues
    for (var pOff = 1; pOff < maxBackScan; pOff++) {
        try {
            var pAddr = addr.sub(pOff);
            var b1 = pAddr.readU8();
            var b2 = pAddr.add(1).readU8();
            var b3 = pAddr.add(2).readU8();
            var b4 = pAddr.add(3).readU8();

            // 48 89 5C 24 = MOV [RSP+xx], RBX (common save)
            if (b1 === 0x48 && b2 === 0x89 && b3 === 0x5C && b4 === 0x24) {
                // Check byte before is CC or C3 (ret)
                var before = pAddr.sub(1).readU8();
                if (before === 0xCC || before === 0xC3) return pAddr;
            }

            // 48 83 EC xx = SUB RSP, imm8
            if (b1 === 0x48 && b2 === 0x83 && b3 === 0xEC) {
                var before2 = pAddr.sub(1).readU8();
                if (before2 === 0xCC || before2 === 0xC3) return pAddr;
            }

            // 55 48 8B EC = PUSH RBP; MOV RBP, RSP
            if (b1 === 0x55 && b2 === 0x48 && b3 === 0x8B && b4 === 0xEC) {
                return pAddr;
            }

            // 40 55 = PUSH RBP (with REX prefix)
            if (b1 === 0x40 && b2 === 0x55) {
                var before3 = pAddr.sub(1).readU8();
                if (before3 === 0xCC || before3 === 0xC3) return pAddr;
            }
        } catch (e) {
            break;
        }
    }

    return null;
}

function hookCallsiteFunctions(classified, moduleName) {
    // Find and hook functions that contain MOV_READ, CALL_INDIRECT, or JMP_INDIRECT of [reg+0x588]
    var callsiteCandidates = [];
    for (var i = 0; i < classified.length; i++) {
        var c = classified[i];
        if (c.instrType === "MOV_READ" || c.instrType === "CALL_INDIRECT" || c.instrType === "JMP_INDIRECT") {
            callsiteCandidates.push(c);
        }
    }

    if (callsiteCandidates.length === 0) {
        log("  No callsite candidates (MOV_READ/CALL/JMP of [reg+0x588]) found in " + moduleName);
        return;
    }

    log("  " + callsiteCandidates.length + " callsite candidates — finding function boundaries...");

    var hookedFuncs = {};
    var hookedCount = 0;

    for (var ci = 0; ci < callsiteCandidates.length; ci++) {
        var candidate = callsiteCandidates[ci];
        var instrAddr = ptr(candidate.instrAddr);

        var funcStart = findFunctionBoundary(instrAddr);
        if (!funcStart) {
            log("  Could not find function boundary for callsite at " + candidate.moduleOffset);
            continue;
        }

        var funcKey = funcStart.toString();
        if (hookedFuncs[funcKey]) {
            log("  Callsite at " + candidate.moduleOffset + " in already-hooked function " + funcKey);
            continue;
        }

        hookedFuncs[funcKey] = true;
        hookedCount++;

        var funcMod = null;
        var funcModOffset = null;
        try {
            funcMod = Process.findModuleByAddress(funcStart);
            if (funcMod) {
                funcModOffset = "0x" + funcStart.sub(funcMod.base).toString(16);
            }
        } catch (e) {}

        log("  HOOKING function at " + funcStart +
            (funcMod ? " (" + funcMod.name + " @ " + funcModOffset + ")" : "") +
            " for callsite " + candidate.instrType + " at " + candidate.moduleOffset);

        // v12: Store callsite function info in memory for summary
        S_callsiteFunctions.push({
            funcAddr: funcStart.toString(),
            moduleName: funcMod ? funcMod.name : moduleName,
            moduleOffset: funcModOffset,
            callsiteOffset: candidate.moduleOffset,
            callsiteType: candidate.instrType,
        });

        emitEvent({
            type: "CALLSITE_FUNC_HOOKED",
            funcAddr: funcStart.toString(),
            moduleName: funcMod ? funcMod.name : moduleName,
            moduleOffset: funcModOffset,
            callsiteOffset: candidate.moduleOffset,
            callsiteType: candidate.instrType,
            baseReg: candidate.baseReg,
        });

        // Hook with closure to capture candidate info
        (function (cand, fStart) {
            var ok = safeAttach(fStart, {
                onEnter: function (args) {
                    this._csArgs = [args[0], args[1], args[2], args[3]];
                    this._csEntry = Date.now();
                    this._csCandidate = cand;

                    // Try to find the decrypt callback pointer by following the chain:
                    // For each pointer arg, try arg[+0x18][+0x588]
                    this._resolvedCallback = null;
                    for (var ai = 0; ai < 4; ai++) {
                        try {
                            var argVal = this._csArgs[ai];
                            if (argVal.isNull()) continue;
                            if (argVal.compare(ptr(0x10000)) <= 0) continue;

                            // Try direct: arg+0x588
                            try {
                                var direct = argVal.add(0x588).readPointer();
                                if (!direct.isNull() && direct.compare(ptr(0x10000)) > 0) {
                                    var rng = Process.findRangeByAddress(direct);
                                    if (rng && rng.protection.indexOf("x") !== -1) {
                                        this._resolvedCallback = { ptr: direct, argIdx: ai, chain: "direct" };
                                        break;
                                    }
                                }
                            } catch (e2) {}

                            // Try indirect: arg+0x18 → ptr+0x588
                            try {
                                var member = argVal.add(0x18).readPointer();
                                if (!member.isNull() && member.compare(ptr(0x10000)) > 0) {
                                    var callback = member.add(0x588).readPointer();
                                    if (!callback.isNull() && callback.compare(ptr(0x10000)) > 0) {
                                        var rng2 = Process.findRangeByAddress(callback);
                                        if (rng2 && rng2.protection.indexOf("x") !== -1) {
                                            this._resolvedCallback = { ptr: callback, argIdx: ai, chain: "indirect_0x18" };
                                            break;
                                        }
                                    }
                                }
                            } catch (e3) {}
                        } catch (e) {}
                    }

                    if (this._resolvedCallback && !S_decryptHooked) {
                        var cb = this._resolvedCallback;
                        var cbMod = null;
                        var cbModOffset = null;
                        try {
                            cbMod = Process.findModuleByAddress(cb.ptr);
                            if (cbMod) cbModOffset = "0x" + cb.ptr.sub(cbMod.base).toString(16);
                        } catch (e) {}

                        // v7: If this module looks like GA and we haven't found GA yet, capture it
                        if (cbMod && !S_gaBase &&
                            (cbMod.name.toLowerCase().indexOf("gameassembly") !== -1 ||
                             cbMod.name.toLowerCase().indexOf("il2cpp") !== -1)) {
                            S_gaBase = cbMod.base;
                            S_gaSize = cbMod.size || 0;
                            S_gaDiscoveredViaCallsite = true;
                            log("v9: GA discovered via callsite resolver: " + cbMod.name + " @ " + cbMod.base + " size=" + S_gaSize);
                        }

                        log("CALLSITE: Resolved decrypt callback via " + cb.chain +
                            " arg" + cb.argIdx + " → " + cb.ptr +
                            (cbMod ? " (" + cbMod.name + " @ " + cbModOffset + ")" : ""));

                        emitEvent({
                            type: "DECRYPT_FUNC_RESOLVED",
                            funcAddr: cb.ptr.toString(),
                            moduleName: cbMod ? cbMod.name : null,
                            moduleBase: cbMod ? cbMod.base.toString() : null,
                            moduleOffset: cbModOffset,
                            source: "callsite_scan",
                            callsiteFunc: fStart.toString(),
                            callsiteOffset: cand.moduleOffset,
                            resolveChain: cb.chain,
                            resolveArgIdx: cb.argIdx,
                            pollCount: 0,
                        });

                        S_decryptHooked = true;
                        S_decryptFuncAddr = cb.ptr;
                        hookDecryptFunction(cb.ptr);
                    }
                },
                onLeave: function (retval) {
                    // Log first few calls for analysis
                    if (S_eventCount["CALLSITE_INVOCATION"] === undefined ||
                        S_eventCount["CALLSITE_INVOCATION"] < 20) {
                        var argVals = [];
                        var argTypes = [];
                        for (var ai = 0; ai < 4; ai++) {
                            argVals.push(this._csArgs[ai].toString());
                            argTypes.push(classifyArg(this._csArgs[ai]));
                        }
                        emitEvent({
                            type: "CALLSITE_INVOCATION",
                            funcAddr: fStart.toString(),
                            callsiteOffset: cand.moduleOffset,
                            callsiteType: cand.instrType,
                            args: argVals,
                            argTypes: argTypes,
                            retval: retval.toString(),
                            retType: classifyArg(retval),
                            resolvedCallback: this._resolvedCallback ? this._resolvedCallback.ptr.toString() : null,
                            elapsedMs: Date.now() - this._csEntry,
                        });
                    }
                },
            }, "callsite_" + cand.moduleOffset);
        })(candidate, funcStart);
    }

    log("  Hooked " + hookedCount + " callsite functions in " + moduleName);
}

// ══════════════════════════════════════════════════════════════
// Track A: xlua.dll — Disassembly + Callsite scan approach
// ══════════════════════════════════════════════════════════════

function pollForXlua() {
    if (S_xluaHooked) return;

    var xluaMod = findModuleByName(XLUA_MODULE);
    if (!xluaMod) {
        setTimeout(pollForXlua, 200);
        return;
    }

    S_xluaHooked = true;
    S_xluaBase = xluaMod.base;
    S_xluaRegistrationAddr = S_xluaBase.add(XLUA_REGISTRATION_OFFSET);
    log("TRACK A: xlua.dll found @ " + S_xluaBase);

    // Enumerate all exports for context
    enumerateXluaExports(xluaMod);

    // Step 1: Disassemble beyond_setDecryptionFunc to find stored callback
    log("  Step 1: Disassembling beyond_setDecryptionFunc @ " + S_xluaRegistrationAddr);
    var discoveries = disassembleRegistration(S_xluaRegistrationAddr);

    // Step 2: Look for function pointer among discoveries
    var callbackFound = false;
    // Prioritize: MOV stores of arg registers (rcx, rdx) first, then others
    var priority = ["rcx", "rdx", "rax", "r8"];
    for (var pri = 0; pri < priority.length && !callbackFound; pri++) {
        for (var i = 0; i < discoveries.length && !callbackFound; i++) {
            var d = discoveries[i];
            if (d.register !== priority[pri]) continue;
            if (!d.storedValue || d.storedValue === "0x0") continue;
            if (!d.isExecutable) continue;

            // This stored value is an executable function pointer!
            var funcAddr = ptr(d.storedValue);
            log("TRACK A: DECRYPT CALLBACK FOUND via disassembly!");
            log("  Instruction: " + d.instruction + " at +" + d.offset.toString(16));
            log("  Global addr: " + d.targetAddr + " → stored callback: " + funcAddr);
            log("  In module: " + (d.targetModule || "?") + " @ " + (d.targetModuleOffset || "?"));

            emitEvent({
                type: "DECRYPT_FUNC_RESOLVED",
                funcAddr: funcAddr.toString(),
                moduleName: d.targetModule,
                moduleBase: d.targetModule ? (findModuleByName(d.targetModule) || {}).base : null,
                moduleOffset: d.targetModuleOffset,
                source: "disassembly",
                disasmInstruction: d.instruction,
                disasmOffset: d.offset,
                globalAddr: d.targetAddr,
                cacheAddr: null,
                pollCount: 0,
            });

            S_decryptHooked = true;
            S_decryptFuncAddr = funcAddr;
            hookDecryptFunction(funcAddr);
            callbackFound = true;
        }
    }

    // If no executable pointer found, check all non-null stored values
    // (might be in JIT memory without x flag, or not yet populated)
    if (!callbackFound) {
        log("  No executable stored callback found in disassembly");
        log("  Checking all non-null stored values...");

        for (var j = 0; j < discoveries.length; j++) {
            var d2 = discoveries[j];
            if (!d2.storedValue || d2.storedValue === "0x0") continue;
            if (d2.register === "jmp_indirect" || d2.register === "call_indirect") continue;
            // Store instructions only
            if (d2.register.indexOf("load_") === 0 || d2.register.indexOf("lea_") === 0) continue;

            var candidate = ptr(d2.storedValue);
            log("  CANDIDATE: " + d2.instruction + " → " + candidate +
                (d2.targetModule ? " (" + d2.targetModule + ")" : " (unknown module)"));
        }

        log("  Will rely on callsite scan, Track B (GA cache), and registration hook as fallbacks");
    }

    // Step 2b: v4 callsite scan — scan xlua.dll for [reg+0x588] access patterns
    if (!S_decryptHooked) {
        log("  Step 2b: Scanning xlua.dll for 0x588 callsites...");
        var xluaCallsites = scanFor0x588Callsites(xluaMod);
        if (xluaCallsites.length > 0) {
            hookCallsiteFunctions(xluaCallsites, XLUA_MODULE);
        }
    }

    // Step 3: Install registration hook (catches future callback changes)
    // But only if the address isn't already hooked by something else
    log("  Step 3: Installing registration hook at " + S_xluaRegistrationAddr);
    hookRegistrationFunc(S_xluaRegistrationAddr);
}

function enumerateXluaExports(xluaMod) {
    try {
        var exports = safeEnumerateExports(xluaMod);
        log("  xlua.dll exports: " + exports.length + " total");

        var exportList = [];
        for (var i = 0; i < exports.length; i++) {
            var exp = exports[i];
            var offset = "0x" + ptr(exp.address).sub(S_xluaBase).toString(16);
            exportList.push({ name: exp.name, offset: offset, type: exp.type });
            // Log first 20 and any near our target offset
            if (i < 20 || offset === "0x35d0" || offset === "0x35D0") {
                log("    [" + offset + "] " + exp.name + " (" + exp.type + ")");
            }
        }

        emitEvent({
            type: "XLUA_EXPORTS",
            moduleBase: S_xluaBase.toString(),
            exportCount: exports.length,
            exports: exportList,
        });
    } catch (e) {
        log("  WARN: Could not enumerate xlua.dll exports: " + e.message);
    }
}

function hookRegistrationFunc(addr) {
    var ok = safeAttach(addr, {
        onEnter: function (args) {
            this._regArgs = [args[0], args[1]];
            this._regArgDetails = [];

            for (var i = 0; i < 2; i++) {
                this._regArgDetails.push(classifyArgDetailed(this._regArgs[i]));
            }

            log("REGISTRATION CALL: beyond_setDecryptionFunc(" +
                this._regArgs[0] + ", " + this._regArgs[1] + ")");
            log("  arg0: " + JSON.stringify(this._regArgDetails[0]));
            log("  arg1: " + JSON.stringify(this._regArgDetails[1]));
        },
        onLeave: function (retval) {
            var retDetail = classifyArgDetailed(retval);

            log("  retval: " + retval + " → " + JSON.stringify(retDetail));

            emitEvent({
                type: "REGISTRATION_CALL",
                args: [this._regArgs[0].toString(), this._regArgs[1].toString()],
                argDetails: this._regArgDetails,
                retval: retval.toString(),
                retDetail: retDetail,
            });

            // If any arg or retval is a function pointer, hook it as the decrypt function
            var candidates = this._regArgDetails.concat([retDetail]);
            var candidateAddrs = [this._regArgs[0], this._regArgs[1], retval];

            for (var i = 0; i < candidates.length; i++) {
                var detail = candidates[i];
                if (detail.type === "function_pointer" && !S_decryptHooked) {
                    var funcAddr = candidateAddrs[i];
                    log("FOUND DECRYPT CALLBACK via registration: arg" + i + " = " + funcAddr +
                        " (in " + detail.moduleName + " @ " + detail.moduleOffset + ")");

                    emitEvent({
                        type: "DECRYPT_FUNC_RESOLVED",
                        funcAddr: funcAddr.toString(),
                        moduleName: detail.moduleName,
                        moduleBase: detail.moduleName ? (findModuleByName(detail.moduleName) || {}).base : null,
                        moduleOffset: detail.moduleOffset,
                        source: "registration_arg" + i,
                        cacheAddr: null,
                        pollCount: 0,
                    });

                    S_decryptHooked = true;
                    S_decryptFuncAddr = funcAddr;
                    hookDecryptFunction(funcAddr);
                    break;
                }
            }

            // If no function pointer found among args, log candidates
            if (!S_decryptHooked) {
                log("  WARNING: No function pointer identified in registration args");
                for (var j = 0; j < this._regArgDetails.length; j++) {
                    var d = this._regArgDetails[j];
                    if (d.type === "pointer" && d.moduleName) {
                        log("  CANDIDATE: arg" + j + " @ " + this._regArgs[j] +
                            " in " + d.moduleName + " offset " + d.moduleOffset);
                    }
                }
            }
        },
    }, "registration_beyond_setDecryptionFunc");

    if (ok) {
        log("  Registration hook installed at " + addr);
    }
}

// ══════════════════════════════════════════════════════════════
// Track B: GA cache pointer — Fallback decrypt discovery
// ══════════════════════════════════════════════════════════════

function pollForGA() {
    if (S_gaHooked) return;

    var ga = findGAModule();

    // v7: If callsite resolver already found GA, use that
    if (!ga && S_gaBase) {
        ga = { name: "GameAssembly.dll", base: S_gaBase };
        log("TRACK B: Using GA base from callsite resolver: " + S_gaBase);
    }

    if (!ga) {
        setTimeout(pollForGA, 200);
        return;
    }

    S_gaHooked = true;
    S_gaBase = ga.base;
    S_gaSize = ga.size || 0;
    log("TRACK B: GameAssembly.dll found @ " + S_gaBase + " size=" + S_gaSize);

    // v8: Removed pre-hook at KNOWN_LOADER_RVA (0x0A0F4ED0) — v7 proved 0 calls.
    // Dynamic discovery via return-address clustering now runs unimpeded.

    // Install hooks that need GA to be loaded
    if (!S_createFileWHooked) {
        S_createFileWHooked = true;
        installCreateFileWHook();
    }

    probeUnsafeVfsDll();

    // v4: Scan GA executable sections for 0x588 callsites (delayed — large module)
    if (!S_decryptHooked) {
        setTimeout(function () {
            if (S_decryptHooked) return;
            log("TRACK B: Scanning GA for 0x588 callsites (fallback)...");
            var gaMod = findGAModule();
            if (gaMod && gaMod.size) {
                var gaCallsites = scanFor0x588Callsites(gaMod);
                if (gaCallsites.length > 0 && !S_decryptHooked) {
                    hookCallsiteFunctions(gaCallsites, GA_MODULE);
                }
            }
        }, 10000); // Delay 10s to let xlua scan finish first
    }

    // Poll the cache pointer for the decrypt function
    var cachePtr = S_gaBase.add(DECRYPT_CACHE_RVA);
    log("  Polling decrypt cache @ " + cachePtr);

    var pollCount = 0;
    var timer = setInterval(function () {
        pollCount++;

        // If Track A already found the decrypt function, stop polling
        if (S_decryptHooked) {
            clearInterval(timer);
            log("  Track B: decrypt already hooked by Track A, stopping cache poll");
            return;
        }

        try {
            var funcPtr = cachePtr.readPointer();
            if (!funcPtr.isNull()) {
                clearInterval(timer);

                // Check if this resolves to xlua.dll+0x35D0 (the registration function itself)
                if (S_xluaRegistrationAddr && funcPtr.equals(S_xluaRegistrationAddr)) {
                    log("TRACK B: cache points to xlua.dll+0x35D0 (registration function, NOT the callback)");
                    log("  Deferring to Track A disassembly approach");
                    emitEvent({
                        type: "GA_CACHE_IS_REGISTRATION",
                        cacheAddr: cachePtr.toString(),
                        resolvedAddr: funcPtr.toString(),
                        xluaRegistrationAddr: S_xluaRegistrationAddr.toString(),
                    });
                    return;
                }

                S_decryptFuncAddr = funcPtr;

                var containingMod = null;
                try { containingMod = Process.findModuleByAddress(funcPtr); } catch (e) {}

                var modName = containingMod ? containingMod.name : "unknown";
                var modBase = containingMod ? containingMod.base.toString() : "unknown";

                log("TRACK B: DECRYPT FUNCTION RESOLVED @ " + funcPtr +
                    " (in " + modName + " base=" + modBase + ") after " + pollCount + " polls");

                emitEvent({
                    type: "DECRYPT_FUNC_RESOLVED",
                    funcAddr: funcPtr.toString(),
                    moduleName: modName,
                    moduleBase: modBase,
                    moduleOffset: containingMod ? "0x" + funcPtr.sub(containingMod.base).toString(16) : null,
                    source: "ga_cache",
                    cacheAddr: cachePtr.toString(),
                    pollCount: pollCount,
                });

                if (!S_decryptHooked) {
                    S_decryptHooked = true;
                    hookDecryptFunction(funcPtr);
                }
            }
        } catch (e) {
            // Memory not accessible yet
        }

        if (pollCount % 120 === 0) {
            log("  Track B: still polling for decrypt func (" + (pollCount * 200 / 1000) + "s elapsed)...");
        }
    }, 200);
}

// ══════════════════════════════════════════════════════════════
// Decrypt Function Hook (shared by Track A and Track B)
// ══════════════════════════════════════════════════════════════

function hookDecryptFunction(funcPtr) {
    log("Hooking decrypt function at " + funcPtr);

    var ok = safeAttach(funcPtr, {
        onEnter: function (args) {
            S_decryptCallCount++;
            this._callNum = S_decryptCallCount;
            this._entryTime = Date.now();

            // Windows x64 ABI: rcx, rdx, r8, r9
            this._args = [args[0], args[1], args[2], args[3]];
            this._argTypes = [];
            this._preHex = [];
            this._preEntropy = [];

            for (var i = 0; i < 4; i++) {
                var aType = classifyArg(this._args[i]);
                this._argTypes.push(aType);

                if (aType === "pointer") {
                    this._preHex.push(hexDump(this._args[i], MAX_SNAPSHOT_BYTES));
                    this._preEntropy.push(computeEntropy(this._args[i], MAX_SNAPSHOT_BYTES));
                } else {
                    this._preHex.push("");
                    this._preEntropy.push(-1);
                }
            }
        },
        onLeave: function (retval) {
            var postHex = [];
            var postEntropy = [];
            var entropyDrop = false;
            var detectedFormat = null;
            var matchedBufferIdx = -1;
            var bufferSize = 0;

            // Re-read pointer args to detect in-place decryption
            for (var i = 0; i < 4; i++) {
                if (this._argTypes[i] === "pointer") {
                    var ph = hexDump(this._args[i], MAX_SNAPSHOT_BYTES);
                    var pe = computeEntropy(this._args[i], MAX_SNAPSHOT_BYTES);
                    postHex.push(ph);
                    postEntropy.push(pe);

                    if (this._preEntropy[i] > 6.0 && pe >= 0 && pe < this._preEntropy[i] - 0.5) {
                        entropyDrop = true;
                        matchedBufferIdx = i;
                    }
                    var fmt = detectFormat(this._args[i]);
                    if (fmt) {
                        detectedFormat = fmt;
                        matchedBufferIdx = i;
                    }
                } else {
                    postHex.push("");
                    postEntropy.push(-1);
                }
            }

            // Also check return value as potential output pointer
            var retType = classifyArg(retval);
            var retHex = "";
            var retEntropy = -1;
            var retFormat = null;
            if (retType === "pointer") {
                retHex = hexDump(retval, MAX_SNAPSHOT_BYTES);
                retEntropy = computeEntropy(retval, MAX_SNAPSHOT_BYTES);
                retFormat = detectFormat(retval);
                if (retFormat && !detectedFormat) {
                    detectedFormat = retFormat;
                }
                if (retEntropy >= 0 && retEntropy < 6.0) {
                    for (var j = 0; j < 4; j++) {
                        if (this._preEntropy[j] > 6.0) {
                            entropyDrop = true;
                            break;
                        }
                    }
                }
            }

            // Determine likely buffer size from integer args
            for (var k = 0; k < 4; k++) {
                if (this._argTypes[k] !== "pointer" && this._argTypes[k] !== "null") {
                    var sizeVal = this._args[k].toInt32();
                    if (sizeVal > 0 && sizeVal < 0x10000000) {
                        bufferSize = sizeVal;
                        break;
                    }
                }
            }

            // Correlate with recent VFS file open
            var correlatedOpen = findRecentVfsOpen(this._entryTime);

            // Full dump for first N calls (signature discovery)
            if (this._callNum <= MAX_RECON_CALLS) {
                emitEvent({
                    type: "DECRYPT_CALL",
                    callNum: this._callNum,
                    args: [
                        this._args[0].toString(),
                        this._args[1].toString(),
                        this._args[2].toString(),
                        this._args[3].toString(),
                    ],
                    argTypes: this._argTypes,
                    preHex: this._preHex,
                    preEntropy: this._preEntropy,
                    postHex: postHex,
                    postEntropy: postEntropy,
                    retval: retval.toString(),
                    retType: retType,
                    retHex: retHex,
                    retEntropy: retEntropy,
                    retFormat: retFormat,
                    entropyDrop: entropyDrop,
                    detectedFormat: detectedFormat,
                    matchedBufferIdx: matchedBufferIdx,
                    bufferSize: bufferSize,
                    correlatedVfsOpen: correlatedOpen,
                    elapsedMs: Date.now() - this._entryTime,
                });
            }

            // Emit DECRYPT_SUCCESS on entropy drop or magic found
            if (entropyDrop || detectedFormat) {
                S_decryptSuccessCount++;

                var outputAddr = null;
                var outputSize = bufferSize > 0 ? bufferSize : MAX_DUMP_BYTES;
                if (matchedBufferIdx >= 0) {
                    outputAddr = this._args[matchedBufferIdx];
                } else if (retType === "pointer" && (retFormat || retEntropy < 6.0)) {
                    outputAddr = retval;
                }

                var dumpFile = null;
                if (outputAddr) {
                    dumpFile = writeDecryptedDump(outputAddr, outputSize, this._callNum);
                }

                if (this._callNum > MAX_RECON_CALLS) {
                    emitEvent({
                        type: "DECRYPT_SUCCESS",
                        callNum: this._callNum,
                        args: [
                            this._args[0].toString(),
                            this._args[1].toString(),
                            this._args[2].toString(),
                            this._args[3].toString(),
                        ],
                        argTypes: this._argTypes,
                        preEntropy: this._preEntropy,
                        postEntropy: postEntropy,
                        preHex: this._preHex,
                        postHex: postHex,
                        retval: retval.toString(),
                        retHex: retHex,
                        retEntropy: retEntropy,
                        entropyDrop: entropyDrop,
                        detectedFormat: detectedFormat,
                        matchedBufferIdx: matchedBufferIdx,
                        bufferSize: bufferSize,
                        dumpFile: dumpFile,
                        correlatedVfsOpen: correlatedOpen,
                    });
                }

                if (S_decryptSuccessCount <= 10) {
                    log("DECRYPT_SUCCESS #" + S_decryptSuccessCount +
                        " call=" + this._callNum +
                        " format=" + (detectedFormat || "entropy_drop") +
                        " size=" + bufferSize +
                        " dump=" + (dumpFile || "none"));
                }
            }

            // Periodic status
            if (S_decryptCallCount % 1000 === 0) {
                log("Decrypt calls: " + S_decryptCallCount +
                    " successes: " + S_decryptSuccessCount +
                    " dumps: " + S_fileDumpCount);
            }
        },
    }, "decrypt_function");

    if (ok) {
        log("Decrypt function hooked successfully");
    }
}

// ══════════════════════════════════════════════════════════════
// Track C: HGP.dll — Independent polling
// ══════════════════════════════════════════════════════════════

function pollForHGP() {
    if (S_hgpHooked) return;

    var hgpMod = findModuleByName("HGP.dll");

    // Fallback: try finding via export name
    if (!hgpMod) {
        try {
            var getConst = Module.findExportByName(null, "GetConst");
            if (getConst) {
                var mod = Process.findModuleByAddress(getConst);
                if (mod) {
                    hgpMod = mod;
                    log("  Found HGP via GetConst export in: " + mod.name);
                }
            }
        } catch (e) {}
    }

    if (!hgpMod) {
        setTimeout(pollForHGP, 2000);
        return;
    }

    S_hgpHooked = true;
    log("TRACK C: HGP.dll found @ " + hgpMod.base);

    emitEvent({
        type: "HGP_LOADED",
        moduleBase: hgpMod.base.toString(),
        moduleName: hgpMod.name,
    });

    for (var i = 0; i < HGP_EXPORTS.length; i++) {
        var exportName = HGP_EXPORTS[i];
        S_hgpCallCounts[exportName] = 0;
        try {
            var addr = Module.findExportByName(null, exportName);
            if (!addr) {
                log("  HGP export not found: " + exportName);
                continue;
            }
            hookHGPExport(exportName, addr);
        } catch (e) {
            log("  HGP hook failed for " + exportName + ": " + e.message);
        }
    }
}

function hookHGPExport(name, addr) {
    var ok = safeAttach(addr, {
        onEnter: function (args) {
            S_hgpCallCounts[name]++;
            this._hgpName = name;
            this._hgpCallNum = S_hgpCallCounts[name];
            this._hgpArgs = [args[0], args[1], args[2], args[3]];
        },
        onLeave: function (retval) {
            if (this._hgpCallNum <= MAX_HGP_CALLS_PER) {
                var argTypes = [];
                var argVals = [];
                for (var i = 0; i < 4; i++) {
                    argTypes.push(classifyArg(this._hgpArgs[i]));
                    argVals.push(this._hgpArgs[i].toString());
                }
                emitEvent({
                    type: "HGP_CALL",
                    export: this._hgpName,
                    callNum: this._hgpCallNum,
                    args: argVals,
                    argTypes: argTypes,
                    retval: retval.toString(),
                    retType: classifyArg(retval),
                });
                if (this._hgpCallNum <= 3) {
                    log("HGP." + this._hgpName + " #" + this._hgpCallNum +
                        " args=[" + argVals.join(", ") + "] ret=" + retval);
                }
            }
        },
    }, "hgp_" + name);

    if (ok) {
        log("  Hooked HGP." + name + " @ " + addr);
    }
}

// ══════════════════════════════════════════════════════════════
// Track D: CreateFileW Filter (gadget-mode safe)
// ══════════════════════════════════════════════════════════════

function installCreateFileWHook() {
    try {
        // Gadget-mode safe: use module instance method
        var createFileW = null;

        // Method 1: Find kernel32.dll module object, use getExportByName
        var mods = Process.enumerateModules();
        for (var mi = 0; mi < mods.length; mi++) {
            if (mods[mi].name.toLowerCase() === "kernel32.dll") {
                try {
                    createFileW = mods[mi].getExportByName("CreateFileW");
                    if (createFileW) {
                        log("  CreateFileW found via kernel32.dll module instance");
                        break;
                    }
                } catch (e) {}
            }
        }

        // Method 2: null module name (works in injection mode)
        if (!createFileW) {
            try {
                createFileW = Module.findExportByName(null, "CreateFileW");
                if (createFileW) log("  CreateFileW found via null module lookup");
            } catch (e) {}
        }

        // Method 3: explicit kernel32 string
        if (!createFileW) {
            try {
                createFileW = Module.findExportByName("kernel32.dll", "CreateFileW");
                if (createFileW) log("  CreateFileW found via explicit kernel32.dll string");
            } catch (e2) {}
        }

        // Method 4: ntdll NtCreateFile as lower-level fallback
        if (!createFileW) {
            log("CreateFileW not found — trying NtCreateFile in ntdll.dll");
            for (var ni = 0; ni < mods.length; ni++) {
                if (mods[ni].name.toLowerCase() === "ntdll.dll") {
                    try {
                        createFileW = mods[ni].getExportByName("NtCreateFile");
                        if (createFileW) {
                            log("  NtCreateFile found — note: different ABI than CreateFileW");
                            break;
                        }
                    } catch (e) {}
                }
            }
        }

        if (!createFileW) {
            log("CreateFileW/NtCreateFile not found at all");
            return;
        }

        var ok = safeAttach(createFileW, {
            onEnter: function (args) {
                this._skip = true;
                try {
                    var path = args[0].readUtf16String();
                    if (!path) return;
                    var pathLower = path.toLowerCase();

                    var matchedDir = null;
                    for (var i = 0; i < ENCRYPTED_DIRS_LOWER.length; i++) {
                        if (pathLower.indexOf(ENCRYPTED_DIRS_LOWER[i]) !== -1) {
                            matchedDir = ENCRYPTED_DIRS[i];
                            break;
                        }
                    }
                    if (!matchedDir && pathLower.indexOf("hgvfs") === -1) return;

                    this._skip = false;
                    this._path = path;
                    this._dirHash = matchedDir || "hgvfs";
                    this._openTime = Date.now();
                } catch (e) {}
            },
            onLeave: function (retval) {
                if (this._skip) return;
                var handle = retval.toInt32();
                if (handle === -1) return;

                var openInfo = {
                    path: this._path,
                    dirHash: this._dirHash,
                    time: this._openTime,
                    handle: handle,
                };

                S_vfsFileHandles[handle] = openInfo;
                S_recentVfsOpens.push(openInfo);
                if (S_recentVfsOpens.length > 20) {
                    S_recentVfsOpens.shift();
                }

                emitEvent({
                    type: "VFS_FILE_OPEN",
                    path: this._path,
                    dirHash: this._dirHash,
                    handle: handle,
                });

                log("VFS_FILE_OPEN: " + this._dirHash + " → " + this._path);
            },
        }, "CreateFileW");

        if (ok) {
            log("CreateFileW hook installed (filtering for encrypted dirs + hgvfs)");
        }
    } catch (e) {
        log("CreateFileW hook failed: " + e.message);
    }
}

// ══════════════════════════════════════════════════════════════
// Track F: ReadFile Interception + Loader Discovery (v5)
// ══════════════════════════════════════════════════════════════

function installReadFileHook() {
    if (S_readFileHooked) return;
    S_readFileHooked = true;

    var readFile = null;

    // Method 1: Find kernel32.dll module object, use getExportByName
    var mods = Process.enumerateModules();
    for (var mi = 0; mi < mods.length; mi++) {
        if (mods[mi].name.toLowerCase() === "kernel32.dll") {
            try {
                readFile = mods[mi].getExportByName("ReadFile");
                if (readFile) {
                    log("  ReadFile found via kernel32.dll module instance");
                    break;
                }
            } catch (e) {}
        }
    }

    // Method 2: null module name
    if (!readFile) {
        try {
            readFile = Module.findExportByName(null, "ReadFile");
            if (readFile) log("  ReadFile found via null module lookup");
        } catch (e) {}
    }

    // Method 3: explicit kernel32 string
    if (!readFile) {
        try {
            readFile = Module.findExportByName("kernel32.dll", "ReadFile");
            if (readFile) log("  ReadFile found via explicit kernel32.dll string");
        } catch (e) {}
    }

    if (!readFile) {
        log("ReadFile not found — Track F disabled");
        return;
    }

    var ok = safeAttach(readFile, {
        onEnter: function (args) {
            // v7: Unfiltered diagnostic — log first N ReadFile calls regardless of handle match
            S_readFileDiagCount++;
            if (S_readFileDiagCount <= MAX_READ_DIAG) {
                var diagHandle = args[0].toInt32();
                var diagSize = args[2].toInt32();
                var isTracked = !!S_vfsFileHandles[diagHandle];
                emitEvent({
                    type: "READFILE_DIAG",
                    callNum: S_readFileDiagCount,
                    handle: diagHandle,
                    requestedSize: diagSize,
                    isTrackedVfs: isTracked,
                    trackedHandleCount: Object.keys(S_vfsFileHandles).length,
                });
            }

            // Original filter logic
            this._rfSkip = true;
            var handle = args[0].toInt32();
            var info = S_vfsFileHandles[handle];
            if (!info) return; // Not a tracked VFS handle

            this._rfSkip = false;
            this._rfHandle = handle;
            this._rfBufAddr = args[1];
            this._rfRequestedSize = args[2].toInt32();
            this._rfBytesReadPtr = args[3];
            this._rfReturnAddr = this.returnAddress;
            this._rfDirHash = info.dirHash;
            this._rfPath = info.path;
        },
        onLeave: function (retval) {
            if (this._rfSkip) return;

            // Read actual bytes read
            var bytesRead = 0;
            try {
                if (this._rfBytesReadPtr && !this._rfBytesReadPtr.isNull()) {
                    bytesRead = this._rfBytesReadPtr.readU32();
                }
            } catch (e) {}

            if (bytesRead === 0) return;

            handleVfsRead(
                this._rfHandle, this._rfBufAddr, bytesRead,
                this._rfRequestedSize, this._rfReturnAddr,
                this._rfDirHash, this._rfPath, "ReadFile"
            );
        },
    }, "ReadFile");

    if (ok) {
        log("ReadFile hook installed (Track F — filtering on tracked VFS handles)");
    }
}

// v7: Shared VFS read handler — used by both ReadFile and NtReadFile hooks
function handleVfsRead(handle, bufAddr, bytesRead, requestedSize, returnAddr, dirHash, path, source) {
    S_readFileCallCount++;
    var callNum = S_readFileCallCount;

    // v15: Trigger summary write from rich context every 200 reads.
    // This guarantees the summary is written from the context that has data,
    // bypassing the dual-context timer alternation bug.
    if (callNum % 200 === 0) {
        try { writeSummary(); } catch (e) {}
    }

    // Record return address frequency
    var retAddrStr = returnAddr.toString();
    S_readReturnAddrs[retAddrStr] = (S_readReturnAddrs[retAddrStr] || 0) + 1;

    // Resolve return address module info
    var retModule = null;
    var retModuleOffset = null;
    try {
        var mod = Process.findModuleByAddress(returnAddr);
        if (mod) {
            retModule = mod.name;
            retModuleOffset = "0x" + returnAddr.sub(mod.base).toString(16);
        }
    } catch (e) {}

    // For first MAX_READ_DETAILED calls: full hex dump + entropy
    var bufHex = "";
    var entropy = -1;
    if (callNum <= MAX_READ_DETAILED) {
        var snapSize = Math.min(bytesRead, MAX_SNAPSHOT_BYTES);
        bufHex = hexDump(bufAddr, snapSize);
        entropy = computeEntropy(bufAddr, snapSize);
    }

    // v12: Accumulate VFS_READ sample in memory (first 50 — always stored regardless of JSONL)
    if (S_vfsReadSample.length < 50) {
        S_vfsReadSample.push({
            callNum: callNum,
            dirHash: dirHash,
            bytesRead: bytesRead,
            entropy: entropy,
            returnAddr: retAddrStr,
            returnModule: retModule,
            returnModuleOffset: retModuleOffset,
            source: source,
        });
    }

    // Emit VFS_READ event (up to MAX_READ_EVENTS)
    if (callNum <= MAX_READ_EVENTS) {
        emitEvent({
            type: "VFS_READ",
            callNum: callNum,
            handle: handle,
            dirHash: dirHash,
            path: path,
            bytesRead: bytesRead,
            requestedSize: requestedSize,
            bufHex: bufHex,
            entropy: entropy,
            returnAddr: retAddrStr,
            returnModule: retModule,
            returnModuleOffset: retModuleOffset,
            source: source,
        });
    }

    // v11: Log first N directly (not buffered — buffered logs from I/O threads are lost)
    if (callNum <= MAX_READ_LOG) {
        logDirect("VFS_READ #" + callNum + " [" + source + "]: " + dirHash +
            " " + bytesRead + "B" +
            (entropy >= 0 ? " entropy=" + entropy : "") +
            " ret=" + retAddrStr +
            (retModule ? " (" + retModule + "+" + retModuleOffset + ")" : ""));
    }

    // v16: Track per-handle read offset (ReadFile only — each physical read fires twice,
    // once as NtReadFile and once as ReadFile, so only count ReadFile to avoid double-counting)
    var readOffset = -1;
    if (source === "ReadFile") {
        var hkey = "" + handle;
        if (!S_handleReadMap[hkey]) {
            S_handleReadMap[hkey] = {
                path: path,
                dirHash: dirHash,
                totalRead: 0,
                chunks: [],
            };
        }
        readOffset = S_handleReadMap[hkey].totalRead;
        S_handleReadMap[hkey].totalRead += bytesRead;
    }

    // v17: Postcheck ALL ReadFile VFS reads (not just first MAX_READ_DETAILED).
    // Uses lightweight 64-byte pre-snapshot for mutation detection.
    // Decoupled from MAX_READ_DETAILED which only controls VFS_READ event detail.
    if (source === "ReadFile" && S_handleDumpCount < MAX_FILE_DUMPS) {
        // v17: Take a small snapshot for mutation detection (64 bytes is enough)
        var pcSnapSize = Math.min(bytesRead, 64);
        var pcPreHex = hexDump(bufAddr, pcSnapSize);
        var pcPreEntropy = computeEntropy(bufAddr, pcSnapSize);
        var readCallNum = callNum;
        var readBytesRead = bytesRead;
        var readBufAddr = bufAddr;
        var readDirHash = dirHash;
        var readHandle = handle;
        var capturedOffset = readOffset;

        setTimeout(function () {
            try {
                var postHex = hexDump(readBufAddr, pcSnapSize);

                if (postHex !== pcPreHex) {
                    var postEntropy = computeEntropy(readBufAddr, pcSnapSize);
                    var entropyDelta = pcPreEntropy - postEntropy;
                    var format = detectFormat(readBufAddr);

                    S_postcheckResults.push({
                        callNum: readCallNum,
                        dirHash: readDirHash,
                        bytesRead: readBytesRead,
                        preEntropy: pcPreEntropy,
                        postEntropy: postEntropy,
                        entropyDelta: Math.round(entropyDelta * 1000) / 1000,
                        detectedFormat: format,
                        preHex: pcPreHex.substring(0, 32),
                        postHex: postHex.substring(0, 32),
                    });

                    emitEvent({
                        type: "VFS_READ_POSTCHECK",
                        callNum: readCallNum,
                        dirHash: readDirHash,
                        bytesRead: readBytesRead,
                        preHex: pcPreHex,
                        postHex: postHex,
                        preEntropy: pcPreEntropy,
                        postEntropy: postEntropy,
                        entropyDelta: Math.round(entropyDelta * 1000) / 1000,
                        detectedFormat: format,
                    });

                    // Log first 50 mutations per dir
                    if (S_handleDumpCount <= 200 && S_handleDumpCount % 50 === 0) {
                        logDirect("VFS_DUMP progress: " + S_handleDumpCount +
                            " chunks dumped, dir=" + readDirHash);
                    }

                    // Dump FULL decrypted buffer
                    var dumpFile = writeDecryptedDump(readBufAddr, readBytesRead,
                        S_handleDumpCount);
                    S_handleDumpCount++;

                    // Record chunk in handle's read map
                    var hkey = "" + readHandle;
                    var track = S_handleReadMap[hkey];
                    if (track && dumpFile) {
                        track.chunks.push({
                            offset: capturedOffset,
                            size: readBytesRead,
                            dumpFile: dumpFile,
                            callNum: readCallNum,
                            format: format,
                        });
                    }
                }
            } catch (e) {}
        }, 0);
    }

    // After threshold: attempt loader discovery
    if (callNum === LOADER_DISCOVERY_THRESHOLD && !S_loaderHooked) {
        setTimeout(function () {
            discoverLoaderFunction();
        }, 0);
    }
}

// v7: NtReadFile hook — low-level fallback that catches all read APIs
function installNtReadFileHook() {
    if (S_ntReadFileHooked) return;
    S_ntReadFileHooked = true;

    var ntReadFile = null;
    var mods = Process.enumerateModules();
    for (var mi = 0; mi < mods.length; mi++) {
        if (mods[mi].name.toLowerCase() === "ntdll.dll") {
            try {
                ntReadFile = mods[mi].getExportByName("NtReadFile");
                if (ntReadFile) break;
            } catch (e) {}
        }
    }
    if (!ntReadFile) {
        log("NtReadFile not found — low-level read hook disabled");
        S_ntReadFileHooked = false;
        return;
    }

    // NtReadFile(FileHandle, Event, ApcRoutine, ApcContext, IoStatusBlock, Buffer, Length, ByteOffset, Key)
    var ok = safeAttach(ntReadFile, {
        onEnter: function (args) {
            this._ntSkip = true;
            var handle = args[0].toInt32();
            var info = S_vfsFileHandles[handle];
            if (!info) return;

            this._ntSkip = false;
            this._ntHandle = handle;
            this._ntBufAddr = args[5];
            this._ntLength = args[6].toInt32();
            this._ntReturnAddr = this.returnAddress;
            this._ntDirHash = info.dirHash;
            this._ntPath = info.path;
        },
        onLeave: function (retval) {
            if (this._ntSkip) return;

            // NtReadFile returns NTSTATUS; bytesRead is in IoStatusBlock
            // For simplicity, use the requested length as an approximation
            var bytesRead = this._ntLength;
            if (bytesRead <= 0) return;

            handleVfsRead(
                this._ntHandle, this._ntBufAddr, bytesRead,
                this._ntLength, this._ntReturnAddr,
                this._ntDirHash, this._ntPath, "NtReadFile"
            );
        },
    }, "NtReadFile");

    if (ok) log("NtReadFile hook installed (low-level fallback for Track F)");
}

function discoverLoaderFunction() {
    if (S_loaderHooked) {
        log("LOADER DISCOVERY: skipped — loader already hooked");
        return;
    }

    // v9: ONLY consider return addresses within GameAssembly.dll
    // v8 hooked a system DLL (336K calls, 9-byte modifications to file paths) — NOT VFS decrypt
    if (!S_gaBase || !S_gaSize) {
        log("LOADER DISCOVERY: GA module not found yet — cannot filter return addresses");
        return;
    }

    var gaStart = S_gaBase;
    var gaEnd = S_gaBase.add(S_gaSize);

    // Collect ALL return addresses for logging, mark GA-only ones
    var allAddrs = [];
    var gaAddrs = [];
    for (var addr in S_readReturnAddrs) {
        var entry = { addr: addr, count: S_readReturnAddrs[addr] };
        allAddrs.push(entry);

        // Check if this address is within GA
        try {
            var p = ptr(addr);
            if (p.compare(gaStart) >= 0 && p.compare(gaEnd) < 0) {
                gaAddrs.push(entry);
            }
        } catch (e) {}
    }
    allAddrs.sort(function (a, b) { return b.count - a.count; });
    gaAddrs.sort(function (a, b) { return b.count - a.count; });

    // Log all return addresses for diagnostics
    log("LOADER DISCOVERY: " + allAddrs.length + " unique return addresses (" +
        gaAddrs.length + " in GA) from " + S_readFileCallCount + " VFS reads");
    for (var i = 0; i < Math.min(allAddrs.length, 8); i++) {
        var retMod = null;
        var retModOff = null;
        var inGA = false;
        try {
            var p2 = ptr(allAddrs[i].addr);
            inGA = p2.compare(gaStart) >= 0 && p2.compare(gaEnd) < 0;
            var m = Process.findModuleByAddress(p2);
            if (m) {
                retMod = m.name;
                retModOff = "0x" + p2.sub(m.base).toString(16);
            }
        } catch (e) {}
        var marker = inGA ? " [GA]" : " [SYSTEM - SKIPPED]";
        log("  " + allAddrs[i].addr + ": " + allAddrs[i].count + " calls" +
            (retMod ? " (" + retMod + "+" + retModOff + ")" : "") + marker);
    }

    if (gaAddrs.length === 0) {
        log("LOADER DISCOVERY: No return addresses within GameAssembly.dll — VFS reads may use system calls only");
        log("  GA range: " + gaStart + " — " + gaEnd + " (size=" + S_gaSize + ")");
        return;
    }

    // Take top GA-only candidate
    var topAddr = ptr(gaAddrs[0].addr);
    var topCount = gaAddrs[0].count;

    // Find the enclosing function
    var funcStart = findFunctionBoundary(topAddr);
    if (!funcStart) {
        log("LOADER DISCOVERY: Could not find function boundary for top GA return address " + topAddr);
        if (gaAddrs.length > 1) {
            topAddr = ptr(gaAddrs[1].addr);
            topCount = gaAddrs[1].count;
            funcStart = findFunctionBoundary(topAddr);
            if (!funcStart) {
                log("LOADER DISCOVERY: Could not find function boundary for second GA candidate either");
                return;
            }
        } else {
            return;
        }
    }

    var funcModOffset = "0x" + funcStart.sub(S_gaBase).toString(16);

    log("VFS LOADER DETECTED at GA+" + funcModOffset + " (" + funcStart + ")" +
        " — return addr " + topAddr + " (" + topCount + " GA calls)");

    S_loaderFuncAddr = funcStart;

    // v12: Store loader info in memory for summary dump
    S_loaderInfo = {
        funcAddr: funcStart.toString(),
        funcRVA: funcModOffset,
        returnAddr: topAddr.toString(),
        returnAddrCount: topCount,
        totalReturnAddrs: allAddrs.length,
        gaReturnAddrs: gaAddrs.length,
        topCandidates: gaAddrs.slice(0, 5).map(function (a) {
            var rMod = null; var rOff = null;
            try { var m = Process.findModuleByAddress(ptr(a.addr)); if (m) { rMod = m.name; rOff = "0x" + ptr(a.addr).sub(m.base).toString(16); } } catch (e) {}
            return { addr: a.addr, count: a.count, module: rMod, offset: rOff };
        }),
        allReturnAddrs: allAddrs.map(function (a) {
            var rMod = null; var rOff = null;
            try { var m = Process.findModuleByAddress(ptr(a.addr)); if (m) { rMod = m.name; rOff = "0x" + ptr(a.addr).sub(m.base).toString(16); } } catch (e) {}
            return { addr: a.addr, count: a.count, module: rMod, offset: rOff };
        }),
    };

    emitEvent({
        type: "VFS_LOADER_DETECTED",
        funcAddr: funcStart.toString(),
        funcRVA: funcModOffset,
        returnAddr: topAddr.toString(),
        returnAddrCount: topCount,
        moduleName: "GameAssembly.dll",
        moduleOffset: funcModOffset,
        totalReturnAddrs: allAddrs.length,
        gaReturnAddrs: gaAddrs.length,
        topCandidates: gaAddrs.slice(0, 5).map(function (a) {
            return { addr: a.addr, count: a.count };
        }),
    });

    hookLoaderFunction(funcStart);
}

function hookLoaderFunction(funcStart) {
    if (S_loaderHooked) return;
    S_loaderHooked = true;
    S_loaderStartTime = Date.now();

    var ok = safeAttach(funcStart, {
        onEnter: function (args) {
            S_loaderCallCount++;
            this._loaderCallNum = S_loaderCallCount;
            this._loaderEntry = Date.now();

            // v9: Rate limiter — detach if function is too hot (prevents game freeze)
            if (S_loaderCallCount === MAX_LOADER_RATE) {
                var elapsed = Date.now() - S_loaderStartTime;
                if (elapsed < 2000) {
                    log("LOADER RATE LIMIT: " + S_loaderCallCount + " calls in " + elapsed +
                        "ms — DETACHING (wrong function, too hot)");
                    S_loaderDetached = true;
                    try { Interceptor.detachAll(); } catch (e) {}
                    // Re-attach other critical hooks
                    setTimeout(function () {
                        log("Re-attaching essential hooks after loader detach...");
                        S_hookedAddrs = {};
                        S_loaderHooked = false;
                        try { installCreateFileWHook(); } catch (e) {}
                        try { installNtReadFileHook(); } catch (e) {}
                    }, 100);
                    return;
                }
            }

            // v9: Only scan args[0] and args[1] (reduced from 4), 128 bytes (reduced from 512)
            this._loaderArgs = [args[0], args[1], args[2], args[3]];
            this._preSnapshots = [];

            for (var i = 0; i < 2; i++) {
                var arg = args[i];
                var snap = { argIdx: i, addr: null, hex: "", entropy: -1 };

                try {
                    if (!arg.isNull() && arg.compare(ptr(0x10000)) > 0 &&
                        arg.compare(ptr("0x7FFFFFFFFFFF")) < 0) {
                        arg.readU8(); // test readability
                        snap.addr = arg;
                        snap.hex = hexDump(arg, 128);
                        snap.entropy = computeEntropy(arg, 128);

                        // Also try nested pointer at arg+0x18 (VFS context pattern)
                        try {
                            var nested = arg.add(0x18).readPointer();
                            if (!nested.isNull() && nested.compare(ptr(0x10000)) > 0) {
                                nested.readU8();
                                snap.nestedAddr = nested;
                                snap.nestedHex = hexDump(nested, 128);
                                snap.nestedEntropy = computeEntropy(nested, 128);
                            }
                        } catch (e2) {}
                    }
                } catch (e) {}

                this._preSnapshots.push(snap);
            }
        },
        onLeave: function (retval) {
            if (S_loaderDetached) return;

            var decryptDetected = false;
            var bestMatch = null;

            // Re-read buffers at same addresses — detect entropy drops or format magic
            for (var i = 0; i < this._preSnapshots.length; i++) {
                var snap = this._preSnapshots[i];
                if (!snap.addr) continue;

                try {
                    var postHex = hexDump(snap.addr, 128);
                    var postEntropy = computeEntropy(snap.addr, 128);
                    var fmt = detectFormat(snap.addr);
                    var entropyDelta = snap.entropy - postEntropy;

                    if ((snap.entropy > 6.0 && entropyDelta > 1.0) || fmt) {
                        decryptDetected = true;
                        bestMatch = {
                            argIdx: i,
                            addr: snap.addr,
                            preHex: snap.hex.substring(0, 128),
                            postHex: postHex.substring(0, 128),
                            preEntropy: snap.entropy,
                            postEntropy: postEntropy,
                            entropyDelta: Math.round(entropyDelta * 1000) / 1000,
                            format: fmt,
                        };
                    }

                    // Also check nested pointer
                    if (!decryptDetected && snap.nestedAddr) {
                        var nestPostHex = hexDump(snap.nestedAddr, 128);
                        var nestPostEntropy = computeEntropy(snap.nestedAddr, 128);
                        var nestFmt = detectFormat(snap.nestedAddr);
                        var nestDelta = snap.nestedEntropy - nestPostEntropy;

                        if ((snap.nestedEntropy > 6.0 && nestDelta > 1.0) || nestFmt) {
                            decryptDetected = true;
                            bestMatch = {
                                argIdx: i,
                                addr: snap.nestedAddr,
                                chain: "nested_0x18",
                                preHex: snap.nestedHex.substring(0, 128),
                                postHex: nestPostHex.substring(0, 128),
                                preEntropy: snap.nestedEntropy,
                                postEntropy: nestPostEntropy,
                                entropyDelta: Math.round(nestDelta * 1000) / 1000,
                                format: nestFmt,
                            };
                        }
                    }
                } catch (e) {}
            }

            if (decryptDetected && bestMatch) {
                S_loaderDecryptCount++;

                // Dump decrypted data
                var dumpFile = null;
                // Try to determine size from integer args
                var bufSize = MAX_DUMP_BYTES;
                for (var k = 0; k < 4; k++) {
                    try {
                        var intVal = this._loaderArgs[k].toInt32();
                        if (intVal > 0 && intVal < 0x10000000 &&
                            classifyArg(this._loaderArgs[k]) !== "pointer") {
                            bufSize = intVal;
                            break;
                        }
                    } catch (e) {}
                }

                if (S_loaderDecryptCount <= MAX_LOADER_DUMPS) {
                    dumpFile = writeDecryptedDump(bestMatch.addr,
                        Math.min(bufSize, MAX_DUMP_BYTES), 80000 + this._loaderCallNum);
                }

                emitEvent({
                    type: "VFS_LOADER_DECRYPT",
                    callNum: this._loaderCallNum,
                    argIdx: bestMatch.argIdx,
                    chain: bestMatch.chain || "direct",
                    preHex: bestMatch.preHex,
                    postHex: bestMatch.postHex,
                    preEntropy: bestMatch.preEntropy,
                    postEntropy: bestMatch.postEntropy,
                    entropyDelta: bestMatch.entropyDelta,
                    detectedFormat: bestMatch.format,
                    bufferSize: bufSize,
                    dumpFile: dumpFile,
                    retval: retval.toString(),
                    elapsedMs: Date.now() - this._loaderEntry,
                });

                if (S_loaderDecryptCount <= 10) {
                    log("VFS_LOADER_DECRYPT #" + S_loaderDecryptCount +
                        " call=" + this._loaderCallNum +
                        " entropy " + bestMatch.preEntropy + " → " + bestMatch.postEntropy +
                        " format=" + (bestMatch.format || "entropy_drop") +
                        " dump=" + (dumpFile || "none"));
                }
            }

            // Emit loader return events for first N calls (for analysis)
            if (this._loaderCallNum <= MAX_LOADER_CALLS) {
                var argVals = [];
                var argTypes = [];
                for (var ai = 0; ai < 4; ai++) {
                    argVals.push(this._loaderArgs[ai].toString());
                    argTypes.push(classifyArg(this._loaderArgs[ai]));
                }
                emitEvent({
                    type: "VFS_LOADER_RETURN",
                    callNum: this._loaderCallNum,
                    args: argVals,
                    argTypes: argTypes,
                    retval: retval.toString(),
                    retType: classifyArg(retval),
                    decryptDetected: decryptDetected,
                    preSnapshots: this._preSnapshots.map(function (s) {
                        return {
                            argIdx: s.argIdx,
                            hasAddr: !!s.addr,
                            entropy: s.entropy,
                            hasNested: !!s.nestedAddr,
                            nestedEntropy: s.nestedEntropy || -1,
                        };
                    }),
                    elapsedMs: Date.now() - this._loaderEntry,
                });
            }

            // Periodic status
            if (S_loaderCallCount % 500 === 0) {
                log("Loader calls: " + S_loaderCallCount +
                    " decrypts: " + S_loaderDecryptCount);
            }
        },
    }, "vfs_loader");

    if (ok) {
        log("VFS loader function hooked (Track F phase 2)");
    }
}

// ══════════════════════════════════════════════════════════════
// Track E: Unsafe.VFS.dll Probe
// ══════════════════════════════════════════════════════════════

function probeUnsafeVfsDll() {
    if (S_vfsDllProbed) return;
    S_vfsDllProbed = true;

    var vfsMod = findModuleByName("Unsafe.VFS.dll");

    if (!vfsMod) {
        setTimeout(function () {
            try {
                var m = findModuleByName("Unsafe.VFS.dll");
                if (m) installVfsDllHooks(m);
                else log("Unsafe.VFS.dll not found (will not retry)");
            } catch (e) {}
        }, 30000);
        return;
    }

    installVfsDllHooks(vfsMod);
}

function installVfsDllHooks(vfsMod) {
    log("Unsafe.VFS.dll found @ " + vfsMod.base);

    var exports = safeEnumerateExports(vfsMod);

    log("  " + exports.length + " exports found");
    emitEvent({
        type: "VFS_DLL_LOADED",
        moduleBase: vfsMod.base.toString(),
        exportCount: exports.length,
        exports: exports.map(function (e) { return e.name; }),
    });

    for (var i = 0; i < exports.length; i++) {
        var exp = exports[i];
        if (exp.type !== "function") continue;
        S_vfsDllCallCounts[exp.name] = 0;
        hookVfsDllExport(exp.name, ptr(exp.address));
    }
}

function hookVfsDllExport(name, addr) {
    var ok = safeAttach(addr, {
        onEnter: function (args) {
            S_vfsDllCallCounts[name]++;
            this._vfsName = name;
            this._vfsCallNum = S_vfsDllCallCounts[name];
            this._vfsArgs = [args[0], args[1], args[2], args[3]];
        },
        onLeave: function (retval) {
            if (this._vfsCallNum <= MAX_VFS_DLL_CALLS) {
                var argTypes = [];
                var argVals = [];
                for (var i = 0; i < 4; i++) {
                    argTypes.push(classifyArg(this._vfsArgs[i]));
                    argVals.push(this._vfsArgs[i].toString());
                }
                emitEvent({
                    type: "VFS_DLL_CALL",
                    export: this._vfsName,
                    callNum: this._vfsCallNum,
                    args: argVals,
                    argTypes: argTypes,
                    retval: retval.toString(),
                    retType: classifyArg(retval),
                });
            }
        },
    }, "vfs_" + name);

    if (ok) {
        log("  Hooked VFS." + name + " @ " + addr);
    }
}

// ══════════════════════════════════════════════════════════════
// Summary
// ══════════════════════════════════════════════════════════════

function writeSummary() {
    // v10: Flush pending events/logs BEFORE writing summary — ensures no data loss on exit
    try { flushPendingLogs(); } catch (e) {}
    try { flushPendingEvents(); } catch (e) {}

    // v15: DUAL-CONTEXT GUARD — Frida gadget mode has two V8 contexts.
    // The "rich" context has all interceptor data (VFS handles, GA base, reads).
    // The "empty" context only has READFILE_DIAG events (~200).
    // Only write summary.json from the context that has meaningful data.
    var handleCount = Object.keys(S_vfsFileHandles).length;
    var hasRichData = S_gaBase !== null || handleCount > 0 || S_allEvents.length > 500;
    if (!hasRichData) {
        // This is the "empty" context — skip writing to avoid overwriting good data
        return;
    }

    try {
        var summary = {
            timestamp: Date.now(),
            version: 17,
            processId: Process.id,
            arch: Process.arch,
            xluaBase: S_xluaBase ? S_xluaBase.toString() : null,
            gaBase: S_gaBase ? S_gaBase.toString() : null,
            decryptFuncAddr: S_decryptFuncAddr ? S_decryptFuncAddr.toString() : null,
            decryptCallCount: S_decryptCallCount,
            decryptSuccessCount: S_decryptSuccessCount,
            fileDumpCount: S_fileDumpCount,
            eventCounts: S_eventCount,
            hgpCallCounts: S_hgpCallCounts,
            vfsDllCallCounts: S_vfsDllCallCounts,
            vfsFileHandleCount: Object.keys(S_vfsFileHandles).length,
            totalEvents: S_allEvents.length,
            hookedAddresses: Object.keys(S_hookedAddrs).length,
            // v5 additions
            readFileCallCount: S_readFileCallCount,
            readReturnAddrCount: Object.keys(S_readReturnAddrs).length,
            loaderFuncAddr: S_loaderFuncAddr ? S_loaderFuncAddr.toString() : null,
            loaderCallCount: S_loaderCallCount,
            loaderDecryptCount: S_loaderDecryptCount,
            // v6 additions
            flushCount: S_flushCount,
            pendingEvents: S_pendingEvents.length,
            // v7 additions
            readFileDiagCount: S_readFileDiagCount,
            ntReadFileHooked: S_ntReadFileHooked,
            gaDiscoveredViaCallsite: S_gaDiscoveredViaCallsite,
            // v8 additions
            writeErrors: S_writeErrors,
            // v9 additions
            gaSize: S_gaSize,
            loaderDetached: S_loaderDetached,
            trackStatus: {
                A_xlua: S_xluaHooked ? "hooked" : "polling",
                B_ga_cache: S_gaHooked ? "hooked" : "polling",
                C_hgp: S_hgpHooked ? "hooked" : "polling",
                D_createFileW: S_createFileWHooked ? "hooked" : "pending",
                E_vfsDll: S_vfsDllProbed ? "probed" : "pending",
                F_readFile: S_readFileHooked ? (S_loaderHooked ? "loader_hooked" : "capturing") : "pending",
                F_ntReadFile: S_ntReadFileHooked ? "hooked" : "not_found",
                decrypt: S_decryptHooked ? "hooked" : "waiting",
            },
            // v12: Critical analysis data — accumulated in memory, immune to JSONL event loss
            readReturnAddrs: S_readReturnAddrs,
            postcheckResults: S_postcheckResults,
            vfsReadSample: S_vfsReadSample,
            callsiteFunctions: S_callsiteFunctions,
            loaderInfo: S_loaderInfo,
            // v16: File reconstruction data
            handleDumpCount: S_handleDumpCount,
            handleReadMap: S_handleReadMap,
        };
        var f = new File("vfs_decrypt_summary.json", "w");
        f.write(JSON.stringify(summary, null, 2));
        f.flush();
        f.close();
    } catch (e) {}
}

// ══════════════════════════════════════════════════════════════
// Initialization
// ══════════════════════════════════════════════════════════════

// v9: Create/truncate output files. No persistent handles — each flush opens/writes/closes.
function openOutputFiles() {
    try {
        var lf = new File("vfs_decrypt.log", "w");
        lf.close();
        var ef = new File("vfs_decrypt_events.jsonl", "w");
        ef.close();
        // v11: Critical events backup file
        var cf = new File("vfs_critical_events.jsonl", "w");
        cf.close();
        console.log("[vfs-decrypt] Created output files: vfs_decrypt.log, vfs_decrypt_events.jsonl, vfs_critical_events.jsonl");
    } catch (e) {
        console.log("[vfs-decrypt] WARN: Could not create output files: " + e.message);
    }
}

function ensureDumpDir() {
    try {
        var test = new File("vfs_decrypted/.keep", "w");
        test.write("");
        test.flush();
        test.close();
        log("Dump directory ready: vfs_decrypted/");
    } catch (e) {
        log("WARN: Could not create vfs_decrypted/ directory — dumps may fail");
        log("  Create it manually: mkdir vfs_decrypted");
    }
}

// ══════════════════════════════════════════════════════════════
// Boot — Launch all tracks in parallel (200ms polling)
// ══════════════════════════════════════════════════════════════

openOutputFiles();
ensureDumpDir();
logDirect("VFS Decrypt Interception Gadget v17 (capture all 4 encrypted dirs)");
logDirect("Process: " + Process.id + " (" + Process.arch + ")");
logDirect("Target: 4 encrypted dirs: " + ENCRYPTED_DIRS.join(", "));
logDirect("");
logDirect("Launching parallel polling tracks:");
logDirect("  Track A: xlua.dll + 0x" + XLUA_REGISTRATION_OFFSET.toString(16) + " → disassemble + 0x588 callsite scan");
logDirect("  Track B: GA + 0x" + DECRYPT_CACHE_RVA.toString(16) + " (cache pointer + GA callsite scan fallback)");
logDirect("  Track C: HGP.dll exports (independent)");
logDirect("  Track F: ReadFile + NtReadFile → return address clustering → dynamic loader discovery");
logDirect("");

// All tracks start polling immediately and independently
try { pollForXlua(); } catch (e) { logDirect("Track A error: " + e.message); }
try { pollForGA(); } catch (e) { logDirect("Track B error: " + e.message); }
try { pollForHGP(); } catch (e) { logDirect("Track C error: " + e.message); }

// Also try CreateFileW immediately (kernel32 is always loaded)
try {
    S_createFileWHooked = true;
    installCreateFileWHook();
} catch (e) {
    logDirect("CreateFileW early hook error: " + e.message);
    S_createFileWHooked = false; // retry when GA found
}

// v5: Install ReadFile hook immediately (kernel32 is always loaded)
try {
    installReadFileHook();
} catch (e) {
    logDirect("ReadFile early hook error: " + e.message);
    S_readFileHooked = false;
}

// v7: Install NtReadFile hook (ntdll is always loaded)
try {
    installNtReadFileHook();
} catch (e) {
    logDirect("NtReadFile early hook error: " + e.message);
    S_ntReadFileHooked = false;
}

// v6: Flush buffered events periodically from main Frida thread
// v7: Also flush buffered logs
setInterval(function () {
    flushPendingLogs();
    flushPendingEvents();
}, EVENT_FLUSH_INTERVAL);

// Write summary every 30s
setInterval(writeSummary, 30000);

// v7: Flush boot logs that accumulated during setup
flushPendingLogs();

logDirect("Gadget loaded. All tracks polling (200ms interval), summary written every 30s...");
