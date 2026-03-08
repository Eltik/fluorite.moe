/**
 * IL2CPP Metadata Scanner for Arknights Endfield
 *
 * Enumerates ALL IL2CPP classes, methods, and fields at runtime using
 * GameAssembly.dll's exported IL2CPP API functions (unprotected .text section).
 *
 * Usage:
 *   frida -p <game_pid> -l frida_il2cpp_scan.js
 *
 * Output: il2cpp_full_dump.json written to CWD via send() + external receiver,
 *         OR printed to stdout as JSON blocks for pipe capture.
 *
 * Read-only operation — no hooks, no code modification. Minimal detection risk.
 */

"use strict";

// ══════════════════════════════════════════════════════════════
// Configuration
// ══════════════════════════════════════════════════════════════

var CONFIG = {
    // Delay before scanning (ms) — let FairGuard finish initial checks
    STARTUP_DELAY: 60000,

    // Keyword tiers for class filtering
    TIER1_EXACT: [
        "CharacterData", "CharacterStat", "CharacterStats", "CharacterStatData",
        "CombatData", "CombatConfig", "BattleConfig", "BattleData",
        "CharacterTemplateData", "EnemyTemplateData", "EnemyStatData",
        "PlayerData", "PlayerStats", "UnitData", "UnitStats",
        "CharacterLevelData", "GrowthData", "GrowthCurve",
        "AbilityData", "SkillData", "WeaponData", "EquipmentData",
        "CharacterConfig", "StatConfig", "LevelConfig",
        "CharacterInfo", "BattleCharacterData", "RuntimeCharacterData",
        "CharacterProperty", "CharacterAttribute",
    ],

    TIER2_SUBSTRING: [
        "Stat", "HP", "ATK", "DEF", "MaxHp", "BaseAtk", "BaseDef",
        "Growth", "LevelUp", "Template", "Protocol", "Message",
        "CritRate", "CritDmg", "Attack", "Defense", "Health",
        "CharacterTable", "EnemyTable", "ConfigTable",
        "Protobuf", "MergeFrom", "ParseFrom", "Deserialize",
    ],

    TIER3_NAMESPACE: [
        "Data", "Config", "Protocol", "Network", "Battle",
        "Character", "Combat", "Stat", "Game", "Table",
    ],

    // Signature mode: classes with N+ numeric fields are stat candidates
    MIN_NUMERIC_FIELDS: 3,

    // Max classes to fully enumerate (safety limit)
    MAX_CLASSES: 100000,

    // Output mode: "stdout" prints JSON blocks, "file" uses Frida File API
    OUTPUT_MODE: "stdout",

    // Output file path (when OUTPUT_MODE is "file")
    OUTPUT_FILE: "il2cpp_full_dump.json",
};

// ══════════════════════════════════════════════════════════════
// IL2CPP API bindings
// ══════════════════════════════════════════════════════════════

var il2cpp = {};

