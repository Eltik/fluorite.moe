/**
 * One-Time Calibration Script for Offline Stat Pipeline
 *
 * Captures all scaling coefficients, modifier layers, and config table
 * parameters needed to compute character stats offline (without Frida).
 *
 * Run ONCE while the game is loaded with characters. Outputs calibration_data.json.
 *
 * Captures:
 *   1. All scalar getter results for every Attributes object encountered
 *   2. Raw + final cached data at current level (multi-level if user levels up)
 *   3. Modifier layer dump (8-layer WeaponAttributeModifier arrays)
 *   4. AttributesCalculator interception (config table inputs + formula params)
 *
 * Usage:
 *   frida -f "ArknightsEndfield.exe" -l frida_calibration.js --no-pause 2>&1 | tee calibration.log
 *   OR: Frida gadget mode with this script
 *
 * Then: cargo run --bin stat-engine -- --calibration output/frida/calibration_data.json
 */

"use strict";

var GA_MODULE = "GameAssembly.dll";
var STARTUP_DELAY = 30000;

// ── RVAs ──────────────────────────────────────────────────────

var RVA = {
    // Attributes instance methods (thiscall)
    Attributes_RefreshAll:         0x3B0A740,
    Attributes_GetValue:           0x3B05D60,
    GetFinalValue:                 0x3B05E30,

    // Scalar getters (0 extra params, return double)
    GetAtkFinalScalarFromMainAttribute:     0x3B06E30,
    GetAtkFinalScalarFromStr:               0x3B07090,
    GetAtkFinalScalarFromAgi:               0x3B07150,
    GetAtkFinalScalarFromWisd:              0x3B07210,
    GetAtkFinalScalarFromWill:              0x3B072D0,
    GetHpBaseAddFromStrAttribute:           0x3B06C80,
    GetHealScalarFromWill:                  0x3B07C30,
    GetPhysicalResistanceFromAgiAttribute:  0x3B07890,
    GetEnergyResistanceFromWisdAttribute:   0x3B07A60,

    // CharInfo
    CharInfo_get_attributes:       0x2668440,

    // AttributesCalculator (static methods)
    GetCharacterAttributeDataFromTable:  0x530D6C0,
    _CalculateBaseAttribute:             0x530E390,
    _CalculateFinalAttribute:            0x530E720,
};

// Field offsets
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

var IL2CPP_ARRAY_HEADER = 32;
var STRING_LENGTH_OFFSET = 16;
var STRING_CHARS_OFFSET  = 20;
var MAX_EVENTS = 200;

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

// ── State ─────────────────────────────────────────────────────

var gaBase = null;
var scalarFns = {};
var charMap = {};          // attrPtr -> charInfo
var calibrationData = {};  // templateId -> calibration record
var calcInterceptions = []; // AttributesCalculator calls
var eventCount = {};

// ── Helpers ───────────────────────────────────────────────────

function log(msg) { console.log("[calibration] " + msg); }

function findModule(name) {
    var mods = Process.enumerateModules();
    for (var i = 0; i < mods.length; i++) {
        if (mods[i].name.toLowerCase() === name.toLowerCase()) return mods[i];
    }
    return null;
}

function readCString(p) {
    if (p.isNull()) return "";
    try { return p.readCString(); } catch (e) { return ""; }
}

function readIl2cppString(p) {
    if (p.isNull()) return null;
    try {
        var klass = p.readPointer();
        if (klass.isNull()) return null;
        var len = p.add(STRING_LENGTH_OFFSET).readS32();
        if (len <= 0 || len > 10000) return null;
        return p.add(STRING_CHARS_OFFSET).readUtf16String(len);
    } catch (e) { return null; }
}

function readDoubleArray(arrayPtr) {
    if (arrayPtr.isNull()) return null;
    try {
        var maxLen = arrayPtr.add(24).readU64().toNumber();
        if (maxLen <= 0 || maxLen > 256) return null;
        var result = [];
        var dataStart = arrayPtr.add(IL2CPP_ARRAY_HEADER);
        for (var i = 0; i < maxLen; i++) {
            result.push(dataStart.add(i * 8).readDouble());
        }
        return result;
    } catch (e) { return null; }
}

function readAttributeDataCache(cacheAddr) {
    try {
        var result = {};
        var dataPtr = cacheAddr.readPointer();
        if (!dataPtr.isNull()) {
            var data = readDoubleArray(dataPtr);
            if (data) {
                result.data = data;
                var named = {};
                for (var i = 0; i < data.length && i < ATTR_NAMES.length; i++) {
                    if (data[i] !== 0.0) named[ATTR_NAMES[i]] = data[i];
                }
                result.namedAttrs = named;
            }
        }
        return result;
    } catch (e) { return { error: e.message }; }
}

