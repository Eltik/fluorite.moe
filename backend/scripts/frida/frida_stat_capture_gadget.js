/**
 * Phase S2: Unified Stat Capture Gadget for Arknights Endfield
 *
 * Single script that captures ALL 24 characters' complete stat profiles.
 * Key improvement over v11: IL2CPP class-guided object navigation for
 * reliable character identification (instead of heuristic "chr_" string scanning).
 *
 * 5 hooks (down from 10):
 *   Hook 1: AbilitySystem.InitAttributes    (0x39C5240)  — PRIMARY: full profile capture
 *   Hook 2: CharInfo._RefreshCharAttribute  (0x4B337C0)  — char info + klass detection
 *   Hook 3: CharInfo.get_attributes         (0x2668440)  — charMap confirmation + late capture
 *   Hook 4: Attributes._RefreshAll          (0x3B0A740)  — live update snapshots
 *   Hook 5: Attributes.GetValue             (0x3B05D60)  — lightweight runtime tracking
 *
 * Outputs:
 *   stat_capture_events.jsonl  — structured events for char_data_probe.rs
 *   stat_capture.log           — human-readable log
 *   stat_capture_summary.json  — periodic summary (N/24 chars identified)
 */

"use strict";

// ══════════════════════════════════════════════════════════════
// Configuration
// ══════════════════════════════════════════════════════════════

var GA_MODULE = "GameAssembly.dll";

var RVA = {
    AbilitySystem_InitAttributes:  0x39C5240,
    CharInfo_RefreshCharAttribute: 0x4B337C0,
    CharInfo_get_attributes:       0x2668440,
    Attributes_RefreshAll:         0x3B0A740,
    Attributes_GetValue:           0x3B05D60,
    // Scalar getters (Attributes* → double)
    GetAtkFinalScalarFromMainAttribute:    0x3B06E30,
    GetAtkFinalScalarFromStr:              0x3B07090,
    GetAtkFinalScalarFromAgi:              0x3B07150,
    GetAtkFinalScalarFromWisd:             0x3B07210,
    GetAtkFinalScalarFromWill:             0x3B072D0,
    GetHpBaseAddFromStrAttribute:          0x3B06C80,
    GetHealScalarFromWill:                 0x3B07C30,
    GetPhysicalResistanceFromAgiAttribute: 0x3B07890,
    GetEnergyResistanceFromWisdAttribute:  0x3B07A60,
};

// Attributes object field offsets
var ATTR_OFFSETS = {
    m_finalCachedData:              24,
    m_baseNonConvertedCachedData:   48,
    m_finalNonConvertedCachedData:  72,
    m_rawData:                      96,
    m_dataMin:                      104,
    m_dataMax:                      112,
    m_mainAttrType:                 136,
    m_subAttrType:                  144,
};

var OFFSETS_CharInfo = {
    templateId: 24,   // Il2CppString* (chr_XXXX)
    level:      32,   // int32
};

var IL2CPP_ARRAY_HEADER = 32;
var STRING_LENGTH_OFFSET = 16;
var STRING_CHARS_OFFSET  = 20;
var MAX_GETVALUE_EVENTS = 50000;

var ATTR_NAMES = [
    "hp", "str", "agi", "wisd", "will", "atk", "def", "maxHp",
    "maxUltimateSp", "maxPoise", "poiseRecTime", "poiseRecTimeScalar",
    "physicalDamageTakenScalar", "fireDamageTakenScalar",
    "pulseDamageTakenScalar", "crystDamageTakenScalar",
    "naturalDamageTakenScalar", "etherDamageTakenScalar",
    "weight", "criticalRate", "criticalDamageIncrease",
    "moveSpeedMultiplier", "turnRateMultiplier", "attackRate",
    "skillCooldownScalar", "normalAttackRange",
    "hpRecoveryPerSec", "hpRecoveryPerSecByHpRatio",
    "poiseTakenScalar", "poiseOutputScalar",
    "breakingAttackDamageTakenScalar", "knockDownTimeAddition",
    "healOutputIncrease", "healTakenIncrease", "lifeSteal",
    "ultimateSpGainScalar", "atbCostAddition", "skillCooldownAddition",
    "comboSkillCooldownScalar", "igniteDamageScalar",
    "physicalInflictionDamageScalar",
    "physicalDamageIncrease", "fireDamageIncrease",
    "pulseDamageIncrease", "crystDamageIncrease",
    "naturalDamageIncrease", "etherDamageIncrease",
];

// ══════════════════════════════════════════════════════════════
// State
// ══════════════════════════════════════════════════════════════

var gaBase = null;
var charMap = {};              // attrPtr string → { templateId, level, ... }
var capturedChars = {};        // templateId → full profile (only store first capture per char)
var seenAttrPtrs = {};         // attrPtr string → { firstSeen, count }
var eventCount = {};
var allEvents = [];
var scalarFns = null;          // 9 NativeFunction objects for scalar getters
var _currentInitSystem = null; // Set in Hook 1 onEnter for re-entrancy tracking
var _initAttrPtrs = [];        // attrPtrs captured during current InitAttributes call