var API_SIGNATURES = {
    // Domain
    "il2cpp_domain_get":                 { ret: "pointer", args: [] },
    "il2cpp_domain_get_assemblies":      { ret: "pointer", args: ["pointer", "pointer"] },

    // Assembly → Image
    "il2cpp_assembly_get_image":         { ret: "pointer", args: ["pointer"] },

    // Image
    "il2cpp_image_get_name":             { ret: "pointer", args: ["pointer"] },
    "il2cpp_image_get_class_count":      { ret: "uint32",  args: ["pointer"] },
    "il2cpp_image_get_class":            { ret: "pointer", args: ["pointer", "uint32"] },

    // Class
    "il2cpp_class_get_name":             { ret: "pointer", args: ["pointer"] },
    "il2cpp_class_get_namespace":        { ret: "pointer", args: ["pointer"] },
    "il2cpp_class_get_methods":          { ret: "pointer", args: ["pointer", "pointer"] },
    "il2cpp_class_get_fields":           { ret: "pointer", args: ["pointer", "pointer"] },
    "il2cpp_class_get_parent":           { ret: "pointer", args: ["pointer"] },
    "il2cpp_class_get_flags":            { ret: "uint32",  args: ["pointer"] },
    "il2cpp_class_get_interfaces":       { ret: "pointer", args: ["pointer", "pointer"] },
    "il2cpp_class_num_fields":           { ret: "int32",   args: ["pointer"] },
    "il2cpp_class_get_type":             { ret: "pointer", args: ["pointer"] },

    // Method
    "il2cpp_method_get_name":            { ret: "pointer", args: ["pointer"] },
    "il2cpp_method_get_param_count":     { ret: "uint32",  args: ["pointer"] },
    "il2cpp_method_get_return_type":     { ret: "pointer", args: ["pointer"] },
    "il2cpp_method_get_flags":           { ret: "uint32",  args: ["pointer", "pointer"] },

    // Field
    "il2cpp_field_get_name":             { ret: "pointer", args: ["pointer"] },
    "il2cpp_field_get_type":             { ret: "pointer", args: ["pointer"] },
    "il2cpp_field_get_offset":           { ret: "int32",   args: ["pointer"] },
    "il2cpp_field_get_flags":            { ret: "uint32",  args: ["pointer"] },

    // Type
    "il2cpp_type_get_name":              { ret: "pointer", args: ["pointer"] },
    "il2cpp_type_get_type":              { ret: "int32",   args: ["pointer"] },
    "il2cpp_type_is_byref":             { ret: "bool",    args: ["pointer"] },

    // String (for reading IL2CPP String objects)
    "il2cpp_string_chars":               { ret: "pointer", args: ["pointer"] },
    "il2cpp_string_length":              { ret: "int32",   args: ["pointer"] },
};

// IL2CPP type enum values (Il2CppTypeEnum)
var IL2CPP_TYPE = {
    VOID:       1,
    BOOLEAN:    2,
    CHAR:       3,
    I1:         4,
    U1:         5,
    I2:         6,
    U2:         7,
    I4:         8,
    U4:         9,
    I8:         10,
    U8:         11,
    R4:         12,
    R8:         13,
    STRING:     14,
    PTR:        15,
    VALUETYPE:  17,
    CLASS:      18,
    ARRAY:      20,
    GENERICINST: 21,
    SZARRAY:    29,
    OBJECT:     28,
};

var NUMERIC_TYPES = [
    IL2CPP_TYPE.I1, IL2CPP_TYPE.U1, IL2CPP_TYPE.I2, IL2CPP_TYPE.U2,
    IL2CPP_TYPE.I4, IL2CPP_TYPE.U4, IL2CPP_TYPE.I8, IL2CPP_TYPE.U8,
    IL2CPP_TYPE.R4, IL2CPP_TYPE.R8,
];

// ══════════════════════════════════════════════════════════════
// Utility
// ══════════════════════════════════════════════════════════════

function log(msg) {
    console.log("[il2cpp-scan] " + msg);
}

function readCString(ptr) {
    if (ptr.isNull()) return "";
    try {
        return ptr.readCString();
    } catch (e) {
        return "";
    }
}

function findModule(name) {
    var mods = Process.enumerateModules();
    for (var i = 0; i < mods.length; i++) {
        if (mods[i].name.toLowerCase() === name.toLowerCase()) {
            return mods[i];
        }
    }
    return null;
}

// ══════════════════════════════════════════════════════════════
// Resolve IL2CPP API exports
// ══════════════════════════════════════════════════════════════