function readNullableAttrType(addr) {
    try {
        return { value: addr.readS32(), hasValue: addr.add(4).readU8() !== 0 };
    } catch (e) { return null; }
}

function readCharInfo(ptr) {
    if (ptr.isNull()) return null;
    try {
        var result = { ptr: ptr.toString() };
        for (var off = 16; off <= 128; off += 8) {
            try {
                var fieldPtr = ptr.add(off).readPointer();
                if (!fieldPtr.isNull()) {
                    var str = readIl2cppString(fieldPtr);
                    if (str && str.indexOf("chr_") === 0) {
                        result.templateId = str;
                    }
                }
            } catch (e) { /* skip */ }
        }
        try { result.level = ptr.add(0x30).readS32(); } catch (e) { /* skip */ }
        try { result.breakStage = ptr.add(0x34).readS32(); } catch (e) { /* skip */ }
        return result;
    } catch (e) { return null; }
}

// ── Core Calibration Logic ────────────────────────────────────

/**
 * Full calibration capture for one Attributes object.
 * Reads all data layers, calls all scalar methods, probes modifiers.
 */
function captureFullCalibration(attrPtr, charInfo) {
    var templateId = charInfo ? charInfo.templateId : "unknown_" + attrPtr.toString();
    if (!templateId || templateId === "(null)") return;

    var record = calibrationData[templateId] || {
        templateId: templateId,
        snapshots: [],
    };

    var snapshot = {
        timestamp: Date.now(),
        level: charInfo ? charInfo.level : null,
        breakStage: charInfo ? charInfo.breakStage : null,
        attrPtr: attrPtr.toString(),
    };

    // 1. Raw data
    try {
        var rawPtr = attrPtr.add(ATTR_OFFSETS.m_rawData).readPointer();
        snapshot.rawData = readDoubleArray(rawPtr);
    } catch (e) { snapshot.rawData = null; }

    // 2. Final cached data
    snapshot.finalCachedData = readAttributeDataCache(attrPtr.add(ATTR_OFFSETS.m_finalCachedData));

    // 3. Base non-converted cache
    snapshot.baseNonConverted = readAttributeDataCache(attrPtr.add(ATTR_OFFSETS.m_baseNonConvertedCachedData));

    // 4. Final non-converted cache
    snapshot.finalNonConverted = readAttributeDataCache(attrPtr.add(ATTR_OFFSETS.m_finalNonConvertedCachedData));

    // 5. Data min/max
    try {
        var minPtr = attrPtr.add(ATTR_OFFSETS.m_dataMin).readPointer();
        snapshot.dataMin = readDoubleArray(minPtr);
    } catch (e) { /* skip */ }
    try {
        var maxPtr = attrPtr.add(ATTR_OFFSETS.m_dataMax).readPointer();
        snapshot.dataMax = readDoubleArray(maxPtr);
    } catch (e) { /* skip */ }

    // 6. Main/sub attr types
    snapshot.mainAttrType = readNullableAttrType(attrPtr.add(ATTR_OFFSETS.m_mainAttrType));
    snapshot.subAttrType = readNullableAttrType(attrPtr.add(ATTR_OFFSETS.m_subAttrType));

    // 7. All scalar getters
    snapshot.scalars = {};
    for (var key in scalarFns) {
        if (!scalarFns.hasOwnProperty(key)) continue;
        try {
            var val = scalarFns[key](attrPtr);
            if (!isNaN(val) && isFinite(val)) {
                snapshot.scalars[key] = val;
            }
        } catch (e) {
            snapshot.scalars[key + "_error"] = e.message;
        }
    }

    // 8. Probe modifier list (m_attributeModifiers at offset +16, likely a HashSet[] or List[])
    snapshot.modifiers = probeModifiers(attrPtr);

    record.snapshots.push(snapshot);
    calibrationData[templateId] = record;

    log("  Calibrated: " + templateId +
        " (lv" + (charInfo ? charInfo.level : "?") +
        ", snapshot #" + record.snapshots.length + ")");
}

/**
 * Probe the modifier arrays attached to an Attributes object.
 * These are at offset +16 (approximate — may be a managed array/list).
 * Each modifier has 8 Double[] layers.
 */