var logFile = null;
var eventsFile = null;

// IL2CPP API cache
var IL2CPP = {};

// Cached path from AbilitySystem → CharInfo (discovered via reflection)
var _charInfoPath = null;

// ══════════════════════════════════════════════════════════════
// Logging + Output
// ══════════════════════════════════════════════════════════════

function log(msg) {
    console.log("[stat-capture] " + msg);
    if (logFile) {
        try { logFile.write(msg + "\n"); logFile.flush(); } catch (e) {}
    }
}

function emitEvent(data) {
    var json = JSON.stringify(data);
    console.log("IL2CPP_EVENT|" + json);
    allEvents.push(data);
    if (eventsFile) {
        try { eventsFile.write(json + "\n"); eventsFile.flush(); } catch (e) {}
    }
}

// ══════════════════════════════════════════════════════════════
// Module Discovery
// ══════════════════════════════════════════════════════════════

function findModule(name) {
    var mods = Process.enumerateModules();
    for (var i = 0; i < mods.length; i++) {
        if (mods[i].name.toLowerCase() === name.toLowerCase()) return mods[i];
    }
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

// ══════════════════════════════════════════════════════════════
// IL2CPP API Initialization (NEW — class-guided navigation)
// ══════════════════════════════════════════════════════════════

function initIL2CppApi() {
    var apis = [
        ["il2cpp_object_get_class",    "pointer", ["pointer"]],
        ["il2cpp_class_get_name",      "pointer", ["pointer"]],
        ["il2cpp_class_get_namespace",  "pointer", ["pointer"]],
        ["il2cpp_class_get_fields",    "pointer", ["pointer", "pointer"]],
        ["il2cpp_field_get_name",      "pointer", ["pointer"]],
        ["il2cpp_field_get_offset",    "int",     ["pointer"]],
        ["il2cpp_field_get_type",      "pointer", ["pointer"]],
        ["il2cpp_type_get_name",       "pointer", ["pointer"]],
    ];

    var resolved = 0;
    for (var i = 0; i < apis.length; i++) {
        var name = apis[i][0];
        var retType = apis[i][1];
        var argTypes = apis[i][2];
        try {
            var addr = Module.findExportByName(null, name);
            if (addr) {
                IL2CPP[name] = new NativeFunction(addr, retType, argTypes);
                resolved++;
            }
        } catch (e) {
            log("  WARN: IL2CPP API " + name + ": " + e.message);
        }
    }
    log("  IL2CPP API: resolved " + resolved + "/" + apis.length);
    return resolved >= 4; // Need at minimum class_get_name, class_get_fields, field_get_name, field_get_offset
}

/**
 * Get the class name of an IL2CPP object (reads klass ptr at +0).
 * Returns null on failure.
 */
function safeGetClassName(objPtr) {
    if (!objPtr || objPtr.isNull()) return null;
    try {
        var klass = objPtr.readPointer();
        if (klass.isNull()) return null;

        if (IL2CPP.il2cpp_class_get_name) {
            var namePtr = IL2CPP.il2cpp_class_get_name(klass);
            if (!namePtr.isNull()) {
                var name = namePtr.readCString();
                if (name && name.length > 0 && name.length < 200) return name;
            }
        }

        // Fallback: direct struct read at +0x10
        var np = klass.add(0x10).readPointer();
        if (!np.isNull()) {
            var s = np.readCString();
            if (s && s.length >= 2 && s.length < 100 && /^[A-Za-z_][A-Za-z0-9_.<>]*$/.test(s)) {
                return s;
            }
        }
    } catch (e) {}
    return null;
}

/**
 * Get the IL2CPP class pointer from an object pointer.
 */
function getObjKlass(objPtr) {
    if (!objPtr || objPtr.isNull()) return null;
    try {
        if (IL2CPP.il2cpp_object_get_class) {
            return IL2CPP.il2cpp_object_get_class(objPtr);
        }
        // Fallback: klass is at +0
        return objPtr.readPointer();
    } catch (e) { return null; }
}

/**
 * Check if a field type name looks like an object reference (not a value type).
 */
function isObjectRef(typeName) {
    if (!typeName) return false;
    // Value types that aren't object refs
    var valueTypes = [
        "System.Int32", "System.Int64", "System.Single", "System.Double",
        "System.Boolean", "System.Byte", "System.UInt32", "System.UInt64",
        "System.Int16", "System.UInt16", "System.Char", "System.IntPtr",
        "Int32", "Int64", "Single", "Double", "Boolean", "Byte",
        "UInt32", "UInt64", "Int16", "UInt16", "Char", "IntPtr",
        "System.Nullable`1", "Nullable`1",
    ];
    for (var i = 0; i < valueTypes.length; i++) {
        if (typeName === valueTypes[i]) return false;
    }
    return true;
}

/**
 * IL2CPP class-guided walk to find CharInfo (NEW).
 * Starting from objPtr, enumerate fields using IL2CPP reflection API,
 * check each object-type field's class name. Recurse one level deep.
 * Caches the discovered offset path for subsequent calls.
 */
function findCharInfoViaReflection(objPtr) {
    if (!objPtr || objPtr.isNull()) return null;
    if (!IL2CPP.il2cpp_class_get_fields || !IL2CPP.il2cpp_field_get_name) return null;

    // Fast path: use cached offset chain
    if (_charInfoPath) {
        return followCachedPath(objPtr, _charInfoPath);
    }

    var klass = getObjKlass(objPtr);
    if (!klass || klass.isNull()) return null;

    var iter = Memory.alloc(Process.pointerSize);

    // Depth 1: scan fields of this object
    iter.writePointer(ptr(0));
    for (var i = 0; i < 60; i++) {
        var field;
        try { field = IL2CPP.il2cpp_class_get_fields(klass, iter); } catch (e) { break; }
        if (field.isNull()) break;

        var offset;
        try { offset = IL2CPP.il2cpp_field_get_offset(field); } catch (e) { continue; }
        if (offset < 16 || offset > 512) continue; // skip klass/gc_desc/implausible offsets

        // Read field value as pointer
        var fieldVal;
        try { fieldVal = objPtr.add(offset).readPointer(); } catch (e) { continue; }
        if (!fieldVal || fieldVal.isNull()) continue;
        if (fieldVal.compare(ptr(0x10000)) < 0) continue;
        if (fieldVal.compare(ptr("0x7FFFFFFFFFFF")) > 0) continue;

        var cn;
        try { cn = safeGetClassName(fieldVal); } catch (e) { continue; }

        if (cn === "CharInfo") {
            _charInfoPath = [{ off: offset }];
            log("  >> CharInfo path discovered: obj+" + offset + " (depth 1)");
            return readCharInfo(fieldVal);
        }

        // Depth 2: if this field is an object, enumerate ITS fields
        if (cn && isObjectRef(cn)) {
            var childKlass = getObjKlass(fieldVal);
            if (!childKlass || childKlass.isNull()) continue;

            var iter2 = Memory.alloc(Process.pointerSize);
            iter2.writePointer(ptr(0));

            for (var j = 0; j < 40; j++) {
                var field2;
                try { field2 = IL2CPP.il2cpp_class_get_fields(childKlass, iter2); } catch (e) { break; }
                if (field2.isNull()) break;

                var off2;
                try { off2 = IL2CPP.il2cpp_field_get_offset(field2); } catch (e) { continue; }
                if (off2 < 16 || off2 > 512) continue;

                var val2;
                try { val2 = fieldVal.add(off2).readPointer(); } catch (e) { continue; }
                if (!val2 || val2.isNull()) continue;
                if (val2.compare(ptr(0x10000)) < 0) continue;
                if (val2.compare(ptr("0x7FFFFFFFFFFF")) > 0) continue;

                var cn2;
                try { cn2 = safeGetClassName(val2); } catch (e) { continue; }

                if (cn2 === "CharInfo") {
                    _charInfoPath = [{ off: offset }, { off: off2 }];
                    log("  >> CharInfo path discovered: obj+" + offset + "→+" + off2 + " via " + cn + " (depth 2)");
                    return readCharInfo(val2);
                }
            }
        }
    }

    return null; // Not found
}

/**
 * Follow a cached offset path to reach CharInfo.
 */
function followCachedPath(objPtr, path) {
    if (!objPtr || objPtr.isNull() || !path || path.length === 0) return null;
    try {
        var current = objPtr;
        for (var i = 0; i < path.length; i++) {
            current = current.add(path[i].off).readPointer();
            if (!current || current.isNull()) return null;
        }
        // Verify it's still CharInfo
        var cn = safeGetClassName(current);
        if (cn === "CharInfo") {
            return readCharInfo(current);
        }
        // Path invalid — reset cache and retry full walk
        _charInfoPath = null;
        return null;
    } catch (e) {
        _charInfoPath = null;
        return null;
    }
}

// ══════════════════════════════════════════════════════════════
// IL2CPP Memory Reading Helpers
// ══════════════════════════════════════════════════════════════

function readCString(p) {
    if (p.isNull()) return "";
    try { return p.readCString(); } catch (e) { return ""; }
}

function readIl2cppString(p) {
    if (p.isNull()) return null;
    try {
        var len = p.add(STRING_LENGTH_OFFSET).readS32();
        if (len <= 0 || len > 1024) return null;
        return p.add(STRING_CHARS_OFFSET).readUtf16String(len);
    } catch (e) { return null; }
}

function readDoubleArray(arrayPtr) {
    if (!arrayPtr || arrayPtr.isNull()) return null;
    try {
        var maxLen = arrayPtr.add(24).readS32();
        if (maxLen <= 0 || maxLen > 1000) return null;
        var data = arrayPtr.add(IL2CPP_ARRAY_HEADER);
        var result = [];
        for (var i = 0; i < maxLen; i++) {
            result.push(data.add(i * 8).readDouble());
        }
        return result;
    } catch (e) { return null; }
}

function readAttributeDataCache(cacheAddr) {
    try {
        var dataPtr = cacheAddr.readPointer();
        if (!dataPtr || dataPtr.isNull()) return null;
        var data = readDoubleArray(dataPtr);
        if (!data) return null;
        var named = {};
        for (var i = 0; i < data.length && i < ATTR_NAMES.length; i++) {
            if (data[i] !== 0.0) named[ATTR_NAMES[i]] = data[i];
        }
        return { data: data, namedAttrs: named };
    } catch (e) { return null; }
}

function readNullableAttrType(addr) {
    try {
        var hasValue = addr.add(4).readU8();
        if (!hasValue) return null;
        return { value: addr.readS32(), hasValue: true };
    } catch (e) { return null; }
}

function readAttributesObject(attrPtr) {
    if (!attrPtr || attrPtr.isNull()) return null;
    try {
        var rawDataPtr = attrPtr.add(ATTR_OFFSETS.m_rawData).readPointer();
        var rawData = readDoubleArray(rawDataPtr);
        var namedAttrs = {};
        if (rawData) {
            for (var i = 0; i < rawData.length && i < ATTR_NAMES.length; i++) {
                if (rawData[i] !== 0.0) namedAttrs[ATTR_NAMES[i]] = rawData[i];
            }
        }
        return {
            rawData: rawData,
            namedAttrs: namedAttrs,
            finalCachedData: readAttributeDataCache(attrPtr.add(ATTR_OFFSETS.m_finalCachedData)),
            baseNonConverted: readAttributeDataCache(attrPtr.add(ATTR_OFFSETS.m_baseNonConvertedCachedData)),
            finalNonConverted: readAttributeDataCache(attrPtr.add(ATTR_OFFSETS.m_finalNonConvertedCachedData)),
            dataMin: readDoubleArray(attrPtr.add(ATTR_OFFSETS.m_dataMin).readPointer()),
            dataMax: readDoubleArray(attrPtr.add(ATTR_OFFSETS.m_dataMax).readPointer()),
            mainAttrType: readNullableAttrType(attrPtr.add(ATTR_OFFSETS.m_mainAttrType)),
            subAttrType: readNullableAttrType(attrPtr.add(ATTR_OFFSETS.m_subAttrType)),
        };
    } catch (e) { return null; }
}

function readCharInfo(charInfoPtr) {
    if (!charInfoPtr || charInfoPtr.isNull()) return null;
    var result = { ptr: charInfoPtr.toString() };
    try {
        var templateIdPtr = charInfoPtr.add(OFFSETS_CharInfo.templateId).readPointer();
        if (!templateIdPtr.isNull()) {
            result.templateId = readIl2cppString(templateIdPtr);
        }
    } catch (e) {}
    try {
        result.level = charInfoPtr.add(OFFSETS_CharInfo.level).readS32();
    } catch (e) {}
    return result;
}

// ══════════════════════════════════════════════════════════════
// Heuristic Fallbacks (from v11, as safety net)
// ══════════════════════════════════════════════════════════════

/**
 * Scan an object's pointer fields for an Il2CppString containing "chr_".
 */
function probeForCharId(objPtr) {
    if (!objPtr || objPtr.isNull()) return null;
    try {
        for (var off = 16; off <= 256; off += 8) {
            try {
                var fieldPtr = objPtr.add(off).readPointer();
                if (fieldPtr.isNull()) continue;
                var str = readIl2cppString(fieldPtr);
                if (str && str.indexOf("chr_") === 0) {
                    return { templateId: str, offset: off };
                }
            } catch (e) { continue; }
        }
        // Depth 2: walk pointer fields one level deep
        for (var off2 = 16; off2 <= 256; off2 += 8) {
            try {
                var childPtr = objPtr.add(off2).readPointer();
                if (childPtr.isNull()) continue;
                if (childPtr.compare(ptr(0x10000)) < 0) continue;
                if (childPtr.compare(ptr("0x7FFFFFFFFFFF")) > 0) continue;
                for (var coff = 16; coff <= 128; coff += 8) {
                    try {
                        var fp = childPtr.add(coff).readPointer();
                        if (fp.isNull()) continue;
                        var s = readIl2cppString(fp);
                        if (s && s.indexOf("chr_") === 0) {
                            return { templateId: s, offset: off2, childOffset: coff };
                        }
                    } catch (e) { continue; }
                }
            } catch (e) { continue; }
        }
    } catch (e) {}
    return null;
}

/**
 * Scan an object's pointer fields for something that looks like an Attributes object.
 */
function probeForAttributes(objPtr) {
    if (!objPtr || objPtr.isNull()) return null;
    try {
        for (var off = 16; off <= 256; off += 8) {
            try {
                var fieldPtr = objPtr.add(off).readPointer();
                if (fieldPtr.isNull()) continue;
                var rawDataPtr = fieldPtr.add(96).readPointer();
                if (rawDataPtr.isNull()) continue;
                var arrLen = rawDataPtr.add(24).readS32();
                if (arrLen >= 40 && arrLen <= 200) {
                    return { attrPtr: fieldPtr, offset: off, rawDataLen: arrLen };
                }
            } catch (e) { continue; }
        }
    } catch (e) {}
    return null;
}

// ══════════════════════════════════════════════════════════════
// Character Mapping
// ══════════════════════════════════════════════════════════════

function registerCharMapping(attrPtr, charInfo) {
    if (!attrPtr || attrPtr.isNull() || !charInfo) return;
    var key = attrPtr.toString();
    if (!charMap[key] || (charInfo.templateId && !charMap[key].templateId)) {
        charMap[key] = charInfo;
    }
}

function lookupChar(attrPtr) {
    if (!attrPtr) return null;
    return charMap[attrPtr.toString()] || null;
}

// ══════════════════════════════════════════════════════════════
// Scalar Capture (from calibration script)
// ══════════════════════════════════════════════════════════════

function captureAllScalars(attrPtr) {
    if (!scalarFns) return null;
    var result = {};
    try {
        for (var name in scalarFns) {
            if (!scalarFns.hasOwnProperty(name)) continue;
            var val = scalarFns[name](attrPtr);
            if (!isNaN(val) && isFinite(val)) {
                result[name] = val;
            }
        }
    } catch (e) {
        log("  ERR captureAllScalars: " + e.message);
        return null;
    }
    return result;
}

// ══════════════════════════════════════════════════════════════
// Modifier Probe (from calibration script)
// ══════════════════════════════════════════════════════════════

function probeModifiers(attrPtr) {
    var modifiers = [];
    try {
        var modFieldPtr = attrPtr.add(16).readPointer();
        if (modFieldPtr.isNull()) return modifiers;

        var maxLen = modFieldPtr.add(24).readS32();
        if (maxLen <= 0 || maxLen > 100) return modifiers;

        var dataStart = modFieldPtr.add(IL2CPP_ARRAY_HEADER);

        for (var i = 0; i < maxLen; i++) {
            try {
                var modPtr = dataStart.add(i * 8).readPointer();
                if (modPtr.isNull()) continue;

                var mod = { index: i };
                var layerNames = [
                    "baseAdditions", "baseMultipliers", "baseFinalAdditions",
                    "baseFinalMultipliers", "additions", "multipliers",
                    "finalAdditions", "finalMultipliers"
                ];

                for (var j = 0; j < layerNames.length; j++) {
                    try {
                        var arrPtr = modPtr.add(16 + j * 8).readPointer();
                        if (!arrPtr.isNull()) {
                            var arr = readDoubleArray(arrPtr);
                            if (arr) {
                                var hasNonZero = arr.some(function(v) { return v !== 0; });
                                if (hasNonZero) {
                                    mod[layerNames[j]] = arr;
                                }
                            }
                        }
                    } catch (e) { /* skip layer */ }
                }

                if (Object.keys(mod).length > 1) {
                    modifiers.push(mod);
                }
            } catch (e) { /* skip modifier */ }
        }
    } catch (e) { /* modifier field is not a simple array */ }
    return modifiers;
}

// ══════════════════════════════════════════════════════════════
// Full Profile Capture (merged from calibration + gadget)
// ══════════════════════════════════════════════════════════════

/**
 * Capture the complete stat profile for a character.
 * Emits FULL_CHARACTER_CAPTURE event.
 */
function captureFullProfile(attrPtr, charInfo, source, idMethod) {
    if (!attrPtr || attrPtr.isNull()) return;
    var templateId = charInfo ? charInfo.templateId : null;
    if (!templateId || templateId === "(null)") return;

    // Skip if already captured for this character
    if (capturedChars[templateId]) {
        log("  (skip duplicate capture for " + templateId + ")");
        return;
    }

    var profile = readAttributesObject(attrPtr);
    if (!profile) {
        log("  ERR: readAttributesObject returned null for " + templateId);
        return;
    }

    // Scalars (9 cross-attribute coefficients)
    profile.scalars = captureAllScalars(attrPtr);

    // Modifier layers
    profile.modifiers = probeModifiers(attrPtr);

    var event = {
        type: "FULL_CHARACTER_CAPTURE",
        templateId: templateId,
        level: charInfo.level || null,
        source: source,
        identificationMethod: idMethod,
        attrPtr: attrPtr.toString(),
        rawData: profile.rawData,
        namedAttrs: profile.namedAttrs,
        finalCachedData: profile.finalCachedData,
        baseNonConverted: profile.baseNonConverted,
        finalNonConverted: profile.finalNonConverted,
        dataMin: profile.dataMin,
        dataMax: profile.dataMax,
        mainAttrType: profile.mainAttrType,
        subAttrType: profile.subAttrType,
        scalars: profile.scalars,
        modifiers: profile.modifiers,
        timestamp: Date.now(),
    };

    emitEvent(event);
    capturedChars[templateId] = event;

    var scalarCount = profile.scalars ? Object.keys(profile.scalars).length : 0;
    var modCount = profile.modifiers ? profile.modifiers.length : 0;
    log("  ★ CAPTURED " + templateId +
        " (lv" + (charInfo.level || "?") +
        ", " + (profile.rawData ? profile.rawData.length : 0) + " raw attrs" +
        ", " + scalarCount + " scalars" +
        ", " + modCount + " modifiers" +
        ", via " + idMethod + ")" +
        " [" + Object.keys(capturedChars).length + "/24]");
}

// ══════════════════════════════════════════════════════════════
// Hooks
// ══════════════════════════════════════════════════════════════

function installHooks() {
    var ga = findModule(GA_MODULE);
    if (!ga) { log("ERROR: " + GA_MODULE + " not found"); return; }
    gaBase = ga.base;
    log("GameAssembly @ " + gaBase);

    // ── Scalar NativeFunctions ──
    var scalarRvas = {
        "atkFromMain":          RVA.GetAtkFinalScalarFromMainAttribute,
        "atkFromStr":           RVA.GetAtkFinalScalarFromStr,
        "atkFromAgi":           RVA.GetAtkFinalScalarFromAgi,
        "atkFromWisd":          RVA.GetAtkFinalScalarFromWisd,
        "atkFromWill":          RVA.GetAtkFinalScalarFromWill,
        "hpFromStr":            RVA.GetHpBaseAddFromStrAttribute,
        "healFromWill":         RVA.GetHealScalarFromWill,
        "physResistFromAgi":    RVA.GetPhysicalResistanceFromAgiAttribute,
        "energyResistFromWisd": RVA.GetEnergyResistanceFromWisdAttribute,
    };

    scalarFns = {};
    for (var key in scalarRvas) {
        if (!scalarRvas.hasOwnProperty(key)) continue;
        try {
            scalarFns[key] = new NativeFunction(gaBase.add(scalarRvas[key]), "double", ["pointer"]);
        } catch (e) { log("  WARN: scalar " + key + ": " + e.message); }
    }
    log("  Resolved " + Object.keys(scalarFns).length + " scalar functions");

    // ── Hook 1: AbilitySystem.InitAttributes (PRIMARY) ──
    // Fires exactly 24 times (once per character). Full profile capture.
    var h1addr = gaBase.add(RVA.AbilitySystem_InitAttributes);
    eventCount["InitAttributes"] = 0;
    try {
        Interceptor.attach(h1addr, {
            onEnter: function (args) {
                eventCount["InitAttributes"]++;
                this._asPtr = args[0];
                _currentInitSystem = args[0];
                _initAttrPtrs = [];
            },
            onLeave: function () {
                var asPtr = this._asPtr;
                _currentInitSystem = null;

                if (!asPtr || asPtr.isNull()) return;

                // 1. Try IL2CPP class-guided walk (NEW, reliable)
                var charInfo = findCharInfoViaReflection(asPtr);
                var idMethod = "il2cpp_reflection";

                // 2. Fallback: heuristic scan
                if (!charInfo || !charInfo.templateId) {
                    var heuristic = probeForCharId(asPtr);
                    if (heuristic) {
                        charInfo = { templateId: heuristic.templateId, ptr: asPtr.toString() };
                        idMethod = "heuristic_scan";
                    }
                }

                // 3. Fallback: walk attrPtrs back-references
                if ((!charInfo || !charInfo.templateId) && _initAttrPtrs.length > 0) {
                    for (var k = 0; k < _initAttrPtrs.length; k++) {
                        var ci = findCharInfoViaReflection(ptr(_initAttrPtrs[k]));
                        if (ci && ci.templateId) {
                            charInfo = ci;
                            idMethod = "il2cpp_via_attrPtr";
                            break;
                        }
                    }
                }

                // Find Attributes pointer(s) via re-entrancy tracking or probing
                var attrPtrs = _initAttrPtrs.length > 0 ? _initAttrPtrs : [];
                if (attrPtrs.length === 0) {
                    var probed = probeForAttributes(asPtr);
                    if (probed) {
                        attrPtrs = [probed.attrPtr.toString()];
                    }
                }

                // Register mapping and capture
                if (charInfo && charInfo.templateId && charInfo.templateId !== "(null)") {
                    for (var i = 0; i < attrPtrs.length; i++) {
                        var ap = ptr(attrPtrs[i]);
                        registerCharMapping(ap, charInfo);
                        captureFullProfile(ap, charInfo, "InitAttributes", idMethod);
                    }
                } else {
                    // Emit a partial event so char_data_probe can attempt resolution
                    emitEvent({
                        type: "INIT_ATTRIBUTES_UNRESOLVED",
                        asPtr: asPtr.toString(),
                        attrPtrs: attrPtrs,
                        timestamp: Date.now(),
                    });
                    log("  InitAttributes #" + eventCount["InitAttributes"] + ": UNRESOLVED (no CharInfo found)");
                }
            },
        });
        log("  Hook 1: AbilitySystem.InitAttributes @ " + h1addr);
    } catch (e) {
        log("  FAILED Hook 1: " + e.message);
    }

    // ── Hook 2: CharInfo._RefreshCharAttribute ──
    // Provides charInfo → attrPtr mapping
    var h2addr = gaBase.add(RVA.CharInfo_RefreshCharAttribute);
    eventCount["RefreshCharAttribute"] = 0;
    try {
        Interceptor.attach(h2addr, {
            onEnter: function (args) {
                eventCount["RefreshCharAttribute"]++;
                this._charInfoPtr = args[0];
                this._charInfo = readCharInfo(args[0]);
            },
            onLeave: function () {
                if (!this._charInfo || !this._charInfo.templateId) return;

                // Probe for Attributes pointer inside CharInfo object
                var attrProbe = probeForAttributes(this._charInfoPtr);
                if (attrProbe) {
                    registerCharMapping(attrProbe.attrPtr, this._charInfo);

                    // Late capture: if not yet captured for this character
                    var tid = this._charInfo.templateId;
                    if (!capturedChars[tid]) {
                        captureFullProfile(attrProbe.attrPtr, this._charInfo, "RefreshCharAttribute", "hook2_direct");
                    }
                }

                emitEvent({
                    type: "CHAR_ATTRIBUTE_REFRESH",
                    charInfo: this._charInfo,
                    hasAttr: !!attrProbe,
                    attrPtr: attrProbe ? attrProbe.attrPtr.toString() : null,
                    timestamp: Date.now(),
                });
            },
        });
        log("  Hook 2: CharInfo._RefreshCharAttribute @ " + h2addr);
    } catch (e) {
        log("  FAILED Hook 2: " + e.message);
    }

    // ── Hook 3: CharInfo.get_attributes ──
    // Maps attrPtr → charInfo on return. Late-binding fallback for missed InitAttributes.
    var h3addr = gaBase.add(RVA.CharInfo_get_attributes);
    eventCount["get_attributes"] = 0;
    try {
        Interceptor.attach(h3addr, {
            onEnter: function (args) {
                eventCount["get_attributes"]++;
                this._charInfoPtr = args[0];
                this._charInfo = readCharInfo(args[0]);
            },
            onLeave: function (retval) {
                if (!retval || retval.isNull()) return;
                if (!this._charInfo || !this._charInfo.templateId) return;
                if (this._charInfo.templateId === "(null)") return;

                registerCharMapping(retval, this._charInfo);

                // Late capture: if character wasn't identified during InitAttributes
                var tid = this._charInfo.templateId;
                if (!capturedChars[tid]) {
                    captureFullProfile(retval, this._charInfo, "get_attributes", "hook3_late_bind");
                }
            },
        });
        log("  Hook 3: CharInfo.get_attributes @ " + h3addr);
    } catch (e) {
        log("  FAILED Hook 3: " + e.message);
    }

    // ── Hook 4: Attributes._RefreshAll ──
    // Captures live updates (e.g., equipment changes, level ups)
    var h4addr = gaBase.add(RVA.Attributes_RefreshAll);
    eventCount["RefreshAll"] = 0;
    try {
        Interceptor.attach(h4addr, {
            onEnter: function (args) {
                eventCount["RefreshAll"]++;
                this._attrPtr = args[0];

                // Track attrPtrs during InitAttributes (re-entrancy)
                if (_currentInitSystem && !_currentInitSystem.isNull()) {
                    _initAttrPtrs.push(args[0].toString());
                }
            },
            onLeave: function () {
                if (!this._attrPtr || this._attrPtr.isNull()) return;

                // Only emit if we know which character this is
                var charInfo = lookupChar(this._attrPtr);
                if (!charInfo || !charInfo.templateId) return;

                // Re-capture if we already have this character (updates)
                // Only re-emit periodically (not every RefreshAll)
                if (eventCount["RefreshAll"] % 100 === 0) {
                    var profile = readAttributesObject(this._attrPtr);
                    if (profile) {
                        emitEvent({
                            type: "ATTR_REFRESH_UPDATE",
                            templateId: charInfo.templateId,
                            attrPtr: this._attrPtr.toString(),
                            rawData: profile.rawData,
                            namedAttrs: profile.namedAttrs,
                            timestamp: Date.now(),
                        });
                    }
                }
            },
        });
        log("  Hook 4: Attributes._RefreshAll @ " + h4addr);
    } catch (e) {
        log("  FAILED Hook 4: " + e.message);
    }

    // ── Hook 5: Attributes.GetValue ──
    // Lightweight: just tracks unique attrPtrs and their values
    var h5addr = gaBase.add(RVA.Attributes_GetValue);
    eventCount["GetValue"] = 0;
    try {
        Interceptor.attach(h5addr, {
            onEnter: function (args) {
                eventCount["GetValue"]++;
                if (eventCount["GetValue"] > MAX_GETVALUE_EVENTS) return;

                var attrPtr = args[0];
                var key = attrPtr.toString();

                // First-seen tracking
                if (!seenAttrPtrs[key]) {
                    seenAttrPtrs[key] = { firstSeen: Date.now(), count: 0 };

                    // Register attrPtr class name
                    var cn = safeGetClassName(attrPtr);
                    emitEvent({
                        type: "ATTR_PTR_FIRST_SEEN",
                        attrPtr: key,
                        attrClassName: cn,
                        templateId: lookupChar(attrPtr) ? lookupChar(attrPtr).templateId : null,
                        duringInitAttributes: !!_currentInitSystem,
                        timestamp: Date.now(),
                    });
                }
                seenAttrPtrs[key].count++;
            },
        });
        log("  Hook 5: Attributes.GetValue @ " + h5addr);
    } catch (e) {
        log("  FAILED Hook 5: " + e.message);
    }

    log("All hooks installed. Waiting for game events...");
}

// ══════════════════════════════════════════════════════════════
// Summary + Export
// ══════════════════════════════════════════════════════════════

function writeSummary() {
    var capturedCount = Object.keys(capturedChars).length;
    var summary = {
        version: 2,
        timestamp: new Date().toISOString(),
        identified: capturedCount,
        total: 24,
        charInfoPath: _charInfoPath
            ? _charInfoPath.map(function(p) { return "+" + p.off; }).join("→")
            : "not_discovered",
        characters: {},
        unmappedAttrPtrs: [],
        eventCounts: eventCount,
        totalAttrPtrs: Object.keys(seenAttrPtrs).length,
    };

    // Characters
    for (var tid in capturedChars) {
        var c = capturedChars[tid];
        summary.characters[tid] = {
            level: c.level,
            source: c.source,
            identificationMethod: c.identificationMethod,
            rawDataLen: c.rawData ? c.rawData.length : 0,
            scalars: c.scalars ? Object.keys(c.scalars).length : 0,
            modifiers: c.modifiers ? c.modifiers.length : 0,
        };
    }

    // Unmapped attrPtrs (not associated with any character)
    for (var ap in seenAttrPtrs) {
        if (!charMap[ap]) {
            summary.unmappedAttrPtrs.push(ap);
        }
    }

    try {
        var f = new File("stat_capture_summary.json", "w");
        f.write(JSON.stringify(summary, null, 2));
        f.flush();
        f.close();
    } catch (e) {
        log("ERR writing summary: " + e.message);
    }

    log("── Summary: " + capturedCount + "/24 characters captured ──");
    for (var tid2 in capturedChars) {
        var cc = capturedChars[tid2];
        log("  " + tid2 + " (lv" + (cc.level || "?") + ") via " + cc.identificationMethod);
    }
    log("  " + summary.unmappedAttrPtrs.length + " unmapped attrPtrs");
    log("  charInfoPath: " + summary.charInfoPath);
}

// ══════════════════════════════════════════════════════════════
// Boot
// ══════════════════════════════════════════════════════════════

log("Phase S2 Stat Capture Gadget initializing...");
log("Process: " + Process.id + " (" + Process.arch + ")");

// Open output files
try {
    logFile = new File("stat_capture.log", "w");
    log("Opened stat_capture.log");
} catch (e) { log("WARN: Cannot open log file: " + e.message); }

try {
    eventsFile = new File("stat_capture_events.jsonl", "w");
    log("Opened stat_capture_events.jsonl");
} catch (e) { log("WARN: Cannot open events file: " + e.message); }

// Init IL2CPP API
var il2cppReady = initIL2CppApi();
if (!il2cppReady) {
    log("WARN: IL2CPP API incomplete — falling back to heuristic scanning");
}

// Install hooks (no startup delay — gadget mode loads early)
installHooks();

// Periodic summary (every 30s)
setInterval(function () {
    writeSummary();
}, 30000);

// First summary after 60s (give game time to fire InitAttributes)
setTimeout(function () {
    writeSummary();
}, 60000);

log("");
log("Stat capture active. All hooks installed.");
log("IL2CPP reflection: " + (il2cppReady ? "READY" : "FALLBACK MODE"));
log("Waiting for AbilitySystem.InitAttributes calls (expect 24)...");
log("");