function resolveApi() {
    log("Resolving IL2CPP API exports...");

    var ga = findModule("GameAssembly.dll");
    if (!ga) {
        log("ERROR: GameAssembly.dll not loaded");
        return false;
    }

    log("GameAssembly.dll at " + ga.base + " (" + ga.size + " bytes)");

    // Build export lookup table
    var exports = {};
    var gaExports = ga.enumerateExports();
    for (var i = 0; i < gaExports.length; i++) {
        exports[gaExports[i].name] = gaExports[i].address;
    }

    log("Found " + gaExports.length + " exports total");

    var resolved = 0;
    var missing = [];

    for (var name in API_SIGNATURES) {
        if (!API_SIGNATURES.hasOwnProperty(name)) continue;
        var sig = API_SIGNATURES[name];

        var addr = exports[name] || Module.findExportByName("GameAssembly.dll", name);

        if (addr) {
            il2cpp[name] = new NativeFunction(addr, sig.ret, sig.args);
            resolved++;
        } else {
            missing.push(name);
        }
    }

    log("Resolved " + resolved + "/" + Object.keys(API_SIGNATURES).length + " API functions");
    if (missing.length > 0) {
        log("Missing: " + missing.join(", "));
    }

    il2cpp._base = ga.base;
    il2cpp._size = ga.size;

    // Minimum required APIs
    var required = [
        "il2cpp_domain_get",
        "il2cpp_domain_get_assemblies",
        "il2cpp_assembly_get_image",
        "il2cpp_image_get_class_count",
        "il2cpp_image_get_class",
        "il2cpp_class_get_name",
        "il2cpp_class_get_namespace",
    ];

    for (var r = 0; r < required.length; r++) {
        if (!il2cpp[required[r]]) {
            log("FATAL: Required API " + required[r] + " not found");
            return false;
        }
    }

    return true;
}

// ══════════════════════════════════════════════════════════════
// Enumeration functions
// ══════════════════════════════════════════════════════════════

function getMethodRva(methodInfo) {
    // MethodInfo offset 0 = native function pointer
    try {
        var funcPtr = methodInfo.readPointer();
        if (funcPtr.isNull()) return 0;
        var rva = funcPtr.sub(il2cpp._base).toUInt32();
        if (rva > 0 && rva < il2cpp._size) {
            return rva;
        }
        return 0;
    } catch (e) {
        return 0;
    }
}

function enumerateMethods(klass) {
    if (!il2cpp.il2cpp_class_get_methods || !il2cpp.il2cpp_method_get_name) {
        return [];
    }

    var methods = [];
    var iter = Memory.alloc(Process.pointerSize);
    iter.writePointer(ptr(0));

    try {
        while (true) {
            var method = il2cpp.il2cpp_class_get_methods(klass, iter);
            if (method.isNull()) break;

            var name = readCString(il2cpp.il2cpp_method_get_name(method));
            var paramCount = il2cpp.il2cpp_method_get_param_count
                ? il2cpp.il2cpp_method_get_param_count(method)
                : -1;
            var rva = getMethodRva(method);

            var retType = "";
            if (il2cpp.il2cpp_method_get_return_type && il2cpp.il2cpp_type_get_name) {
                try {
                    var rt = il2cpp.il2cpp_method_get_return_type(method);
                    if (!rt.isNull()) {
                        retType = readCString(il2cpp.il2cpp_type_get_name(rt));
                    }
                } catch (e) { /* ignore */ }
            }

            methods.push({
                name: name,
                paramCount: paramCount,
                rva: rva,
                rvaHex: rva > 0 ? "0x" + rva.toString(16).toUpperCase() : "0x0",
                returnType: retType,
            });
        }
    } catch (e) {
        // Iterator exhausted or error
    }

    return methods;
}