function probeModifiers(attrPtr) {
    var modifiers = [];

    try {
        // Offset +16 is likely the modifier collection
        // In IL2CPP this could be: IAttributeModifier[] or HashSet<IAttributeModifier>[]
        var modFieldPtr = attrPtr.add(16).readPointer();
        if (modFieldPtr.isNull()) return modifiers;

        // Try reading as IL2CPP array (Array of objects)
        var maxLen = modFieldPtr.add(24).readU64().toNumber();
        if (maxLen <= 0 || maxLen > 100) return modifiers;

        var dataStart = modFieldPtr.add(IL2CPP_ARRAY_HEADER);

        for (var i = 0; i < maxLen; i++) {
            try {
                var modPtr = dataStart.add(i * 8).readPointer();
                if (modPtr.isNull()) continue;

                var mod = { index: i };

                // WeaponAttributeModifier has 8 Double[] fields
                // Offsets: 16, 24, 32, 40, 48, 56, 64, 72 (each is a pointer to Double[])
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
                                // Only include if any non-zero values
                                var hasNonZero = arr.some(function(v) { return v !== 0; });
                                if (hasNonZero) {
                                    mod[layerNames[j]] = arr;
                                }
                            }
                        }
                    } catch (e) { /* skip layer */ }
                }

                // Only include if modifier has any data
                if (Object.keys(mod).length > 1) {
                    modifiers.push(mod);
                }
            } catch (e) { /* skip modifier */ }
        }
    } catch (e) {
        // modifiers field is not a simple array
    }

    return modifiers;
}

// ── Hook Installation ─────────────────────────────────────────

function installCalibrationHooks() {
    var ga = findModule(GA_MODULE);
    if (!ga) { log("ERROR: " + GA_MODULE + " not found"); return; }
    gaBase = ga.base;

    // Resolve scalar NativeFunctions
    var scalarRvas = {
        "atkFromMain":      RVA.GetAtkFinalScalarFromMainAttribute,
        "atkFromStr":       RVA.GetAtkFinalScalarFromStr,
        "atkFromAgi":       RVA.GetAtkFinalScalarFromAgi,
        "atkFromWisd":      RVA.GetAtkFinalScalarFromWisd,
        "atkFromWill":      RVA.GetAtkFinalScalarFromWill,
        "hpFromStr":        RVA.GetHpBaseAddFromStrAttribute,
        "healFromWill":     RVA.GetHealScalarFromWill,
        "physResistFromAgi":  RVA.GetPhysicalResistanceFromAgiAttribute,
        "energyResistFromWisd": RVA.GetEnergyResistanceFromWisdAttribute,
    };

    for (var key in scalarRvas) {
        if (!scalarRvas.hasOwnProperty(key)) continue;
        try {
            scalarFns[key] = new NativeFunction(gaBase.add(scalarRvas[key]), "double", ["pointer"]);
        } catch (e) { log("  WARN: scalar " + key + ": " + e.message); }
    }

    log("Resolved " + Object.keys(scalarFns).length + " scalar functions");

    // Hook 1: CharInfo.get_attributes — map Attributes -> character
    hookSimple(gaBase, RVA.CharInfo_get_attributes, "CharInfo.get_attributes", {
        onEnter: function (args) {
            this._charInfo = readCharInfo(args[0]);
        },
        onLeave: function () {
            // retval not used — we just need the charInfo mapping
            // The actual Attributes ptr comes from _RefreshAll
        },
    });

    // Hook 2: Attributes._RefreshAll — main calibration point
    hookSimple(gaBase, RVA.Attributes_RefreshAll, "Attributes._RefreshAll", {
        onEnter: function (args) {
            this._attrPtr = args[0];
        },
        onLeave: function () {
            if (!this._attrPtr || this._attrPtr.isNull()) return;
            var charInfo = charMap[this._attrPtr.toString()] || null;
            captureFullCalibration(this._attrPtr, charInfo);
        },
    });

    // Hook 3: CharInfo.get_attributes — capture charMap on return
    hookSimple(gaBase, RVA.CharInfo_get_attributes, "CharInfo.get_attributes.map", {
        onEnter: function (args) {
            this._charInfoPtr = args[0];
            this._charInfo = readCharInfo(args[0]);
        },
        onLeave: function (retval) {
            if (retval && !retval.isNull() && this._charInfo && this._charInfo.templateId) {
                charMap[retval.toString()] = this._charInfo;
            }
        },
    });

    // Hook 4: AttributesCalculator.GetCharacterAttributeDataFromTable (static, 4 params)
    try {
        var calcAddr = gaBase.add(RVA.GetCharacterAttributeDataFromTable);
        eventCount["GetCharAttrFromTable"] = 0;
        Interceptor.attach(calcAddr, {
            onEnter: function (args) {
                eventCount["GetCharAttrFromTable"]++;
                if (eventCount["GetCharAttrFromTable"] > MAX_EVENTS) return;

                this._args = [];
                for (var i = 0; i < 4; i++) {
                    this._args.push(args[i].toString());
                }
            },
            onLeave: function (retval) {
                if (eventCount["GetCharAttrFromTable"] > MAX_EVENTS) return;

                calcInterceptions.push({
                    method: "GetCharacterAttributeDataFromTable",
                    args: this._args,
                    retval: retval ? retval.toString() : null,
                    timestamp: Date.now(),
                });
            },
        });
        log("  Hooked: GetCharacterAttributeDataFromTable @ " + calcAddr);
    } catch (e) {
        log("  FAILED: GetCharacterAttributeDataFromTable: " + e.message);
    }

    // Hook 5: AttributesCalculator._CalculateFinalAttribute (static, 11 params)
    try {
        var calcFinalAddr = gaBase.add(RVA._CalculateFinalAttribute);
        eventCount["CalcFinalAttr"] = 0;
        Interceptor.attach(calcFinalAddr, {
            onEnter: function (args) {
                eventCount["CalcFinalAttr"]++;
                if (eventCount["CalcFinalAttr"] > MAX_EVENTS) return;

                this._args = [];
                for (var i = 0; i < 11; i++) {
                    this._args.push(args[i].toString());
                }
            },
            onLeave: function (retval) {
                if (eventCount["CalcFinalAttr"] > MAX_EVENTS) return;

                calcInterceptions.push({
                    method: "_CalculateFinalAttribute",
                    args: this._args,
                    retval: retval ? retval.toString() : null,
                    timestamp: Date.now(),
                });
            },
        });
        log("  Hooked: _CalculateFinalAttribute @ " + calcFinalAddr);
    } catch (e) {
        log("  FAILED: _CalculateFinalAttribute: " + e.message);
    }

    // Hook 6: AttributesCalculator._CalculateBaseAttribute (static, 7 params)
    try {
        var calcBaseAddr = gaBase.add(RVA._CalculateBaseAttribute);
        eventCount["CalcBaseAttr"] = 0;
        Interceptor.attach(calcBaseAddr, {
            onEnter: function (args) {
                eventCount["CalcBaseAttr"]++;
                if (eventCount["CalcBaseAttr"] > MAX_EVENTS) return;

                this._args = [];
                for (var i = 0; i < 7; i++) {
                    this._args.push(args[i].toString());
                }
            },
            onLeave: function (retval) {
                if (eventCount["CalcBaseAttr"] > MAX_EVENTS) return;

                calcInterceptions.push({
                    method: "_CalculateBaseAttribute",
                    args: this._args,
                    retval: retval ? retval.toString() : null,
                    timestamp: Date.now(),
                });
            },
        });
        log("  Hooked: _CalculateBaseAttribute @ " + calcBaseAddr);
    } catch (e) {
        log("  FAILED: _CalculateBaseAttribute: " + e.message);
    }

    log("Calibration hooks installed.");
}

