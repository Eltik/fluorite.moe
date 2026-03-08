/**
 * Passive Character Data Hooks for Arknights Endfield (v11)
 *
 * PASSIVE: No heap scanning. Hooks observe natural game calls.
 * Deep probing: scans object fields for char IDs + Attributes pointers.
 * Scalar coefficients captured via lightweight NativeFunction calls.
 *
 * 10 hooks + 9 scalar getters:
 *   Hook 1:  CharInfo._RefreshCharAttribute           (0x4B337C0)
 *   Hook 2:  CharInfo.get_attributes                  (0x2668440) — NO klass filter, captures all
 *   Hook 3:  Attributes._RefreshAll                   (0x3B0A740)
 *   Hook 4:  Attributes.Dump                          (0x3B088E0)
 *   Hook 5:  AbilitySystem.InitAttributes             (0x39C5240) — deep probe for char ID + Attributes*
 *   Hook 6:  Attributes.GetValue                      (0x3B05D60) — tracks unique attrPtrs
 *   Hook 7:  GetCharacterAttributeDataFromTable       (0x530D6C0)
 *   Hook 8:  _CalculateBaseAttribute                  (0x530E390)
 *   Hook 9:  _CalculateFinalAttribute                 (0x530E720)
 *   Hook 10: DataManager.OnInit                       (0x53195C0)
 *
 * Outputs:
 *   il2cpp_char_events.jsonl  — structured events for char_data_probe.rs
 *   il2cpp_char_data.log      — human-readable log
 *   il2cpp_char_summary.json  — written periodically (includes charMap + attrPtr tracking)
 */

"use strict";

// ══════════════════════════════════════════════════════════════
// Configuration
// ══════════════════════════════════════════════════════════════

var GA_MODULE = "GameAssembly.dll";

var RVA = {
    CharInfo_RefreshCharAttribute: 0x4B337C0,
    CharInfo_get_attributes:       0x2668440,
    Attributes_RefreshAll:         0x3B0A740,
    Attributes_Dump:               0x3B088E0,
    AbilitySystem_InitAttributes:  0x39C5240,
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
    // AttributesCalculator (static methods)
    GetCharacterAttributeDataFromTable:    0x530D6C0,
    _CalculateBaseAttribute:               0x530E390,
    _CalculateFinalAttribute:              0x530E720,
    // DataManager instance methods
    DataManager_OnInit:                    0x53195C0,
    DataManager_get_characterAttributeTable: 0x39D3DE0,
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

// CharInfo object field offsets
var OFFSETS_CharInfo = {
    templateId: 24,   // Il2CppString* (chr_XXXX)
    level:      32,   // int32
};

var IL2CPP_ARRAY_HEADER = 32;
var STRING_LENGTH_OFFSET = 16;
var STRING_CHARS_OFFSET  = 20;
var MAX_EVENTS = 10000;

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
var charMap = {};
var eventCount = {};
var allEvents = [];
var charInfoKlass = null;
var scalarsCaptured = {};  // templateId → scalar data (only capture once per char)
var scalarFns = null;      // NativeFunctions for the 9 scalar getters
var seenAttrPtrs = {};     // attrPtr string → { firstSeen, count, charInfo }
var abilitySystems = [];   // Array of { ptr, templateId, attrPtr } from Hook 5
var _abilitySysLayout = null; // Cached AbilitySystem field layout from reflection
var _currentInitSystem = null; // Set in Hook 5 onEnter, cleared in onLeave — re-entrancy tracking
var _cachedGetName = null;    // Cached NativeFunction for il2cpp_class_get_name
var _gameplaySystemCount = 0; // Count of AbilitySystems with attrPtrs (for string dump gating)

var dataManagerPtr = null;  // DataManager singleton instance

var logFile = null;
var eventsFile = null;

// ══════════════════════════════════════════════════════════════
// Logging + Output
// ══════════════════════════════════════════════════════════════

function log(msg) {
    console.log("[char-data] " + msg);
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
    try {
        var base = Module.findBaseAddress(name);
        if (base) return { name: name, base: base, enumerateExports: function () { return Module.enumerateExports(name); } };
    } catch (e) {}
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
        var chars = p.add(STRING_CHARS_OFFSET);
        return chars.readUtf16String(len);
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
        return {
            value1: cacheAddr.readDouble(),
            value2: cacheAddr.add(8).readDouble(),
            value3: cacheAddr.add(16).readDouble(),
        };
    } catch (e) { return null; }
}

function readNullableAttrType(addr) {
    try {
        var hasValue = addr.add(4).readU8();
        if (!hasValue) return null;
        return addr.readS32();
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
                namedAttrs[ATTR_NAMES[i]] = rawData[i];
            }
        }
        return {
            rawData: rawData,
            namedAttrs: namedAttrs,
            finalCachedData: readAttributeDataCache(attrPtr.add(ATTR_OFFSETS.m_finalCachedData)),
            baseNonConvertedCache: readAttributeDataCache(attrPtr.add(ATTR_OFFSETS.m_baseNonConvertedCachedData)),
            finalNonConvertedCache: readAttributeDataCache(attrPtr.add(ATTR_OFFSETS.m_finalNonConvertedCachedData)),
            dataMin: readDoubleArray(attrPtr.add(ATTR_OFFSETS.m_dataMin).readPointer()),
            dataMax: readDoubleArray(attrPtr.add(ATTR_OFFSETS.m_dataMax).readPointer()),
            mainAttrType: readNullableAttrType(attrPtr.add(ATTR_OFFSETS.m_mainAttrType)),
            subAttrType: readNullableAttrType(attrPtr.add(ATTR_OFFSETS.m_subAttrType)),
        };
    } catch (e) { return null; }
}