function enumerateFields(klass) {
    if (!il2cpp.il2cpp_class_get_fields || !il2cpp.il2cpp_field_get_name) {
        return [];
    }

    var fields = [];
    var iter = Memory.alloc(Process.pointerSize);
    iter.writePointer(ptr(0));

    try {
        while (true) {
            var field = il2cpp.il2cpp_class_get_fields(klass, iter);
            if (field.isNull()) break;

            var name = readCString(il2cpp.il2cpp_field_get_name(field));

            var offset = -1;
            if (il2cpp.il2cpp_field_get_offset) {
                offset = il2cpp.il2cpp_field_get_offset(field);
            }

            var typeEnum = -1;
            var typeName = "";
            if (il2cpp.il2cpp_field_get_type) {
                var ftype = il2cpp.il2cpp_field_get_type(field);
                if (!ftype.isNull()) {
                    if (il2cpp.il2cpp_type_get_type) {
                        typeEnum = il2cpp.il2cpp_type_get_type(ftype);
                    }
                    if (il2cpp.il2cpp_type_get_name) {
                        typeName = readCString(il2cpp.il2cpp_type_get_name(ftype));
                    }
                }
            }

            var flags = 0;
            if (il2cpp.il2cpp_field_get_flags) {
                flags = il2cpp.il2cpp_field_get_flags(field);
            }

            // Static flag = 0x10 in FieldAttributes
            var isStatic = (flags & 0x10) !== 0;

            fields.push({
                name: name,
                offset: offset,
                typeEnum: typeEnum,
                typeName: typeName,
                isStatic: isStatic,
                isNumeric: NUMERIC_TYPES.indexOf(typeEnum) !== -1,
            });
        }
    } catch (e) {
        // Iterator exhausted or error
    }

    return fields;
}

function getParentName(klass) {
    if (!il2cpp.il2cpp_class_get_parent || !il2cpp.il2cpp_class_get_name) return "";
    try {
        var parent = il2cpp.il2cpp_class_get_parent(klass);
        if (parent.isNull()) return "";
        return readCString(il2cpp.il2cpp_class_get_name(parent));
    } catch (e) {
        return "";
    }
}

// ══════════════════════════════════════════════════════════════
// Matching logic
// ══════════════════════════════════════════════════════════════

function matchClass(name, namespace) {
    // Tier 1: exact class name match
    for (var i = 0; i < CONFIG.TIER1_EXACT.length; i++) {
        if (name === CONFIG.TIER1_EXACT[i]) {
            return { tier: 1, reason: "exact:" + CONFIG.TIER1_EXACT[i] };
        }
    }

    // Tier 2: substring in class name
    var nameUpper = name.toUpperCase();
    for (var j = 0; j < CONFIG.TIER2_SUBSTRING.length; j++) {
        if (nameUpper.indexOf(CONFIG.TIER2_SUBSTRING[j].toUpperCase()) !== -1) {
            return { tier: 2, reason: "substr:" + CONFIG.TIER2_SUBSTRING[j] };
        }
    }

    // Tier 3: namespace match
    if (namespace) {
        var nsUpper = namespace.toUpperCase();
        for (var k = 0; k < CONFIG.TIER3_NAMESPACE.length; k++) {
            // Check if namespace contains or equals the keyword
            if (nsUpper.indexOf(CONFIG.TIER3_NAMESPACE[k].toUpperCase()) !== -1) {
                return { tier: 3, reason: "ns:" + CONFIG.TIER3_NAMESPACE[k] };
            }
        }
    }

    return null;
}

// ══════════════════════════════════════════════════════════════
// Main scan
// ══════════════════════════════════════════════════════════════