function hookSimple(base, rva, name, callbacks) {
    var addr = base.add(rva);
    eventCount[name] = 0;
    try {
        Interceptor.attach(addr, {
            onEnter: function (args) {
                eventCount[name]++;
                if (eventCount[name] > MAX_EVENTS) return;
                if (callbacks.onEnter) {
                    try { callbacks.onEnter.call(this, args); }
                    catch (e) { /* skip */ }
                }
            },
            onLeave: function (retval) {
                if (eventCount[name] > MAX_EVENTS) return;
                if (callbacks.onLeave) {
                    try { callbacks.onLeave.call(this, retval); }
                    catch (e) { /* skip */ }
                }
            },
        });
        log("  Hooked: " + name + " @ " + addr);
    } catch (e) {
        log("  FAILED: " + name + ": " + e.message);
    }
}

// ── Export / Summary ──────────────────────────────────────────

/**
 * Call this from REPL when done capturing: exportCalibration()
 * Emits the full calibration dataset as an IL2CPP_EVENT.
 */
globalThis.exportCalibration = function () {
    var output = {
        version: 1,
        timestamp: new Date().toISOString(),
        characters: calibrationData,
        calculatorInterceptions: calcInterceptions,
        eventCounts: eventCount,
        charMap: charMap,
    };

    console.log("CALIBRATION_DATA|" + JSON.stringify(output));
    log("Exported calibration data: " +
        Object.keys(calibrationData).length + " characters, " +
        calcInterceptions.length + " calculator calls");
    return output;
};

globalThis.getCalibration = function () { return calibrationData; };
globalThis.getCalcCalls = function () { return calcInterceptions; };
globalThis.getCharMap = function () { return charMap; };

// ── Init ──────────────────────────────────────────────────────

log("Calibration script initializing...");
log("Process: " + Process.id + " (" + Process.arch + ")");
log("Waiting " + (STARTUP_DELAY / 1000) + "s for game init...");

setTimeout(function () {
    installCalibrationHooks();

    log("");
    log("Calibration capture active.");
    log("Navigate ALL character screens to capture each character's stats.");
    log("For multi-level calibration, level up characters during capture.");
    log("");
    log("When done, call: exportCalibration()");
    log("Or press Ctrl+C — data is in the log as IL2CPP_EVENT lines.");
}, STARTUP_DELAY);