function readConfigStatArray(arrayPtr) {
    if (!arrayPtr || arrayPtr.isNull()) return { initialized: false, error: "null" };
    try {
        var arrLen = arrayPtr.add(24).readS32();
        if (arrLen <= 0 || arrLen > 1000) return { initialized: false, error: "len=" + arrLen };

        var data = arrayPtr.add(IL2CPP_ARRAY_HEADER); // offset 32

        // Read marker at index 55 (if array is long enough)
        var marker = null;
        if (arrLen > 55) {
            marker = data.add(55 * 8).readDouble();
        }

        // Read stat region [56..102] — 47 entries mapped to ATTR_NAMES
        var stats = {};
        var rawStats = [];
        var nonZeroCount = 0;
        var statStart = Math.min(56, arrLen);
        var statEnd = Math.min(103, arrLen);
        for (var i = statStart; i < statEnd; i++) {
            var val = data.add(i * 8).readDouble();
            var idx = i - 56;
            rawStats.push(val);
            if (val !== 0.0 && isFinite(val) && idx < ATTR_NAMES.length) {
                stats[ATTR_NAMES[idx]] = val;
                nonZeroCount++;
            }
        }

        // Consider initialized if marker > 0 OR we have non-zero stat values
        var isInit = (marker !== null && isFinite(marker) && marker >= 1.0) || nonZeroCount > 3;

        return {
            initialized: isInit,
            arrLen: arrLen,
            marker: marker,
            attrCount: marker,
            nonZeroCount: nonZeroCount,
            stats: nonZeroCount > 0 ? stats : null,
            rawStats: rawStats.length > 0 ? rawStats : null,
        };
    } catch (e) {
        return { initialized: false, error: e.message };
    }
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
// Deep Object Probing — find char IDs and Attributes in unknown objects
// ══════════════════════════════════════════════════════════════

/**
 * Scan an object's pointer fields for an Il2CppString containing "chr_".
 * Returns { templateId, offset } or null.
 */
/**
 * Read IL2CPP class name directly from klass struct memory.
 * Tries il2cpp_class_get_name first, then fallback offsets 0x10 (name) and 0x48.
 */
function readClassName(objPtr) {
    if (!objPtr || objPtr.isNull()) return null;
    try {
        var klass = objPtr.readPointer();
        if (klass.isNull()) return null;

        // Method 1: cached API function
        if (_cachedGetName) {
            try {
                var namePtr = _cachedGetName(klass);
                if (!namePtr.isNull()) {
                    var name = namePtr.readCString();
                    if (name && name.length > 0 && name.length < 200) return name;
                }
            } catch (e) {}
        }

        // Method 2: direct struct offset — Il2CppClass.name is typically at +0x10 (x64)
        var directOffsets = [0x10, 0x48, 0x08];
        for (var di = 0; di < directOffsets.length; di++) {
            try {
                var np = klass.add(directOffsets[di]).readPointer();
                if (np.isNull()) continue;
                var s = np.readCString();
                // Validate: real class names are ASCII, 2-100 chars, no nulls
                if (s && s.length >= 2 && s.length < 100 && /^[A-Za-z_][A-Za-z0-9_.<>]*$/.test(s)) {
                    return s;
                }
            } catch (e) {}
        }
    } catch (e) {}
    return null;
}

/**
 * Read class name directly from an Il2CppClass* pointer (NOT from obj+0).
 * Il2CppClass layout:
 *   +0x00: Il2CppImage* image
 *   +0x08: void* gc_desc
 *   +0x10: const char* name
 *   +0x18: const char* namespaze
 */
function readKlassName(klassPtr) {
    if (!klassPtr || klassPtr.isNull()) return null;
    try {
        // Try cached API first
        if (_cachedGetName) {
            try {
                var namePtr = _cachedGetName(klassPtr);
                if (!namePtr.isNull()) {
                    var name = namePtr.readCString();
                    if (name && name.length > 0 && name.length < 200) {
                        // Also try namespace
                        var ns = "";
                        try {
                            var nsPtr = klassPtr.add(0x18).readPointer();
                            if (!nsPtr.isNull()) {
                                var nsStr = nsPtr.readCString();
                                if (nsStr && nsStr.length > 0 && nsStr.length < 200 && /^[A-Za-z_]/.test(nsStr)) {
                                    ns = nsStr;
                                }
                            }
                        } catch (e) {}
                        return ns ? (ns + "." + name) : name;
                    }
                }
            } catch (e) {}
        }

        // Fallback: direct struct read at +0x10
        var np = klassPtr.add(0x10).readPointer();
        if (np.isNull()) return null;
        var s = np.readCString();
        if (!s || s.length < 1 || s.length >= 200 || !/^[A-Za-z_][A-Za-z0-9_.<>`]*$/.test(s)) return null;

        // Namespace at +0x18
        var ns2 = "";
        try {
            var nsPtr2 = klassPtr.add(0x18).readPointer();
            if (!nsPtr2.isNull()) {
                var nsStr2 = nsPtr2.readCString();
                if (nsStr2 && nsStr2.length > 0 && nsStr2.length < 200 && /^[A-Za-z_]/.test(nsStr2)) {
                    ns2 = nsStr2;
                }
            }
        } catch (e) {}
        return ns2 ? (ns2 + "." + s) : s;
    } catch (e) { return null; }
}

/**
 * Return hex string of `size` bytes at `addr`. Max 512 bytes.
 * Groups by 8 bytes (qword) separated by spaces.
 */
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
            if ((i + 1) % 8 === 0 && i + 1 < arr.length) parts.push(" ");
        }
        return parts.join("");
    } catch (e) { return "(read error: " + e.message + ")"; }
}

/**
 * Probe a bare (non-managed) data pointer. Returns structured analysis.
 * Reads 256 bytes forward, 64 bytes backward (for IL2CPP array header),
 * interprets qwords as pointers/strings/doubles.
 */
function probeBarePtrBlock(dataPtr, label) {
    if (!dataPtr || dataPtr.isNull()) return null;

    var result = {
        label: label,
        ptr: dataPtr.toString(),
        hex: "",
        headerHex: "",
        pointerFields: [],
        int32Fields: [],
        doubleValues: [],
        stringFields: [],
    };

    // Read 256 bytes forward
    result.hex = hexDump(dataPtr, 256);

    // Read 64 bytes BEFORE dataPtr (check for IL2CPP array header)
    try {
        result.headerHex = hexDump(dataPtr.add(-64), 64);
    } catch (e) {
        result.headerHex = "(unreadable)";
    }

    // Check for IL2CPP array header at -32: klass at -32, length at -8
    try {
        var possibleKlass = dataPtr.add(-32).readPointer();
        if (!possibleKlass.isNull() && possibleKlass.compare(ptr(0x10000)) > 0 && possibleKlass.compare(ptr("0x7FFFFFFFFFFF")) < 0) {
            var klassName = readKlassName(possibleKlass);
            if (klassName) {
                result.arrayHeaderKlass = klassName;
                result.arrayHeaderKlassPtr = possibleKlass.toString();
            }
        }
        var possibleLen = dataPtr.add(-8).readS32();
        if (possibleLen > 0 && possibleLen < 100000) {
            result.arrayHeaderLen = possibleLen;
        }
    } catch (e) {}

    // Interpret each 8-byte qword (32 qwords = 256 bytes)
    for (var qi = 0; qi < 32; qi++) {
        var off = qi * 8;
        try {
            var qval = dataPtr.add(off).readPointer();
            // Check if it looks like a valid pointer
            if (!qval.isNull() && qval.compare(ptr(0x10000)) > 0 && qval.compare(ptr("0x7FFFFFFFFFFF")) < 0) {
                var pInfo = { offset: off, ptr: qval.toString() };

                // Try reading class name (object at qval)
                try {
                    var cn = readClassName(qval);
                    if (cn) pInfo.className = cn;
                } catch (e) {}

                // Try reading class name if qval IS a klass ptr
                try {
                    var kn = readKlassName(qval);
                    if (kn) pInfo.klassName = kn;
                } catch (e) {}

                // Try reading as IL2CPP string
                try {
                    var str = readIl2cppString(qval);
                    if (str && str.length > 0 && str.length < 500) {
                        pInfo.stringValue = str;
                        result.stringFields.push({ offset: off, value: str });
                    }
                } catch (e) {}

                // Try following the pointer and reading its first qword
                try {
                    var deref = qval.readPointer();
                    if (!deref.isNull() && deref.compare(ptr(0x10000)) > 0 && deref.compare(ptr("0x7FFFFFFFFFFF")) < 0) {
                        pInfo.derefPtr = deref.toString();
                        try {
                            var derefCn = readClassName(qval);
                            if (derefCn) pInfo.derefClassName = derefCn;
                        } catch (e) {}
                    }
                } catch (e) {}

                result.pointerFields.push(pInfo);
            }
        } catch (e) {}

        // Also read as int32 (first 4 bytes of each qword)
        try {
            var i32 = dataPtr.add(off).readS32();
            if (i32 !== 0 && Math.abs(i32) < 1000000) {
                result.int32Fields.push({ offset: off, value: i32 });
            }
        } catch (e) {}
    }

    // Try reading as Double[] (check for reasonable stat values)
    for (var di = 0; di < 32; di++) {
        try {
            var dval = dataPtr.add(di * 8).readDouble();
            if (isFinite(dval) && dval !== 0.0 && Math.abs(dval) < 100000 && Math.abs(dval) > 1e-10) {
                result.doubleValues.push({ offset: di * 8, index: di, value: dval });
            }
        } catch (e) {}
    }

    return result;
}

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
    } catch (e) {}
    return null;
}

/**
 * Scan an object's pointer fields for something that looks like an Attributes object.
 * Attributes has m_rawData (Double[]) at offset +96.
 * Returns { attrPtr, offset, rawDataLen } or null.
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

/**
 * Use IL2CPP reflection to enumerate all fields of an object's class.
 * Returns array of { name, offset, type, valueStr? } or null.
 */
function reflectClassFields(klass, objPtr) {
    try {
        var il2cpp_class_get_name = Module.findExportByName(null, "il2cpp_class_get_name");
        var il2cpp_class_get_fields = Module.findExportByName(null, "il2cpp_class_get_fields");
        var il2cpp_field_get_name = Module.findExportByName(null, "il2cpp_field_get_name");
        var il2cpp_field_get_offset = Module.findExportByName(null, "il2cpp_field_get_offset");
        var il2cpp_field_get_type = Module.findExportByName(null, "il2cpp_field_get_type");
        var il2cpp_type_get_name = Module.findExportByName(null, "il2cpp_type_get_name");

        if (!il2cpp_class_get_fields || !il2cpp_field_get_name) return null;

        var getFields = new NativeFunction(il2cpp_class_get_fields, "pointer", ["pointer", "pointer"]);
        var getFieldName = new NativeFunction(il2cpp_field_get_name, "pointer", ["pointer"]);
        var getFieldOffset = new NativeFunction(il2cpp_field_get_offset, "int", ["pointer"]);

        var getType = (il2cpp_field_get_type && il2cpp_type_get_name)
            ? { ft: new NativeFunction(il2cpp_field_get_type, "pointer", ["pointer"]),
                tn: new NativeFunction(il2cpp_type_get_name, "pointer", ["pointer"]) }
            : null;

        var className = il2cpp_class_get_name
            ? readCString(new NativeFunction(il2cpp_class_get_name, "pointer", ["pointer"])(klass))
            : "";

        var iter = Memory.alloc(Process.pointerSize);
        iter.writePointer(ptr(0));

        var fields = [];
        for (var fi = 0; fi < 80; fi++) {
            var field = getFields(klass, iter);
            if (field.isNull()) break;
            var fname = readCString(getFieldName(field));
            var foff = getFieldOffset(field);
            var ftype = "";
            if (getType) {
                var ft = getType.ft(field);
                if (!ft.isNull()) ftype = readCString(getType.tn(ft));
            }
            var info = { name: fname, offset: foff, type: ftype };

            // Try reading pointer value and string
            if (objPtr) {
                try {
                    var fptr = objPtr.add(foff).readPointer();
                    if (!fptr.isNull()) {
                        var str = readIl2cppString(fptr);
                        if (str) info.valueStr = str;
                    }
                } catch (e) {}
            }
            fields.push(info);
        }
        return { className: className, fields: fields };
    } catch (e) { return null; }
}

// ══════════════════════════════════════════════════════════════
// Type Probing — decode unknown arguments
// ══════════════════════════════════════════════════════════════

function probeArg(arg) {
    var result = { raw: arg.toString() };
    try { result.asInt32 = arg.toInt32(); } catch (e) {}
    try {
        var p = arg.readPointer ? arg : ptr(arg.toString());
        if (!p.isNull()) {
            var str = readIl2cppString(p);
            if (str) result.asString = str;
            try {
                var dblArr = readDoubleArray(p);
                if (dblArr) result.asDoubleArray = dblArr;
            } catch (e2) {}
            // Try reading first few bytes as hex for structure analysis
            try {
                var hexBytes = [];
                for (var hi = 0; hi < 32; hi++) {
                    hexBytes.push(("0" + p.add(hi).readU8().toString(16)).slice(-2));
                }
                result.hexDump = hexBytes.join(" ");
            } catch (e3) {}
        }
    } catch (e) {}
    return result;
}

function probeRetval(retval) {
    var result = { raw: retval.toString() };
    try { result.asInt32 = retval.toInt32(); } catch (e) {}
    try {
        var p = ptr(retval.toString());
        if (!p.isNull()) {
            var str = readIl2cppString(p);
            if (str) result.asString = str;
            try {
                var dblArr = readDoubleArray(p);
                if (dblArr) result.asDoubleArray = dblArr;
            } catch (e2) {}
            try {
                var hexBytes = [];
                for (var hi = 0; hi < 64; hi++) {
                    hexBytes.push(("0" + p.add(hi).readU8().toString(16)).slice(-2));
                }
                result.hexDump = hexBytes.join(" ");
            } catch (e3) {}
        }
    } catch (e) {}
    return result;
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
// Hooks (passive observation only)
// ══════════════════════════════════════════════════════════════

function captureScalars(attrPtr, templateId) {
    if (!scalarFns || !templateId || scalarsCaptured[templateId]) return null;
    var result = {};
    try {
        for (var name in scalarFns) {
            var val = scalarFns[name](attrPtr);
            result[name] = isFinite(val) ? val : null;
        }
        scalarsCaptured[templateId] = result;
        log("  Scalars captured for " + templateId + ": " + JSON.stringify(result));
    } catch (e) {
        log("  ERR captureScalars: " + e.message);
        return null;
    }
    return result;
}

// ══════════════════════════════════════════════════════════════
// Deep Object Graph Walking — v11
// ══════════════════════════════════════════════════════════════

/**
 * Recursively probe an object's pointer fields for IL2CPP class names and strings.
 * Returns tree: { offset, ptr, className, stringValue, children? }
 */
function probeObjectGraph(objPtr, label, maxDepth) {
    if (!objPtr || objPtr.isNull()) return null;
    var result = { label: label, ptr: objPtr.toString(), fields: [] };

    try {
        result.className = readClassName(objPtr);
    } catch (e) {}

    try {
        var bytes = objPtr.readByteArray(Math.min(512, 4096));
        if (!bytes) return result;
    } catch (e) { return result; }

    for (var off = 16; off < 504; off += 8) {
        try {
            var fieldPtr = objPtr.add(off).readPointer();
            if (fieldPtr.isNull()) continue;
            // Skip implausible pointers
            if (fieldPtr.compare(ptr(0x10000)) < 0) continue;
            if (fieldPtr.compare(ptr("0x7FFFFFFFFFFF")) > 0) continue;

            var info = { offset: off, ptr: fieldPtr.toString() };

            // Try reading as IL2CPP string
            try {
                var str = readIl2cppString(fieldPtr);
                if (str && str.length > 0 && str.length < 300) {
                    info.stringValue = str;
                }
            } catch (e) {}

            // Try reading class name of pointed-to object
            try {
                var cn = readClassName(fieldPtr);
                if (cn) info.className = cn;
            } catch (e) {}

            // Try reading as IL2CPP array (get length)
            try {
                var arrLen = fieldPtr.add(24).readS32();
                if (arrLen > 0 && arrLen < 100000) {
                    info.possibleArrayLen = arrLen;
                }
            } catch (e) {}

            // Try reading int32 value at offset (for inline value types)
            try {
                var i32val = objPtr.add(off).readS32();
                if (i32val !== 0 && Math.abs(i32val) < 100000) {
                    info.int32Value = i32val;
                }
            } catch (e) {}

            result.fields.push(info);

            // Recurse if depth allows
            if (maxDepth > 0 && info.className) {
                try {
                    var children = probeObjectGraph(fieldPtr, info.className + "@+" + off, maxDepth - 1);
                    if (children && children.fields && children.fields.length > 0) {
                        info.children = children.fields;
                    }
                } catch (e) {}
            }
        } catch (e) { continue; }
    }

    return result;
}

/**
 * Try to identify and walk an IL2CPP Dictionary<TKey, TValue>.
 *
 * IL2CPP Dictionary layout (after +0 klass, +8 gc):
 *   +16: _buckets  (Int32[] pointer)
 *   +24: _entries  (Entry[] pointer)
 *   +32: _count    (Int32)
 *   +36: _version  (Int32)
 *   +40: _freeList (Int32, typically -1)
 *   +44: _freeCount (Int32)
 *   +48: _comparer (IEqualityComparer pointer)
 *
 * Returns { isDictionary, count, entries: [{key, valuePtr, valueClassName}] } or null.
 */
function tryWalkDictionary(objPtr) {
    if (!objPtr || objPtr.isNull()) return null;
    try {
        // Validate _buckets at +16
        var bucketsPtr = objPtr.add(16).readPointer();
        if (bucketsPtr.isNull()) return null;
        try {
            var bucketsKlass = bucketsPtr.readPointer();
            if (bucketsKlass.isNull()) return null;
            var bucketsLen = bucketsPtr.add(24).readS32();
            if (bucketsLen <= 0 || bucketsLen > 100000) return null;
        } catch (e) { return null; }

        // Validate _entries at +24
        var entriesPtr = objPtr.add(24).readPointer();
        if (entriesPtr.isNull()) return null;
        try {
            var entriesKlass = entriesPtr.readPointer();
            if (entriesKlass.isNull()) return null;
        } catch (e) { return null; }
        var entriesLen = entriesPtr.add(24).readS32();
        if (entriesLen <= 0 || entriesLen > 100000) return null;

        // Validate _count at +32
        var count = objPtr.add(32).readS32();
        if (count <= 0 || count > 100000) return null;

        // Validate _freeList at +40 (typically -1 when no free slots)
        var freeList = objPtr.add(40).readS32();

        // All checks passed — this is likely a Dictionary
        var result = {
            isDictionary: true,
            count: count,
            entriesLen: entriesLen,
            freeList: freeList,
            entries: [],
        };

        // Try to walk entries. Entry data starts at entriesPtr + 32 (IL2CPP array header).
        // Try stride 24: {i32 hashCode, i32 next, ptr key, ptr value}
        // Also try stride 32: {i32 hashCode, i32 next, i32 pad, i32 pad, ptr key, ptr value}
        var entryBase = entriesPtr.add(32);
        var strides = [24, 32];
        var bestEntries = null;
        var bestStride = 0;

        for (var si = 0; si < strides.length; si++) {
            var stride = strides[si];
            var entries = [];
            var chrCount = 0;

            for (var ei = 0; ei < Math.min(count, 200); ei++) {
                var ep = entryBase.add(ei * stride);
                var entry = {};
                try {
                    entry.hashCode = ep.readS32();
                    entry.next = ep.add(4).readS32();

                    // Skip free slots
                    if (entry.hashCode === -1) continue;

                    // Try key as IL2CPP string at multiple offsets
                    var keyOffsets = (stride === 24) ? [8, 16] : [8, 16, 24];
                    for (var ki = 0; ki < keyOffsets.length; ki++) {
                        try {
                            var keyPtr = ep.add(keyOffsets[ki]).readPointer();
                            if (!keyPtr.isNull()) {
                                var keyStr = readIl2cppString(keyPtr);
                                if (keyStr && keyStr.length > 0 && keyStr.length < 300) {
                                    entry.key = keyStr;
                                    entry.keyOffset = keyOffsets[ki];
                                    if (keyStr.indexOf("chr_") === 0) chrCount++;
                                    break;
                                }
                            }
                        } catch (e) {}
                    }

                    // Try value as pointer (at offset after key)
                    var valueOffsets = (stride === 24) ? [16, 8] : [24, 16];
                    for (var vi = 0; vi < valueOffsets.length; vi++) {
                        // Skip the offset used for the key
                        if (entry.keyOffset === valueOffsets[vi]) continue;
                        try {
                            var valPtr = ep.add(valueOffsets[vi]).readPointer();
                            if (!valPtr.isNull()) {
                                entry.valuePtr = valPtr.toString();
                                entry.valueOffset = valueOffsets[vi];
                                try {
                                    var vcn = readClassName(valPtr);
                                    if (vcn) entry.valueClassName = vcn;
                                } catch (e) {}
                                break;
                            }
                        } catch (e) {}
                    }

                    entries.push(entry);
                } catch (e) { continue; }
            }

            // Pick stride that found "chr_" keys, or the one with more entries
            if (chrCount > 0 && (!bestEntries || chrCount > bestEntries.filter(function(e) { return e.key && e.key.indexOf("chr_") === 0; }).length)) {
                bestEntries = entries;
                bestStride = stride;
            } else if (!bestEntries || entries.length > bestEntries.length) {
                bestEntries = entries;
                bestStride = stride;
            }
        }

        result.entries = bestEntries || [];
        result.stride = bestStride;
        return result;
    } catch (e) { return null; }
}

/**
 * Probe a value object from a Dictionary entry for stat data.
 * Scans for IL2CPP Double[] arrays that could contain attribute data.
 */
function probeStatContainer(valuePtr) {
    if (!valuePtr || valuePtr.isNull()) return null;
    var result = {};

    try {
        result.className = readClassName(valuePtr);
    } catch (e) {}

    // Scan offsets 16-256 for Double[] arrays with length matching ATTR_NAMES range
    var statArrays = [];
    for (var off = 16; off <= 256; off += 8) {
        try {
            var fieldPtr = valuePtr.add(off).readPointer();
            if (fieldPtr.isNull()) continue;

            // Check if it's a valid IL2CPP array
            try {
                var klass = fieldPtr.readPointer();
                if (klass.isNull()) continue;
                var arrLen = fieldPtr.add(24).readS32();
                if (arrLen >= 40 && arrLen <= 200) {
                    // Try reading as Double[]
                    var doubles = readDoubleArray(fieldPtr);
                    if (doubles && doubles.length > 0) {
                        var namedStats = {};
                        var nonZero = 0;
                        for (var i = 0; i < doubles.length && i < ATTR_NAMES.length; i++) {
                            if (doubles[i] !== 0.0 && isFinite(doubles[i])) {
                                namedStats[ATTR_NAMES[i]] = doubles[i];
                                nonZero++;
                            }
                        }
                        if (nonZero > 0) {
                            statArrays.push({
                                offset: off,
                                arrayLen: arrLen,
                                nonZeroCount: nonZero,
                                stats: namedStats,
                                rawData: doubles,
                            });
                        }
                    }
                }
            } catch (e) {}
        } catch (e) {}
    }

    if (statArrays.length > 0) {
        result.statArrays = statArrays;
        // Use the array with the most non-zero values as primary
        statArrays.sort(function(a, b) { return b.nonZeroCount - a.nonZeroCount; });
        result.stats = statArrays[0].stats;
        result.rawData = statArrays[0].rawData;
    }

    // Also scan for sub-Dictionaries (nested structure)
    var subDicts = [];
    for (var dOff = 16; dOff <= 256; dOff += 8) {
        try {
            var dPtr = valuePtr.add(dOff).readPointer();
            if (dPtr.isNull()) continue;
            var subDict = tryWalkDictionary(dPtr);
            if (subDict && subDict.isDictionary && subDict.count > 0) {
                subDicts.push({ offset: dOff, count: subDict.count, entries: subDict.entries.slice(0, 5) });
            }
        } catch (e) {}
    }
    if (subDicts.length > 0) result.subDictionaries = subDicts;

    // Scan for string fields (type identifiers)
    var strings = [];
    for (var sOff = 16; sOff <= 128; sOff += 8) {
        try {
            var sPtr = valuePtr.add(sOff).readPointer();
            if (sPtr.isNull()) continue;
            var str = readIl2cppString(sPtr);
            if (str && str.length > 0 && str.length < 300) {
                strings.push({ offset: sOff, value: str });
            }
        } catch (e) {}
    }
    if (strings.length > 0) result.stringFields = strings;

    // Read numeric fields inline (int32/float at each 4-byte offset 16..64)
    var numerics = [];
    for (var nOff = 16; nOff <= 64; nOff += 4) {
        try {
            var ival = valuePtr.add(nOff).readS32();
            if (ival !== 0 && Math.abs(ival) < 100000 && ival !== -1) {
                numerics.push({ offset: nOff, int32: ival });
            }
        } catch (e) {}
    }
    if (numerics.length > 0) result.numericFields = numerics;

    return result;
}

function dumpCharacterAttributeTable() {
    if (!dataManagerPtr || !gaBase) {
        log("  dumpCharacterAttributeTable: no DataManager ptr or GA base");
        return;
    }

    log("  [v11] Deep characterAttributeTable walk + column recon...");

    try {
        var getTableFn = new NativeFunction(
            gaBase.add(RVA.DataManager_get_characterAttributeTable),
            "pointer", ["pointer"]
        );

        var tablePtr = getTableFn(dataManagerPtr);
        log("  characterAttributeTable ptr: " + tablePtr);

        if (tablePtr.isNull()) {
            log("  Table is NULL — configs may not be loaded yet");
            emitEvent({
                type: "CHARACTER_ATTRIBUTE_TABLE_DUMP",
                status: "null",
                dataManagerPtr: dataManagerPtr.toString(),
                timestamp: Date.now(),
            });
            return;
        }

        // 1. Deep object graph probe (depth 2)
        var graph = probeObjectGraph(tablePtr, "CharacterAttributeTable", 2);
        log("  Object graph: " + (graph ? graph.fields.length : 0) + " fields at depth 0");

        emitEvent({
            type: "TABLE_DEEP_PROBE",
            tablePtr: tablePtr.toString(),
            className: graph ? graph.className : null,
            graph: graph,
            timestamp: Date.now(),
        });

        // 2. Walk every pointer field at depth 0 looking for Dictionaries
        var dictFound = false;
        var totalCharsFound = 0;

        if (graph && graph.fields) {
            for (var fi = 0; fi < graph.fields.length; fi++) {
                var field = graph.fields[fi];
                if (!field.ptr) continue;

                var fieldPtr = ptr(field.ptr);
                var dict = tryWalkDictionary(fieldPtr);

                if (dict && dict.isDictionary && dict.count > 0) {
                    dictFound = true;
                    log("  Dictionary found at offset +" + field.offset +
                        " (" + (field.className || "?") + "): " + dict.count + " entries, stride=" + dict.stride);

                    // Walk ALL entries looking for character data
                    for (var dei = 0; dei < dict.entries.length; dei++) {
                        var dEntry = dict.entries[dei];
                        var isChrKey = dEntry.key && dEntry.key.indexOf("chr_") === 0;

                        // Probe value object for stats
                        var valueData = null;
                        if (dEntry.valuePtr) {
                            valueData = probeStatContainer(ptr(dEntry.valuePtr));
                        }

                        // Emit TABLE_CHARACTER_ENTRY for entries with "chr_" key or stat data
                        if (isChrKey || (valueData && valueData.stats)) {
                            totalCharsFound++;
                            var charEntry = {
                                type: "TABLE_CHARACTER_ENTRY",
                                charId: dEntry.key || null,
                                dictOffset: field.offset,
                                entryIndex: dei,
                                hashCode: dEntry.hashCode,
                                valuePtr: dEntry.valuePtr || null,
                                valueClassName: dEntry.valueClassName || null,
                                stats: (valueData && valueData.stats) ? valueData.stats : null,
                                rawData: (valueData && valueData.rawData) ? valueData.rawData : null,
                                statArrays: (valueData && valueData.statArrays) ? valueData.statArrays : null,
                                stringFields: (valueData && valueData.stringFields) ? valueData.stringFields : null,
                                numericFields: (valueData && valueData.numericFields) ? valueData.numericFields : null,
                                subDictionaries: (valueData && valueData.subDictionaries) ? valueData.subDictionaries : null,
                                source: "table_dict_walk",
                                timestamp: Date.now(),
                            };
                            emitEvent(charEntry);
                            if (isChrKey) {
                                log("    Entry " + dei + ": " + dEntry.key +
                                    " → " + (dEntry.valueClassName || "?") +
                                    " stats=" + (valueData && valueData.stats ? Object.keys(valueData.stats).length : 0));
                            }
                        }
                    }

                    // Also emit summary for this dictionary
                    emitEvent({
                        type: "TABLE_DICT_SUMMARY",
                        dictOffset: field.offset,
                        dictClassName: field.className || null,
                        count: dict.count,
                        entriesLen: dict.entriesLen,
                        stride: dict.stride,
                        freeList: dict.freeList,
                        sampleKeys: dict.entries.slice(0, 10).map(function(e) { return e.key || null; }),
                        chrEntryCount: dict.entries.filter(function(e) { return e.key && e.key.indexOf("chr_") === 0; }).length,
                        timestamp: Date.now(),
                    });
                }
            }
        }

        // 3. If no Dictionary at depth 0, try depth 1 (fields of fields)
        if (!dictFound && graph && graph.fields) {
            log("  No Dictionary at depth 0 — trying depth 1...");
            for (var fi2 = 0; fi2 < graph.fields.length; fi2++) {
                if (dictFound) break;
                var f2 = graph.fields[fi2];
                if (!f2.children) continue;

                for (var ci = 0; ci < f2.children.length; ci++) {
                    var child = f2.children[ci];
                    if (!child.ptr) continue;

                    var childDict = tryWalkDictionary(ptr(child.ptr));
                    if (childDict && childDict.isDictionary && childDict.count > 0) {
                        dictFound = true;
                        log("  Dictionary found at depth 1: +" + f2.offset + "→+" + child.offset +
                            " (" + (child.className || "?") + "): " + childDict.count + " entries");

                        for (var dei2 = 0; dei2 < childDict.entries.length; dei2++) {
                            var dEntry2 = childDict.entries[dei2];
                            var isChrKey2 = dEntry2.key && dEntry2.key.indexOf("chr_") === 0;
                            var valueData2 = null;
                            if (dEntry2.valuePtr) {
                                valueData2 = probeStatContainer(ptr(dEntry2.valuePtr));
                            }
                            if (isChrKey2 || (valueData2 && valueData2.stats)) {
                                totalCharsFound++;
                                emitEvent({
                                    type: "TABLE_CHARACTER_ENTRY",
                                    charId: dEntry2.key || null,
                                    dictOffset: f2.offset + "→" + child.offset,
                                    entryIndex: dei2,
                                    hashCode: dEntry2.hashCode,
                                    valuePtr: dEntry2.valuePtr || null,
                                    valueClassName: dEntry2.valueClassName || null,
                                    stats: (valueData2 && valueData2.stats) ? valueData2.stats : null,
                                    rawData: (valueData2 && valueData2.rawData) ? valueData2.rawData : null,
                                    stringFields: (valueData2 && valueData2.stringFields) ? valueData2.stringFields : null,
                                    numericFields: (valueData2 && valueData2.numericFields) ? valueData2.numericFields : null,
                                    source: "table_dict_walk_depth1",
                                    timestamp: Date.now(),
                                });
                            }
                        }

                        emitEvent({
                            type: "TABLE_DICT_SUMMARY",
                            dictOffset: f2.offset + "→" + child.offset,
                            dictClassName: child.className || null,
                            count: childDict.count,
                            entriesLen: childDict.entriesLen,
                            stride: childDict.stride,
                            sampleKeys: childDict.entries.slice(0, 10).map(function(e) { return e.key || null; }),
                            chrEntryCount: childDict.entries.filter(function(e) { return e.key && e.key.indexOf("chr_") === 0; }).length,
                            timestamp: Date.now(),
                        });
                        break;
                    }
                }
            }
        }

        if (totalCharsFound > 0) {
            log("  [v11] Table walk complete: " + totalCharsFound + " character entries found");
        } else if (dictFound) {
            log("  [v11] Dictionary found but no character entries — check TABLE_DEEP_PROBE for structure");
        } else {
            log("  [v11] No Dictionary found — see TABLE_DEEP_PROBE for raw object graph");
        }

        // ── v11: Column Reconnaissance ──
        // Scan for column descriptors at stride 48 from +96 to +480
        log("  [v11] Column descriptor reconnaissance...");
        var COLUMN_STRIDE = 48;
        var COLUMN_START = 96;
        var COLUMN_COUNT = 9;
        var lowestDataPtr = null;
        var highestDataPtr = null;

        for (var ci = 0; ci < COLUMN_COUNT; ci++) {
            var colBase = COLUMN_START + ci * COLUMN_STRIDE;
            try {
                var colKlassPtr = tablePtr.add(colBase).readPointer();
                var colDataPtr  = tablePtr.add(colBase + 16).readPointer();
                var colTypePtr  = tablePtr.add(colBase + 32).readPointer();

                // Read actual klass name using readKlassName (not readClassName!)
                var colKlassName = null;
                if (!colKlassPtr.isNull() && colKlassPtr.compare(ptr(0x10000)) > 0) {
                    colKlassName = readKlassName(colKlassPtr);
                }

                // Probe the bare data pointer
                var colProbe = null;
                if (!colDataPtr.isNull() && colDataPtr.compare(ptr(0x10000)) > 0) {
                    colProbe = probeBarePtrBlock(colDataPtr, "column_" + ci);

                    // Track lowest/highest data ptr for block header
                    if (!lowestDataPtr || colDataPtr.compare(lowestDataPtr) < 0) {
                        lowestDataPtr = colDataPtr;
                    }
                    if (!highestDataPtr || colDataPtr.compare(highestDataPtr) > 0) {
                        highestDataPtr = colDataPtr;
                    }
                }

                log("    Column " + ci + ": klass=" + (colKlassName || "?") +
                    " data=" + colDataPtr + " type=" + colTypePtr);

                emitEvent({
                    type: "TABLE_COLUMN_DUMP",
                    columnIndex: ci,
                    columnOffset: colBase,
                    klassPtr: colKlassPtr.toString(),
                    klassName: colKlassName,
                    dataPtr: colDataPtr.toString(),
                    typePtr: colTypePtr.toString(),
                    probe: colProbe,
                    timestamp: Date.now(),
                });
            } catch (e) {
                log("    Column " + ci + " ERROR: " + e.message);
            }
        }

        // Read contiguous block header (32 bytes before the lowest data ptr)
        if (lowestDataPtr) {
            try {
                var blockHeaderHex = hexDump(lowestDataPtr.add(-32), 32);
                var blockKlassPtr = null;
                var blockKlassName = null;
                var blockLen = null;

                try {
                    blockKlassPtr = lowestDataPtr.add(-32).readPointer();
                    if (!blockKlassPtr.isNull() && blockKlassPtr.compare(ptr(0x10000)) > 0) {
                        blockKlassName = readKlassName(blockKlassPtr);
                    }
                } catch (e) {}

                try {
                    blockLen = lowestDataPtr.add(-8).readS32();
                } catch (e) {}

                log("  [v11] Block header (32B before lowest dataPtr " + lowestDataPtr + "): " +
                    blockHeaderHex.substring(0, 80) + "...");
                log("  [v11] Block klass=" + (blockKlassName || "?") + " len=" + blockLen);

                emitEvent({
                    type: "TABLE_BLOCK_HEADER",
                    lowestDataPtr: lowestDataPtr.toString(),
                    highestDataPtr: highestDataPtr ? highestDataPtr.toString() : null,
                    headerHex: blockHeaderHex,
                    klassPtr: blockKlassPtr ? blockKlassPtr.toString() : null,
                    klassName: blockKlassName,
                    possibleArrayLen: blockLen,
                    timestamp: Date.now(),
                });
            } catch (e) {
                log("  [v11] Block header read error: " + e.message);
            }
        }

        // Legacy dump event for backward compatibility
        emitEvent({
            type: "CHARACTER_ATTRIBUTE_TABLE_DUMP",
            status: dictFound ? "dict_found" : "no_dict",
            dataManagerPtr: dataManagerPtr.toString(),
            tablePtr: tablePtr.toString(),
            className: graph ? graph.className : null,
            totalCharsFound: totalCharsFound,
            timestamp: Date.now(),
        });

    } catch (e) {
        log("  dumpCharacterAttributeTable ERROR: " + e.message + "\n" + e.stack);
        emitEvent({
            type: "CHARACTER_ATTRIBUTE_TABLE_DUMP",
            status: "error",
            error: e.message,
            stack: String(e.stack || ""),
            timestamp: Date.now(),
        });
    }
}

function installHooks() {
    var ga = findModule(GA_MODULE);
    if (!ga) { log("ERROR: " + GA_MODULE + " not found"); return; }

    log("Installing hooks on " + (ga.name || GA_MODULE) + " (base=" + ga.base + ")...");

    // Cache IL2CPP reflection NativeFunctions (created once, reused in all hooks)
    try {
        var _getNameExport = Module.findExportByName(null, "il2cpp_class_get_name");
        if (_getNameExport) {
            _cachedGetName = new NativeFunction(_getNameExport, "pointer", ["pointer"]);
            log("  [DIAG] Cached il2cpp_class_get_name @ " + _getNameExport);
            // Quick validation: try calling on a known class
            try {
                var testKlass = ga.base.readPointer(); // read klass at module base (junk but tests calling)
                log("  [DIAG] _cachedGetName validation: function created OK");
            } catch (ve) {
                log("  [DIAG] _cachedGetName validation error: " + ve.message);
            }
        } else {
            log("  [DIAG] il2cpp_class_get_name export NOT FOUND — className will be null");
        }
    } catch (e) {
        log("  [DIAG] Could not cache il2cpp_class_get_name: " + e.message);
    }

    // Create scalar getter NativeFunctions (Attributes* this → double)
    var scalarDefs = [
        ["atkFromMain", RVA.GetAtkFinalScalarFromMainAttribute],
        ["atkFromStr",  RVA.GetAtkFinalScalarFromStr],
        ["atkFromAgi",  RVA.GetAtkFinalScalarFromAgi],
        ["atkFromWisd", RVA.GetAtkFinalScalarFromWisd],
        ["atkFromWill", RVA.GetAtkFinalScalarFromWill],
        ["hpFromStr",   RVA.GetHpBaseAddFromStrAttribute],
        ["healFromWill", RVA.GetHealScalarFromWill],
        ["physResistFromAgi", RVA.GetPhysicalResistanceFromAgiAttribute],
        ["energyResistFromWisd", RVA.GetEnergyResistanceFromWisdAttribute],
    ];
    scalarFns = {};
    for (var si = 0; si < scalarDefs.length; si++) {
        var sName = scalarDefs[si][0];
        var sRva = scalarDefs[si][1];
        scalarFns[sName] = new NativeFunction(ga.base.add(sRva), "double", ["pointer"]);
        log("  Scalar fn: " + sName + " @ " + ga.base.add(sRva));
    }

    // ── Hook 1: CharInfo._RefreshCharAttribute ──────────────
    hookMethod(ga.base, RVA.CharInfo_RefreshCharAttribute, "CharInfo._RefreshCharAttribute", {
        onEnter: function (args) {
            this._charInfo = readCharInfo(args[0]);
            // Capture klass
            if (!charInfoKlass && this._charInfo && this._charInfo.templateId) {
                try {
                    var klass = args[0].readPointer();
                    if (!klass.isNull()) {
                        charInfoKlass = klass;
                        log("  Captured CharInfo klass (Hook 1): " + klass);
                    }
                } catch (e) {}
            }
        },
        onLeave: function () {
            if (!this._charInfo) return;
            emitEvent({
                type: "CHAR_ATTRIBUTE_REFRESH",
                charInfo: this._charInfo,
                timestamp: Date.now(),
            });
        },
    });

    // ── Hook 2: CharInfo.get_attributes ─────────────────────
    // NOTE: RVA 0x2668440 is a shared virtual slot (CharInfo, MissionSystem, etc.)
    // We capture ALL calls — 313 is manageable. Identify owner via probeForCharId.
    hookMethod(ga.base, RVA.CharInfo_get_attributes, "CharInfo.get_attributes", {
        onEnter: function (args) {
            this._skip = false;
            this._charInfoPtr = args[0];
            if (args[0].isNull()) { this._skip = true; return; }

            this._charInfo = null;
            this._callerKlass = null;

            // Method 1: readCharInfo — check offset 24 for templateId string
            var ci = readCharInfo(args[0]);
            if (ci && ci.templateId && ci.templateId.indexOf("chr_") === 0) {
                this._charInfo = ci;
                if (!charInfoKlass) {
                    try {
                        charInfoKlass = args[0].readPointer();
                        log("  Captured CharInfo klass (Hook 2): " + charInfoKlass);
                    } catch (e) {}
                }
            } else {
                // Method 2: broader scan for "chr_" string in any field
                var probe = probeForCharId(args[0]);
                if (probe) {
                    this._charInfo = {
                        ptr: args[0].toString(),
                        templateId: probe.templateId,
                        probeOffset: probe.offset,
                    };
                }
            }

            // Get caller class name for diagnostics (API + struct fallback)
            this._callerKlass = readClassName(args[0]);
        },
        onLeave: function (retval) {
            if (this._skip) return;
            // Diagnostic: log first 10 null-retval calls to identify non-CharInfo callers
            if (retval.isNull()) {
                if (eventCount["CharInfo.get_attributes"] <= 10) {
                    log("  get_attributes null retval — caller: " + (this._callerKlass || "?") +
                        " obj=" + this._charInfoPtr);
                }
                return;
            }
            if (this._charInfo) registerCharMapping(retval, this._charInfo);

            var attrs = readAttributesObject(retval);
            var tid = this._charInfo ? this._charInfo.templateId : null;

            // Capture scalar coefficients (once per character, lightweight calls)
            var scalars = captureScalars(retval, tid);

            // Track this attrPtr
            var attrKey = retval.toString();
            if (!seenAttrPtrs[attrKey]) {
                seenAttrPtrs[attrKey] = { firstSeen: Date.now(), count: 0, charInfo: this._charInfo };
            }
            seenAttrPtrs[attrKey].count++;

            var evt = {
                type: "CHAR_GET_ATTRIBUTES",
                charInfo: this._charInfo,
                callerClass: this._callerKlass,
                attrPtr: attrKey,
                rawData: attrs ? attrs.rawData : null,
                namedAttrs: attrs ? attrs.namedAttrs : null,
                mainAttrType: attrs ? attrs.mainAttrType : null,
                subAttrType: attrs ? attrs.subAttrType : null,
                timestamp: Date.now(),
            };
            if (scalars) evt.scalars = scalars;
            emitEvent(evt);
        },
    });

    // ── Hook 3: Attributes._RefreshAll ──────────────────────
    hookMethod(ga.base, RVA.Attributes_RefreshAll, "Attributes._RefreshAll", {
        onEnter: function (args) {
            this._attrPtr = args[0];
        },
        onLeave: function () {
            if (!this._attrPtr || this._attrPtr.isNull()) return;
            var after = readAttributesObject(this._attrPtr);
            var charInfo = lookupChar(this._attrPtr);

            emitEvent({
                type: "FINAL_ATTRIBUTES",
                charInfo: charInfo,
                attrPtr: this._attrPtr.toString(),
                rawData: after ? after.rawData : null,
                namedAttrs: after ? after.namedAttrs : null,
                finalCachedData: after ? after.finalCachedData : null,
                baseNonConvertedCache: after ? after.baseNonConvertedCache : null,
                finalNonConvertedCache: after ? after.finalNonConvertedCache : null,
                mainAttrType: after ? after.mainAttrType : null,
                subAttrType: after ? after.subAttrType : null,
                dataMin: after ? after.dataMin : null,
                dataMax: after ? after.dataMax : null,
                timestamp: Date.now(),
            });
        },
    });

    // ── Hook 4: Attributes.Dump ─────────────────────────────
    hookMethod(ga.base, RVA.Attributes_Dump, "Attributes.Dump", {
        onEnter: function (args) { this._attrPtr = args[0]; },
        onLeave: function () {
            var charInfo = lookupChar(this._attrPtr);
            var src = readAttributesObject(this._attrPtr);
            emitEvent({
                type: "ATTR_DUMP",
                charInfo: charInfo,
                attributes: src,
                timestamp: Date.now(),
            });
        },
    });

    // ── Hook 5: AbilitySystem.InitAttributes ────────────────
    // 24 fires = 24 playable characters. Deep probe to find Attributes* + templateId.
    // Uses re-entrancy tracking: GetValue fires DURING this call, so we link attrPtr→AbilitySystem.
    hookMethod(ga.base, RVA.AbilitySystem_InitAttributes, "AbilitySystem.InitAttributes", {
        onEnter: function (args) {
            this._asPtr = args[0];
            // Set re-entrancy flag — Hook 6 will see this and link new attrPtrs to us
            _currentInitSystem = { ptr: args[0].toString(), attrPtrs: [] };
        },
        onLeave: function () {
            if (!this._asPtr || this._asPtr.isNull()) return;

            // Collect attrPtrs found via re-entrancy tracking, then clear flag
            var reentrantAttrPtrs = _currentInitSystem ? _currentInitSystem.attrPtrs : [];
            _currentInitSystem = null;

            var result = { ptr: this._asPtr.toString() };

            // 0. Get actual class name via readClassName (API + struct fallback)
            var cn = readClassName(this._asPtr);
            if (cn) result.className = cn;

            // 1. Use re-entrancy-discovered attrPtrs (from Hook 6 firing during our call)
            if (reentrantAttrPtrs.length > 0) {
                result.attrPtrs = reentrantAttrPtrs;
                // Use the first one as the primary (most likely the character's own Attributes)
                result.attrPtr = reentrantAttrPtrs[0];
                _gameplaySystemCount++;
                log("  AbilitySystem re-entrancy found " + reentrantAttrPtrs.length + " attrPtrs (gameplay #" + _gameplaySystemCount + ")");
            }

            // 2. Probe for templateId (Il2CppString containing "chr_")
            var charProbe = probeForCharId(this._asPtr);
            if (charProbe) {
                result.templateId = charProbe.templateId;
                result.templateIdOffset = charProbe.offset;
            }

            // 3. Deep walk: follow pointer chains to find CharInfo (3 levels, wider range)
            if (!result.templateId) {
                for (var off = 16; off <= 256; off += 8) {
                    if (result.templateId) break;
                    try {
                        var childPtr = this._asPtr.add(off).readPointer();
                        if (childPtr.isNull()) continue;
                        // Level 2: check child fields
                        var deepProbe = probeForCharId(childPtr);
                        if (deepProbe) {
                            result.templateId = deepProbe.templateId;
                            result.templateIdDeepOffset = off + "→" + deepProbe.offset;
                            break;
                        }
                        // Level 3: follow child's pointer fields
                        for (var off2 = 16; off2 <= 128; off2 += 8) {
                            try {
                                var grandchild = childPtr.add(off2).readPointer();
                                if (grandchild.isNull()) continue;
                                var gp = probeForCharId(grandchild);
                                if (gp) {
                                    result.templateId = gp.templateId;
                                    result.templateIdDeepOffset = off + "→" + off2 + "→" + gp.offset;
                                    break;
                                }
                            } catch (e2) {}
                        }
                    } catch (e) {}
                }
            }

            // 4. Probe Attributes object for back-reference to CharInfo (owner)
            //    Walk: attrPtr → offset[8..128] → pointer → probeForCharId
            if (!result.templateId && reentrantAttrPtrs.length > 0) {
                var primaryAttr = ptr(reentrantAttrPtrs[0]);
                // Level 1: scan Attributes object itself for "chr_" string
                var attrCharProbe = probeForCharId(primaryAttr);
                if (attrCharProbe) {
                    result.templateId = attrCharProbe.templateId;
                    result.templateIdSource = "attrPtr→" + attrCharProbe.offset;
                } else {
                    // Level 2: follow Attributes pointer fields → check for CharInfo
                    for (var aOff = 8; aOff <= 128; aOff += 8) {
                        if (result.templateId) break;
                        try {
                            var ownerCandidate = primaryAttr.add(aOff).readPointer();
                            if (ownerCandidate.isNull()) continue;
                            // Check if this pointer's object has a "chr_" string
                            var ownerProbe = probeForCharId(ownerCandidate);
                            if (ownerProbe) {
                                result.templateId = ownerProbe.templateId;
                                result.templateIdSource = "attrPtr+" + aOff + "→" + ownerProbe.offset;
                                break;
                            }
                            // Level 3: follow one more hop
                            for (var aOff2 = 8; aOff2 <= 64; aOff2 += 8) {
                                try {
                                    var deep = ownerCandidate.add(aOff2).readPointer();
                                    if (deep.isNull()) continue;
                                    var deepProbe2 = probeForCharId(deep);
                                    if (deepProbe2) {
                                        result.templateId = deepProbe2.templateId;
                                        result.templateIdSource = "attrPtr+" + aOff + "→" + aOff2 + "→" + deepProbe2.offset;
                                        break;
                                    }
                                } catch (e3) {}
                            }
                        } catch (e2) {}
                    }
                }
                if (result.templateId) {
                    log("  Found templateId via Attributes backlink: " + result.templateId + " (" + result.templateIdSource + ")");
                }
            }

            // 5. Wide string dump on AbilitySystem (first 3 gameplay systems only, for exploration)
            if (!result.templateId && reentrantAttrPtrs.length > 0 && _gameplaySystemCount <= 3) {
                var foundStrings = [];
                // Scan AbilitySystem 0-2048 for Il2CppStrings (expanded range)
                for (var sOff = 0; sOff <= 2048; sOff += 8) {
                    try {
                        var sPtr = this._asPtr.add(sOff).readPointer();
                        if (sPtr.isNull()) continue;
                        var sStr = readIl2cppString(sPtr);
                        if (sStr && sStr.length > 2 && sStr.length < 200) {
                            foundStrings.push({ offset: sOff, value: sStr });
                        }
                    } catch (e) {}
                }
                if (foundStrings.length > 0) {
                    result.stringDump = foundStrings;
                    log("  AbilitySystem string dump (" + foundStrings.length + " strings): " +
                        foundStrings.map(function(s) { return s.offset + "=\"" + s.value + "\""; }).join(", "));
                }
                // Also scan Attributes object 0-256 for strings
                if (reentrantAttrPtrs.length > 0) {
                    var attrStrings = [];
                    var attrObj = ptr(reentrantAttrPtrs[0]);
                    for (var asOff = 0; asOff <= 256; asOff += 8) {
                        try {
                            var asPtr = attrObj.add(asOff).readPointer();
                            if (asPtr.isNull()) continue;
                            var asStr = readIl2cppString(asPtr);
                            if (asStr && asStr.length > 2 && asStr.length < 200) {
                                attrStrings.push({ offset: asOff, value: asStr });
                            }
                        } catch (e) {}
                    }
                    if (attrStrings.length > 0) {
                        result.attrStringDump = attrStrings;
                        log("  Attributes string dump (" + attrStrings.length + " strings): " +
                            attrStrings.map(function(s) { return s.offset + "=\"" + s.value + "\""; }).join(", "));
                    }
                }
            }

            // 6. IL2CPP field reflection (first call only, best-effort)
            if (!_abilitySysLayout) {
                try {
                    var klass = this._asPtr.readPointer();
                    _abilitySysLayout = reflectClassFields(klass, this._asPtr);
                    if (_abilitySysLayout) {
                        result.reflectionFields = _abilitySysLayout.fields;
                        if (!result.className) result.className = _abilitySysLayout.className;
                    }
                } catch (e) {}
            }

            // 7. Register mappings: link attrPtrs to templateId
            if (result.templateId && result.attrPtrs) {
                for (var api = 0; api < result.attrPtrs.length; api++) {
                    var ap = ptr(result.attrPtrs[api]);
                    registerCharMapping(ap, {
                        ptr: this._asPtr.toString(),
                        templateId: result.templateId,
                        source: "AbilitySystem.InitAttributes(reentrant)",
                    });
                    // Capture scalars for each attrPtr
                    captureScalars(ap, result.templateId);
                }
                log("  AbilitySystem → " + result.templateId + " via " + result.attrPtrs.length + " attrPtrs");
            } else if (result.templateId && result.attrPtr) {
                var ap2 = ptr(result.attrPtr);
                registerCharMapping(ap2, {
                    ptr: this._asPtr.toString(),
                    templateId: result.templateId,
                    source: "AbilitySystem.InitAttributes",
                });
                captureScalars(ap2, result.templateId);
                log("  AbilitySystem → " + result.templateId + " attrPtr=" + result.attrPtr);
            }

            abilitySystems.push(result);

            emitEvent({
                type: "ABILITY_INIT_DEEP",
                abilitySystem: result,
                timestamp: Date.now(),
            });
        },
    });

    // ── Hook 6: Attributes.GetValue ─────────────────────────
    // Uses NativeFunction + re-entrancy guard to capture double return (XMM0).
    // Tracks unique attrPtrs and emits ATTR_PTR_FIRST_SEEN on first encounter.
    // Batches GetValue results: emits per-attrPtr snapshot every BATCH_SIZE calls.
    var getValueFn = new NativeFunction(ga.base.add(RVA.Attributes_GetValue), "double", ["pointer", "int32"]);
    var _reentrant = false;
    var _getValueBatch = {};    // attrPtr → { attrs: {name: value}, count: N }
    var _getValueTotal = 0;
    var GETVALUE_BATCH_SIZE = 500; // Emit snapshot every N calls
    hookMethod(ga.base, RVA.Attributes_GetValue, "Attributes.GetValue", {
        onEnter: function (args) {
            if (_reentrant) { this._skip = true; return; }
            this._skip = false;
            this._attrPtr = args[0];
            this._attrType = args[1].toInt32();

            var attrKey = this._attrPtr.toString();

            // Track unique attrPtrs — emit first-seen event
            if (!seenAttrPtrs[attrKey]) {
                var charInfo = lookupChar(this._attrPtr);

                // Back-probe: scan Attributes object for owner with "chr_" string
                var backProbeId = null;
                var backProbeSource = null;
                if (!charInfo) {
                    // Direct: check Attributes fields for Il2CppString "chr_"
                    var directProbe = probeForCharId(this._attrPtr);
                    if (directProbe) {
                        backProbeId = directProbe.templateId;
                        backProbeSource = "attr→" + directProbe.offset;
                    } else {
                        // Walk Attributes→offset→child for "chr_"
                        for (var bOff = 8; bOff <= 128; bOff += 8) {
                            if (backProbeId) break;
                            try {
                                var bPtr = this._attrPtr.add(bOff).readPointer();
                                if (bPtr.isNull()) continue;
                                var bp = probeForCharId(bPtr);
                                if (bp) {
                                    backProbeId = bp.templateId;
                                    backProbeSource = "attr+" + bOff + "→" + bp.offset;
                                }
                            } catch (e) {}
                        }
                    }
                    if (backProbeId) {
                        // Register in charMap
                        registerCharMapping(this._attrPtr, {
                            templateId: backProbeId,
                            source: backProbeSource,
                        });
                        charInfo = { templateId: backProbeId };
                        log("  Back-probe found: " + backProbeId + " via " + backProbeSource);
                    }
                }

                seenAttrPtrs[attrKey] = { firstSeen: Date.now(), count: 0, charInfo: charInfo };

                // Re-entrancy tracking: if inside InitAttributes, link this attrPtr
                var duringInit = false;
                if (_currentInitSystem) {
                    _currentInitSystem.attrPtrs.push(attrKey);
                    duringInit = true;
                }

                // Read rawData snapshot on first sight
                var rawData = null;
                try {
                    var rawDataPtr = this._attrPtr.add(96).readPointer();
                    if (!rawDataPtr.isNull()) {
                        rawData = readDoubleArray(rawDataPtr);
                    }
                } catch (e) {}

                // Get Attributes object class name (API + struct fallback)
                var attrClassName = readClassName(this._attrPtr);

                emitEvent({
                    type: "ATTR_PTR_FIRST_SEEN",
                    attrPtr: attrKey,
                    templateId: charInfo ? charInfo.templateId : null,
                    backProbeSource: backProbeSource,
                    attrClassName: attrClassName,
                    duringInitAttributes: duringInit,
                    initSystemPtr: _currentInitSystem ? _currentInitSystem.ptr : null,
                    uniqueCount: Object.keys(seenAttrPtrs).length,
                    rawData: rawData,
                    timestamp: Date.now(),
                });
                log("  New attrPtr: " + attrKey +
                    " (total=" + Object.keys(seenAttrPtrs).length + ")" +
                    (duringInit ? " [during InitAttributes]" : "") +
                    (charInfo ? " char=" + charInfo.templateId : ""));
            }
            seenAttrPtrs[attrKey].count++;

            var value = null;
            _reentrant = true;
            try {
                value = getValueFn(args[0], this._attrType);
                if (!isFinite(value)) value = null;
            } catch (e) { value = null; }
            _reentrant = false;

            // Batch accumulate: store latest value per (attrPtr, attrName)
            var attrName = (this._attrType >= 0 && this._attrType < ATTR_NAMES.length)
                ? ATTR_NAMES[this._attrType] : "attr_" + this._attrType;

            if (!_getValueBatch[attrKey]) {
                _getValueBatch[attrKey] = { attrs: {}, count: 0 };
            }
            if (value !== null) {
                _getValueBatch[attrKey].attrs[attrName] = value;
            }
            _getValueBatch[attrKey].count++;
            _getValueTotal++;

            // Flush batch every N calls
            if (_getValueTotal % GETVALUE_BATCH_SIZE === 0) {
                for (var bKey in _getValueBatch) {
                    var batch = _getValueBatch[bKey];
                    if (batch.count === 0) continue;
                    var bCharInfo = lookupChar(ptr(bKey));
                    emitEvent({
                        type: "ATTR_VALUE_SNAPSHOT",
                        attrPtr: bKey,
                        attrs: batch.attrs,
                        callCount: batch.count,
                        templateId: bCharInfo ? bCharInfo.templateId : null,
                        timestamp: Date.now(),
                    });
                }
                _getValueBatch = {};
                log("  GetValue batch flush (" + _getValueTotal + " total calls)");
            }
        },
    });

    // ── Hook 7: GetCharacterAttributeDataFromTable ────────
    // Static method: 4 args — arg[0]=category, arg[1]=subIndex, arg[2]=Double[103] array, arg[3]=buffer/offset
    var _configLookupCount = 0;
    hookMethod(ga.base, RVA.GetCharacterAttributeDataFromTable, "GetCharacterAttributeDataFromTable", {
        onEnter: function (args) {
            this._category = args[0].toInt32();
            this._subIndex = args[1].toInt32();
            this._arrayPtr = args[2];
            this._arg3 = args[3].toInt32();
            _configLookupCount++;

            // Capture backtrace + stack probe for ALL calls to find character identity
            this._backtrace = null;
            this._stackCharId = null;
            try {
                var bt = Thread.backtrace(this.context, Backtracer.ACCURATE);
                this._backtrace = bt.map(function(r) {
                    var mod = Process.findModuleByAddress(r);
                    return mod ? (mod.name + "+0x" + r.sub(mod.base).toString(16)) : r.toString();
                });
            } catch (e) {}

            // Probe stack for "chr_" strings (caller might have CharInfo on stack)
            try {
                var sp = this.context.sp;
                for (var si = 0; si < 128; si++) {
                    try {
                        var stackVal = sp.add(si * 8).readPointer();
                        if (stackVal.isNull()) continue;
                        // Check if it's a pointer to a "chr_" Il2CppString
                        var sStr = readIl2cppString(stackVal);
                        if (sStr && sStr.indexOf("chr_") === 0) {
                            this._stackCharId = sStr;
                            break;
                        }
                        // Check if it's a pointer to an object with "chr_" string
                        var cp = probeForCharId(stackVal);
                        if (cp) {
                            this._stackCharId = cp.templateId;
                            break;
                        }
                    } catch (e) {}
                }
            } catch (e) {}
        },
        onLeave: function (retval) {
            var ret = retval.toInt32();
            // Read array AFTER the function has populated it
            var cs = readConfigStatArray(this._arrayPtr);
            log("  GetCharAttrDataFromTable cat=" + this._category +
                " init=" + cs.initialized +
                (cs.error ? " err=" + cs.error : "") +
                (cs.marker !== undefined ? " marker=" + cs.marker : "") +
                (cs.nonZeroCount ? " stats=" + cs.nonZeroCount : "") +
                " ret=0x" + (ret >>> 0).toString(16));

            var evt = {
                type: "CONFIG_TABLE_LOOKUP",
                category: this._category,
                subIndex: this._subIndex,
                arrayPtr: this._arrayPtr.toString(),
                arg3: this._arg3,
                initialized: cs.initialized,
                attrCount: cs.attrCount || null,
                stats: cs.stats || null,
                rawStats: cs.rawStats || null,
                retval: ret,
                timestamp: Date.now(),
            };
            // Include diagnostic fields
            if (cs.error) evt.error = cs.error;
            if (cs.marker !== undefined) evt.marker = cs.marker;
            if (this._backtrace) evt.backtrace = this._backtrace;
            if (this._stackCharId) {
                evt.stackCharId = this._stackCharId;
                log("  CONFIG_TABLE_LOOKUP found char on stack: " + this._stackCharId);
            }

            // For first 3 events, dump raw doubles from the array for analysis
            if (eventCount["GetCharacterAttributeDataFromTable"] <= 3) {
                try {
                    var p = this._arrayPtr;
                    var arrLen = p.add(24).readS32();
                    evt.arrLen = arrLen;
                    // Dump all 103 doubles (or however many exist)
                    var dumpCount = Math.min(arrLen > 0 ? arrLen : 0, 103);
                    if (dumpCount > 0) {
                        var allDoubles = [];
                        var base = p.add(IL2CPP_ARRAY_HEADER);
                        for (var di = 0; di < dumpCount; di++) {
                            allDoubles.push(base.add(di * 8).readDouble());
                        }
                        // Find non-zero indices
                        var nonZero = [];
                        for (var ni = 0; ni < allDoubles.length; ni++) {
                            if (allDoubles[ni] !== 0.0) {
                                nonZero.push({ idx: ni, val: allDoubles[ni] });
                            }
                        }
                        evt.debugNonZero = nonZero;
                        evt.debugTotalDoubles = dumpCount;
                    }
                } catch (e) {
                    evt.debugError = e.message;
                }
            }

            emitEvent(evt);
        },
    });

    // ── Hook 8: _CalculateBaseAttribute ──────────────────
    // Static method: 7 params (base, growth, level, ...)
    hookMethod(ga.base, RVA._CalculateBaseAttribute, "_CalculateBaseAttribute", {
        onEnter: function (args) {
            this._args = [];
            for (var ai = 0; ai < 7; ai++) {
                this._args.push(probeArg(args[ai]));
            }
        },
        onLeave: function (retval) {
            var ret = probeRetval(retval);
            emitEvent({
                type: "CALC_BASE_ATTRIBUTE",
                args: this._args,
                retval: ret,
                timestamp: Date.now(),
            });
        },
    });

    // ── Hook 9: _CalculateFinalAttribute ─────────────────
    // Static method: 11 params (base, modifiers, ...)
    hookMethod(ga.base, RVA._CalculateFinalAttribute, "_CalculateFinalAttribute", {
        onEnter: function (args) {
            this._args = [];
            for (var ai = 0; ai < 11; ai++) {
                this._args.push(probeArg(args[ai]));
            }
        },
        onLeave: function (retval) {
            var ret = probeRetval(retval);
            emitEvent({
                type: "CALC_FINAL_ATTRIBUTE",
                args: this._args,
                retval: ret,
                timestamp: Date.now(),
            });
        },
    });

    // ── Hook 10: DataManager.OnInit ──────────────────────
    // Instance method: captures DataManager singleton
    hookMethod(ga.base, RVA.DataManager_OnInit, "DataManager.OnInit", {
        onEnter: function (args) {
            dataManagerPtr = args[0];
            log("  *** DataManager instance captured (onEnter): " + dataManagerPtr);
        },
        onLeave: function () {
            log("  *** DataManager.OnInit completed (onLeave). Scheduling table dump...");

            // Primary dump after 5s (init should be complete by then)
            setTimeout(function () {
                try {
                    log("  Triggering characterAttributeTable dump (5s)...");
                    dumpCharacterAttributeTable();
                } catch (e) {
                    log("  TABLE DUMP ERROR (5s): " + e.message + "\n" + e.stack);
                    emitEvent({
                        type: "CHARACTER_ATTRIBUTE_TABLE_DUMP",
                        status: "error",
                        error: e.message,
                        stack: String(e.stack || ""),
                        timestamp: Date.now(),
                    });
                }
            }, 5000);

            // Retry at 15s in case configs load late
            setTimeout(function () {
                try {
                    log("  Triggering characterAttributeTable dump retry (15s)...");
                    dumpCharacterAttributeTable();
                } catch (e) {
                    log("  TABLE DUMP ERROR (15s): " + e.message + "\n" + e.stack);
                    emitEvent({
                        type: "CHARACTER_ATTRIBUTE_TABLE_DUMP",
                        status: "error",
                        error: e.message,
                        stack: String(e.stack || ""),
                        timestamp: Date.now(),
                    });
                }
            }, 15000);
        },
    });

    log("All 10 hooks installed.");
}

function hookMethod(base, rva, name, callbacks) {
    var addr = base.add(rva);
    eventCount[name] = 0;

    try {
        Interceptor.attach(addr, {
            onEnter: function (args) {
                eventCount[name]++;
                if (eventCount[name] > MAX_EVENTS) return;
                this._hookName = name;
                this._skipLeave = false;
                if (callbacks.onEnter) {
                    try { callbacks.onEnter.call(this, args); }
                    catch (e) { log("  ERR " + name + " onEnter: " + e.message); this._skipLeave = true; }
                }
            },
            onLeave: function (retval) {
                if (eventCount[name] > MAX_EVENTS || this._skipLeave) return;
                if (this._skip) return;
                if (callbacks.onLeave) {
                    try { callbacks.onLeave.call(this, retval); }
                    catch (e) { log("  ERR " + name + " onLeave: " + e.message); }
                }
                if (eventCount[name] <= 3) {
                    log("  " + name + " #" + eventCount[name] + " fired");
                }
            },
        });
        log("  Hooked: " + name + " @ " + addr);
    } catch (e) {
        log("  FAILED: " + name + ": " + e.message);
    }
}

// ══════════════════════════════════════════════════════════════
// Initialization
// ══════════════════════════════════════════════════════════════

var _initialized = false;

function openOutputFiles() {
    try {
        logFile = new File("il2cpp_char_data.log", "w");
        eventsFile = new File("il2cpp_char_events.jsonl", "w");
        log("Opened output files: il2cpp_char_data.log, il2cpp_char_events.jsonl");
    } catch (e) {
        console.log("[char-data] WARN: Could not open output files: " + e.message);
    }
}

function tryInit() {
    if (_initialized) return true;
    var ga = findModule(GA_MODULE);
    if (!ga) return false;

    log("Found " + (ga.name || GA_MODULE) + " @ " + ga.base + " (size=" + (ga.size || "?") + ")");
    gaBase = ga.base;
    installHooks();
    _initialized = true;

    log("");
    log("Passive capture active. Navigate character/combat screens to trigger events.");
    log("Output: il2cpp_char_events.jsonl + il2cpp_char_data.log");

    // Write summary periodically
    setInterval(writeSummary, 30000);

    return true;
}

function waitForModule() {
    if (tryInit()) {
        log("Module found at script start.");
        return;
    }
    var POLL_INTERVAL = 2000;
    var elapsed = 0;
    log("Polling for " + GA_MODULE + " (every " + (POLL_INTERVAL / 1000) + "s)...");

    var timer = setInterval(function () {
        elapsed += POLL_INTERVAL;
        if (tryInit()) {
            clearInterval(timer);
            log("Init complete after ~" + (elapsed / 1000) + "s.");
            return;
        }
        if (elapsed % 60000 === 0) {
            log("  Still waiting for " + GA_MODULE + " (" + (elapsed / 1000) + "s elapsed)...");
        }
    }, POLL_INTERVAL);
}

function writeSummary() {
    try {
        // Final flush of GetValue batch
        for (var bKey in _getValueBatch) {
            var batch = _getValueBatch[bKey];
            if (batch.count === 0) continue;
            var bCharInfo = lookupChar(ptr(bKey));
            emitEvent({
                type: "ATTR_VALUE_SNAPSHOT",
                attrPtr: bKey,
                attrs: batch.attrs,
                callCount: batch.count,
                templateId: bCharInfo ? bCharInfo.templateId : null,
                timestamp: Date.now(),
            });
        }
        _getValueBatch = {};

        var summary = {
            timestamp: Date.now(),
            processId: Process.id,
            arch: Process.arch,
            eventCounts: eventCount,
            charMap: charMap,
            scalarCoefficients: scalarsCaptured,
            seenAttrPtrs: seenAttrPtrs,
            abilitySystems: abilitySystems,
            getValueTotal: _getValueTotal,
            totalEvents: allEvents.length,
        };
        var f = new File("il2cpp_char_summary.json", "w");
        f.write(JSON.stringify(summary, null, 2));
        f.flush();
        f.close();
    } catch (e) {}
}

// ══════════════════════════════════════════════════════════════
// Boot
// ══════════════════════════════════════════════════════════════

openOutputFiles();
log("Passive Character Data Gadget v11 (bare data pointer recon + klass name read)");
log("Process: " + Process.id + " (" + Process.arch + ")");

try {
    var startupMods = Process.enumerateModules();
    log("Modules at startup: " + startupMods.length);
    for (var _mi = 0; _mi < startupMods.length; _mi++) {
        var _m = startupMods[_mi];
        var _n = _m.name.toLowerCase();
        if (_n.indexOf("game") >= 0 || _n.indexOf("unity") >= 0 ||
            _n.indexOf("il2cpp") >= 0 || _n.indexOf("assembly") >= 0 ||
            _n.indexOf("hgp") >= 0 || _m.size > 10 * 1024 * 1024) {
            log("  * " + _m.name + " @ " + _m.base + " (" + _m.size + " bytes)");
        }
    }
    var _probe = Module.findExportByName(null, "il2cpp_class_get_name");
    if (_probe) {
        var _mod = Process.findModuleByAddress(_probe);
        log("  IL2CPP export in: " + (_mod ? _mod.name : "?") + " @ " + _probe);
    } else {
        log("  IL2CPP exports not found at startup");
    }
} catch (e) {
    log("Module probe failed: " + e.message);
}

waitForModule();