function runScan() {
    log("Starting IL2CPP metadata scan...");

    var domain = il2cpp.il2cpp_domain_get();
    if (domain.isNull()) {
        log("ERROR: il2cpp_domain_get returned null");
        return;
    }
    log("Domain: " + domain);

    // Get assemblies
    var sizePtr = Memory.alloc(4);
    sizePtr.writeU32(0);
    var assemblies = il2cpp.il2cpp_domain_get_assemblies(domain, sizePtr);
    var assemblyCount = sizePtr.readU32();

    log("Found " + assemblyCount + " assemblies");

    if (assemblyCount === 0 || assemblies.isNull()) {
        log("ERROR: No assemblies found");
        return;
    }

    // Results
    var classIndex = [];       // ALL classes (name, namespace, image)
    var matchedClasses = [];   // Filtered by keyword tiers
    var signatureHits = [];    // Classes with 3+ numeric fields
    var totalClasses = 0;
    var totalMethods = 0;
    var totalFields = 0;

    // Iterate assemblies → images → classes
    for (var a = 0; a < assemblyCount; a++) {
        var assembly = assemblies.add(a * Process.pointerSize).readPointer();
        if (assembly.isNull()) continue;

        var image = il2cpp.il2cpp_assembly_get_image(assembly);
        if (image.isNull()) continue;

        var imageName = "";
        if (il2cpp.il2cpp_image_get_name) {
            imageName = readCString(il2cpp.il2cpp_image_get_name(image));
        }

        var classCount = il2cpp.il2cpp_image_get_class_count(image);

        if (a < 10 || classCount > 100) {
            log("  Assembly " + a + ": " + imageName + " (" + classCount + " classes)");
        }

        for (var c = 0; c < classCount; c++) {
            if (totalClasses >= CONFIG.MAX_CLASSES) {
                log("  Safety limit reached (" + CONFIG.MAX_CLASSES + " classes)");
                break;
            }

            var klass = il2cpp.il2cpp_image_get_class(image, c);
            if (klass.isNull()) continue;

            var name = readCString(il2cpp.il2cpp_class_get_name(klass));
            var namespace = readCString(il2cpp.il2cpp_class_get_namespace(klass));

            totalClasses++;

            // Add to full index
            classIndex.push({
                name: name,
                namespace: namespace,
                image: imageName,
            });

            // Check keyword match
            var match = matchClass(name, namespace);

            // Always enumerate fields for signature detection
            var fields = enumerateFields(klass);
            totalFields += fields.length;

            // Signature mode: 3+ instance numeric fields
            var numericCount = 0;
            for (var fi = 0; fi < fields.length; fi++) {
                if (fields[fi].isNumeric && !fields[fi].isStatic) {
                    numericCount++;
                }
            }

            var isSignatureHit = numericCount >= CONFIG.MIN_NUMERIC_FIELDS;

            // If matched by keyword OR signature, enumerate methods too
            if (match || isSignatureHit) {
                var methods = enumerateMethods(klass);
                totalMethods += methods.length;

                var parentName = getParentName(klass);

                var entry = {
                    name: name,
                    namespace: namespace,
                    image: imageName,
                    parent: parentName,
                    fields: fields,
                    methods: methods,
                    numericFieldCount: numericCount,
                    matchTier: match ? match.tier : 0,
                    matchReason: match ? match.reason : "",
                    isSignatureHit: isSignatureHit,
                };

                if (match) {
                    matchedClasses.push(entry);
                }
                if (isSignatureHit) {
                    signatureHits.push(entry);
                }

                // Log tier 1 hits immediately
                if (match && match.tier === 1) {
                    log("  TIER 1 HIT: " + namespace + "." + name +
                        " (" + fields.length + " fields, " + methods.length + " methods)");
                }
            }
        }

        if (totalClasses >= CONFIG.MAX_CLASSES) break;
    }

    // Sort matched classes by tier (1 first)
    matchedClasses.sort(function (a, b) {
        if (a.matchTier !== b.matchTier) return a.matchTier - b.matchTier;
        return b.numericFieldCount - a.numericFieldCount;
    });

    // Sort signature hits by numeric field count
    signatureHits.sort(function (a, b) {
        return b.numericFieldCount - a.numericFieldCount;
    });

    // Build output
    var output = {
        meta: {
            timestamp: new Date().toISOString(),
            processId: Process.id,
            arch: Process.arch,
            gameAssemblyBase: il2cpp._base.toString(),
            gameAssemblySize: il2cpp._size,
            totalAssemblies: assemblyCount,
            totalClasses: totalClasses,
            totalMethods: totalMethods,
            totalFields: totalFields,
            matchedClasses: matchedClasses.length,
            signatureHits: signatureHits.length,
        },
        matched: matchedClasses,
        signatureHits: signatureHits,
        classIndex: classIndex,
    };

    // Summary
    log("");
    log("═══════════════════════════════════════════");
    log("  SCAN COMPLETE");
    log("═══════════════════════════════════════════");
    log("  Total classes:     " + totalClasses);
    log("  Keyword matches:   " + matchedClasses.length);
    log("  Signature hits:    " + signatureHits.length);
    log("  Total methods:     " + totalMethods);
    log("  Total fields:      " + totalFields);
    log("");

    // Print top matches
    log("  Top keyword matches:");
    for (var m = 0; m < Math.min(matchedClasses.length, 30); m++) {
        var mc = matchedClasses[m];
        log("    [T" + mc.matchTier + "] " + mc.namespace + "." + mc.name +
            " (" + mc.fields.length + "F, " + mc.methods.length + "M, " +
            mc.numericFieldCount + " numeric) — " + mc.matchReason);
    }

    log("");
    log("  Top signature hits (3+ numeric fields):");
    for (var s = 0; s < Math.min(signatureHits.length, 20); s++) {
        var sh = signatureHits[s];
        log("    " + sh.namespace + "." + sh.name +
            " (" + sh.numericFieldCount + " numeric / " + sh.fields.length + " total) [" + sh.image + "]");
    }

    // Output JSON
    outputResults(output);
}

function outputResults(output) {
    var json = JSON.stringify(output, null, 2);

    if (CONFIG.OUTPUT_MODE === "file") {
        // Use Frida's File API
        try {
            var f = new File(CONFIG.OUTPUT_FILE, "w");
            f.write(json);
            f.flush();
            f.close();
            log("Wrote " + json.length + " bytes to " + CONFIG.OUTPUT_FILE);
        } catch (e) {
            log("File write failed: " + e.message + " — falling back to stdout");
            outputToStdout(json);
        }
    } else {
        outputToStdout(json);
    }
}

function outputToStdout(json) {
    // Print in chunks to avoid Frida message size limits
    var CHUNK_SIZE = 65000;

    console.log("=== IL2CPP_DUMP_BEGIN ===");
    for (var i = 0; i < json.length; i += CHUNK_SIZE) {
        console.log(json.substring(i, i + CHUNK_SIZE));
    }
    console.log("=== IL2CPP_DUMP_END ===");

    log("Dump output complete (" + json.length + " chars)");
    log("Extract JSON between IL2CPP_DUMP_BEGIN and IL2CPP_DUMP_END markers.");
}

// ══════════════════════════════════════════════════════════════
// Initialization
// ══════════════════════════════════════════════════════════════

log("IL2CPP Metadata Scanner initializing...");
log("Process: " + Process.id + " (" + Process.arch + ")");

// Check if GameAssembly.dll is loaded
var gaCheck = findModule("GameAssembly.dll");
if (gaCheck) {
    log("GameAssembly.dll already loaded at " + gaCheck.base);
} else {
    log("GameAssembly.dll not yet loaded — will wait");
}

log("Waiting " + (CONFIG.STARTUP_DELAY / 1000) + "s for FairGuard to settle...");
log("(Reduce CONFIG.STARTUP_DELAY if you know anti-cheat has passed)");

setTimeout(function () {
    log("Startup delay complete. Resolving API...");

    if (!resolveApi()) {
        log("FATAL: Could not resolve IL2CPP API. Aborting.");
        return;
    }

    log("API resolved. Starting class enumeration...");
    log("This may take 30-120 seconds for ~50K classes...");

    try {
        runScan();
    } catch (e) {
        log("SCAN ERROR: " + e.message);
        log("Stack: " + e.stack);
    }

    log("Scanner finished. You can detach Frida now.");
}, CONFIG.STARTUP_DELAY);
