/*
 * memscan.c — External read-only memory scanner for IL2CPP game data
 *
 * Runs as a standalone EXE alongside the game. Uses OpenProcess with
 * PROCESS_VM_READ only — no injection, no writes, no thread creation.
 *
 * Build (cross-compile from macOS/Linux):
 *   x86_64-w64-mingw32-gcc -O2 -s -o memscan.exe memscan.c -lkernel32 -lpsapi
 *
 * Usage:
 *   memscan.exe [--pid <PID>] [--output <dir>]
 *
 * If --pid not specified, searches for ArknightsEndfield.exe automatically.
 *
 * Outputs:
 *   il2cpp_char_events.jsonl  — compatible with char_data_probe.rs
 *   memscan_summary.json      — scan summary
 */

#define WIN32_LEAN_AND_MEAN
#define _CRT_SECURE_NO_WARNINGS
#include <windows.h>
#include <psapi.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ── IL2CPP Layout Constants ──────────────────────────────────────────── */

#define CI_TEMPLATE_ID   0x18  /* +24: Il2CppString* templateId */
#define CI_LEVEL         0x44  /* +68: int level */
#define CI_BREAK_STAGE   0x4C  /* +76: int breakStage */
#define CI_POTENTIAL     0x60  /* +96: int potentialLevel */
#define CI_ATTRS         0xC8  /* +200: Attributes* */
#define CI_ATTRS_PACK    0xB8  /* +184: attributesPack (fallback) */
#define AT_RAW_DATA      0x60  /* +96: Attributes.m_rawData */

/* Il2CppString: [klass 8][monitor 8][length_i32 4][utf16_chars...] */
#define STR_LEN_OFF   16
#define STR_DATA_OFF  20

/* Il2CppArray<double>: [klass 8][monitor 8][bounds 8][max_length_i32 4][pad 4][doubles...] */
#define ARR_MAXLEN_OFF  24
#define ARR_DATA_OFF    32

#define MAX_CHARS       64
#define MAX_RAW_DATA    128
#define MAX_REGIONS     65536

/* ── Attribute Names ─────────────────────────────────────────────────── */

static const char *ATTR_NAMES[] = {
    "hp", "str", "agi", "wisd", "will",
    "atk", "def", "maxHp",
    "maxUltimateSp", "maxPoise", "poiseRecTime", "poiseRecVal",
    "staminaMax", "staminaRecTime", "energyMax", "energyRecSpeed",
    "criticalRate", "criticalDamage", "attackSpeed", "attackRate",
    "skillCooldownScalar",
    "unknown_21", "unknown_22", "unknown_23",
    "normalAttackRange", "hpRecoveryPerSec",
    "physDmgIncrease", "physDmgResist",
    "igniteDmgIncrease", "igniteDmgResist",
    "freezeDmgIncrease", "freezeDmgResist",
    "voltageDmgIncrease", "voltageDmgResist",
    "erosionDmgIncrease", "erosionDmgResist",
    "binaryDmgIncrease", "binaryDmgResist",
    "physPenetrate", "elementPenetrate",
    "unknown_40", "unknown_41", "unknown_42", "unknown_43",
    "healIncrease", "healReceive",
    "shieldIncrease",
    "etherDmgIncrease",
    "unknown_48", "unknown_49",
};
#define NUM_ATTR_NAMES (sizeof(ATTR_NAMES) / sizeof(ATTR_NAMES[0]))

/* ── Character Data ──────────────────────────────────────────────────── */

typedef struct {
    char template_id[32];
    int level;
    int break_stage;
    int potential_level;
    double raw_data[MAX_RAW_DATA];
    int raw_data_count;
    unsigned long long charinfo_va;
    unsigned long long attrs_va;
} CharData;

static CharData g_chars[MAX_CHARS];
static int g_char_count = 0;

/* ── Memory Region Cache ─────────────────────────────────────────────── */

typedef struct {
    unsigned long long base;
    unsigned long long size;
} MemRegion;

static MemRegion g_regions[MAX_REGIONS];
static int g_region_count = 0;

/* ── Process Helpers ─────────────────────────────────────────────────── */

static DWORD find_game_pid(void) {
    DWORD pids[4096], needed;
    if (!EnumProcesses(pids, sizeof(pids), &needed))
        return 0;

    DWORD count = needed / sizeof(DWORD);
    for (DWORD i = 0; i < count; i++) {
        if (pids[i] == 0) continue;
        HANDLE h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ,
                               FALSE, pids[i]);
        if (!h) continue;

        char name[MAX_PATH] = {0};
        HMODULE mod;
        DWORD cb;
        if (EnumProcessModules(h, &mod, sizeof(mod), &cb)) {
            GetModuleBaseNameA(h, mod, name, sizeof(name));
        }
        CloseHandle(h);

        if (_stricmp(name, "ArknightsEndfield.exe") == 0)
            return pids[i];
    }
    return 0;
}

/* Read from remote process. Returns 1 on success. */
static int rpm(HANDLE proc, unsigned long long addr, void *buf, size_t len) {
    SIZE_T read;
    return ReadProcessMemory(proc, (LPCVOID)addr, buf, len, &read) && read == len;
}

static int rpm_u64(HANDLE proc, unsigned long long addr, unsigned long long *out) {
    return rpm(proc, addr, out, 8);
}

static int rpm_i32(HANDLE proc, unsigned long long addr, int *out) {
    return rpm(proc, addr, out, 4);
}

/* Read Il2CppString at ptr → ASCII output. Returns string length or 0. */
static int read_il2cpp_string(HANDLE proc, unsigned long long ptr,
                               char *out, int max_len) {
    int slen;
    if (!rpm_i32(proc, ptr + STR_LEN_OFF, &slen))
        return 0;
    if (slen <= 0 || slen > 512)
        return 0;

    /* Read UTF-16LE data */
    int byte_len = slen * 2;
    if (byte_len > 1024) byte_len = 1024;
    unsigned char buf[1024];
    if (!rpm(proc, ptr + STR_DATA_OFF, buf, byte_len))
        return 0;

    /* Convert to ASCII (game strings are ASCII-safe) */
    int out_len = (slen < max_len - 1) ? slen : max_len - 1;
    for (int i = 0; i < out_len; i++) {
        unsigned short ch = buf[i * 2] | (buf[i * 2 + 1] << 8);
        out[i] = (ch < 128) ? (char)ch : '?';
    }
    out[out_len] = '\0';
    return out_len;
}

/* Read Il2CppArray<double> at ptr. Returns count or 0. */
static int read_double_array(HANDLE proc, unsigned long long ptr,
                              double *out, int max_count) {
    int max_len;
    if (!rpm_i32(proc, ptr + ARR_MAXLEN_OFF, &max_len))
        return 0;
    if (max_len <= 0 || max_len > 200)
        return 0;

    int count = (max_len < max_count) ? max_len : max_count;
    if (!rpm(proc, ptr + ARR_DATA_OFF, out, count * 8))
        return 0;
    return count;
}

/* ── Region Enumeration ──────────────────────────────────────────────── */

static void enumerate_regions(HANDLE proc) {
    MEMORY_BASIC_INFORMATION mbi;
    unsigned long long addr = 0;
    g_region_count = 0;

    while (VirtualQueryEx(proc, (LPCVOID)addr, &mbi, sizeof(mbi)) == sizeof(mbi)) {
        if (mbi.State == MEM_COMMIT &&
            (mbi.Protect & (PAGE_READONLY | PAGE_READWRITE |
                            PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE)) &&
            !(mbi.Protect & PAGE_GUARD)) {

            if (g_region_count < MAX_REGIONS) {
                g_regions[g_region_count].base = (unsigned long long)mbi.BaseAddress;
                g_regions[g_region_count].size = mbi.RegionSize;
                g_region_count++;
            }
        }

        addr = (unsigned long long)mbi.BaseAddress + mbi.RegionSize;
        if (addr == 0) break; /* wraparound */
    }
}

/* ── Scan for chr_XXXX and resolve CharInfo ──────────────────────────── */

/* UTF-16LE "chr_" = 63 00 68 00 72 00 5F 00 */
static const unsigned char CHR_PAT[8] = {0x63,0x00,0x68,0x00,0x72,0x00,0x5F,0x00};

/* Try to resolve CharInfo at base_va, expecting template_id at +0x18 */
static int try_resolve_charinfo(HANDLE proc, unsigned long long base_va,
                                 const char *expected_id) {
    /* Validate klass pointer */
    unsigned long long klass;
    if (!rpm_u64(proc, base_va, &klass) || klass == 0 || klass < 0x10000)
        return 0;

    /* Verify template_id string */
    unsigned long long str_ptr;
    if (!rpm_u64(proc, base_va + CI_TEMPLATE_ID, &str_ptr) || str_ptr == 0)
        return 0;
    char tid[32];
    if (!read_il2cpp_string(proc, str_ptr, tid, sizeof(tid)))
        return 0;
    if (strcmp(tid, expected_id) != 0)
        return 0;

    /* Read metadata */
    int level = 0, bstage = 0, potential = 0;
    rpm_i32(proc, base_va + CI_LEVEL, &level);
    rpm_i32(proc, base_va + CI_BREAK_STAGE, &bstage);
    rpm_i32(proc, base_va + CI_POTENTIAL, &potential);

    /* Navigate to Attributes */
    unsigned long long attrs_ptr = 0;
    if (!rpm_u64(proc, base_va + CI_ATTRS, &attrs_ptr) || attrs_ptr == 0) {
        /* Try fallback */
        if (!rpm_u64(proc, base_va + CI_ATTRS_PACK, &attrs_ptr) || attrs_ptr == 0)
            return 0;
    }

    /* Read m_rawData */
    unsigned long long raw_ptr = 0;
    if (!rpm_u64(proc, attrs_ptr + AT_RAW_DATA, &raw_ptr) || raw_ptr == 0)
        return 0;

    double data[MAX_RAW_DATA];
    int count = read_double_array(proc, raw_ptr, data, MAX_RAW_DATA);
    if (count < 5)
        return 0;

    /* Sanity: hp should be 0-100000, str 0-50000 */
    if (data[0] < 0 || data[0] > 100000 || data[1] < 0 || data[1] > 50000)
        return 0;

    /* Check for duplicate */
    for (int i = 0; i < g_char_count; i++) {
        if (strcmp(g_chars[i].template_id, expected_id) == 0)
            return 0; /* already captured */
    }

    if (g_char_count >= MAX_CHARS)
        return 0;

    /* Store */
    CharData *cd = &g_chars[g_char_count++];
    strncpy(cd->template_id, tid, sizeof(cd->template_id) - 1);
    cd->level = level;
    cd->break_stage = bstage;
    cd->potential_level = potential;
    memcpy(cd->raw_data, data, count * sizeof(double));
    cd->raw_data_count = count;
    cd->charinfo_va = base_va;
    cd->attrs_va = attrs_ptr;

    printf("  [+] %s (Lv%d E%d) — %d attributes at %016llX\n",
           tid, level, bstage, count, base_va);

    return 1;
}

static void scan_for_characters(HANDLE proc) {
    printf("\n=== Scanning %d memory regions for chr_XXXX ===\n", g_region_count);

    unsigned long long total_bytes = 0;
    int str_found = 0;

    /* Allocate read buffer (4 MB chunks) */
    #define CHUNK_SIZE (4 * 1024 * 1024)
    unsigned char *buf = (unsigned char *)malloc(CHUNK_SIZE);
    if (!buf) {
        printf("ERROR: Cannot allocate scan buffer\n");
        return;
    }

    for (int ri = 0; ri < g_region_count; ri++) {
        unsigned long long rbase = g_regions[ri].base;
        unsigned long long rsize = g_regions[ri].size;

        /* Read in chunks */
        for (unsigned long long off = 0; off < rsize; off += CHUNK_SIZE) {
            unsigned long long chunk_size = rsize - off;
            if (chunk_size > CHUNK_SIZE) chunk_size = CHUNK_SIZE;

            SIZE_T read;
            if (!ReadProcessMemory(proc, (LPCVOID)(rbase + off), buf,
                                   (SIZE_T)chunk_size, &read))
                continue;

            /* Scan for "chr_" UTF-16LE pattern */
            for (unsigned long long i = 0; i + 12 <= read; i += 2) {
                if (memcmp(buf + i, CHR_PAT, 8) != 0)
                    continue;

                /* Found "chr_" — check if next 4 chars are digits (UTF-16) */
                int is_id = 1;
                for (int d = 0; d < 4 && is_id; d++) {
                    unsigned long long pos = i + 8 + d * 2;
                    if (pos + 1 >= read) { is_id = 0; break; }
                    unsigned short ch = buf[pos] | (buf[pos + 1] << 8);
                    if (ch < '0' || ch > '9') is_id = 0;
                }
                if (!is_id) continue;

                /* This is a chr_XXXX string. Try resolving as Il2CppString */
                unsigned long long str_va = rbase + off + i;
                unsigned long long str_obj = str_va - STR_DATA_OFF;

                char sid[32];
                if (!read_il2cpp_string(proc, str_obj, sid, sizeof(sid)))
                    continue;
                if (strncmp(sid, "chr_", 4) != 0 || strlen(sid) < 8)
                    continue;

                str_found++;

                /* Scan for pointers to this string object across all regions */
                unsigned char ptr_bytes[8];
                memcpy(ptr_bytes, &str_obj, 8);

                for (int rj = 0; rj < g_region_count; rj++) {
                    unsigned long long rb2 = g_regions[rj].base;
                    unsigned long long rs2 = g_regions[rj].size;

                    for (unsigned long long off2 = 0; off2 < rs2; off2 += CHUNK_SIZE) {
                        unsigned long long cs2 = rs2 - off2;
                        if (cs2 > CHUNK_SIZE) cs2 = CHUNK_SIZE;

                        unsigned char *buf2 = (unsigned char *)malloc((size_t)cs2);
                        if (!buf2) continue;

                        SIZE_T r2;
                        if (!ReadProcessMemory(proc, (LPCVOID)(rb2 + off2), buf2,
                                               (SIZE_T)cs2, &r2)) {
                            free(buf2);
                            continue;
                        }

                        for (unsigned long long k = 0; k + 8 <= r2; k += 8) {
                            if (memcmp(buf2 + k, ptr_bytes, 8) != 0)
                                continue;

                            unsigned long long ptr_va = rb2 + off2 + k;
                            unsigned long long ci_base = ptr_va - CI_TEMPLATE_ID;
                            try_resolve_charinfo(proc, ci_base, sid);
                        }
                        free(buf2);
                    }
                }
            }

            total_bytes += read;
        }

        /* Progress every 1000 regions */
        if ((ri + 1) % 1000 == 0 || ri == g_region_count - 1) {
            printf("  Scanned %d/%d regions (%.1f MB), %d strings, %d chars\r",
                   ri + 1, g_region_count,
                   total_bytes / (1024.0 * 1024.0),
                   str_found, g_char_count);
            fflush(stdout);
        }
    }

    printf("\n  Scan complete: %d chr_XXXX strings, %d characters resolved\n",
           str_found, g_char_count);

    free(buf);
}

/* ── Output ──────────────────────────────────────────────────────────── */

static void write_jsonl(const char *output_dir) {
    char path[MAX_PATH];
    snprintf(path, sizeof(path), "%s\\il2cpp_char_events.jsonl", output_dir);

    FILE *f = fopen(path, "w");
    if (!f) {
        printf("ERROR: Cannot create %s\n", path);
        return;
    }

    for (int i = 0; i < g_char_count; i++) {
        CharData *cd = &g_chars[i];

        fprintf(f, "{\"type\":\"FULL_CHARACTER_CAPTURE\","
                   "\"templateId\":\"%s\","
                   "\"level\":%d,"
                   "\"breakStage\":%d,"
                   "\"potentialLevel\":%d,"
                   "\"source\":\"memscan_external\","
                   "\"rawData\":[",
                cd->template_id, cd->level, cd->break_stage,
                cd->potential_level);

        for (int j = 0; j < cd->raw_data_count; j++) {
            if (j > 0) fputc(',', f);
            fprintf(f, "%.17g", cd->raw_data[j]);
        }

        fprintf(f, "],\"namedAttrs\":{");
        for (int j = 0; j < cd->raw_data_count; j++) {
            if (j > 0) fputc(',', f);
            const char *name = (j < (int)NUM_ATTR_NAMES) ? ATTR_NAMES[j] : "unknown";
            fprintf(f, "\"%s\":%.17g", name, cd->raw_data[j]);
        }

        fprintf(f, "},\"charInfo\":{\"templateId\":\"%s\",\"level\":%d,"
                   "\"breakStage\":%d,\"potentialLevel\":%d},"
                   "\"attributes\":{\"rawData\":[",
                cd->template_id, cd->level, cd->break_stage,
                cd->potential_level);

        for (int j = 0; j < cd->raw_data_count; j++) {
            if (j > 0) fputc(',', f);
            fprintf(f, "%.17g", cd->raw_data[j]);
        }

        fprintf(f, "],\"rawDataLength\":%d},"
                   "\"_meta\":{\"charinfo_va\":\"%016llX\","
                   "\"attrs_va\":\"%016llX\"}}\n",
                cd->raw_data_count, cd->charinfo_va, cd->attrs_va);
    }

    fclose(f);
    printf("  Wrote %d events to %s\n", g_char_count, path);
}

static void write_summary(const char *output_dir) {
    char path[MAX_PATH];
    snprintf(path, sizeof(path), "%s\\memscan_summary.json", output_dir);

    FILE *f = fopen(path, "w");
    if (!f) return;

    fprintf(f, "{\n  \"total_characters\": %d,\n  \"characters\": [\n",
            g_char_count);

    for (int i = 0; i < g_char_count; i++) {
        CharData *cd = &g_chars[i];
        fprintf(f, "    {\"templateId\":\"%s\",\"level\":%d,"
                   "\"breakStage\":%d,\"rawDataLength\":%d,"
                   "\"hp\":%.1f,\"str\":%.1f,\"agi\":%.1f}%s\n",
                cd->template_id, cd->level, cd->break_stage,
                cd->raw_data_count,
                cd->raw_data[0], cd->raw_data[1],
                cd->raw_data_count > 2 ? cd->raw_data[2] : 0.0,
                (i < g_char_count - 1) ? "," : "");
    }

    fprintf(f, "  ]\n}\n");
    fclose(f);
    printf("  Wrote summary to %s\n", path);
}

static void print_table(void) {
    if (g_char_count == 0) {
        printf("\n  No characters found.\n");
        return;
    }

    printf("\n=== Character Stats ===\n\n");
    printf("%-12s %3s %2s %8s %6s %6s %6s %6s %6s %6s %6s  #\n",
           "Character", "Lv", "E", "HP", "STR", "AGI", "WISD", "WILL",
           "ATK", "DEF", "MaxHP");
    printf("%-12s %3s %2s %8s %6s %6s %6s %6s %6s %6s %6s  -\n",
           "----------", "--", "-", "------", "----", "----", "----", "----",
           "----", "----", "----");

    for (int i = 0; i < g_char_count; i++) {
        CharData *cd = &g_chars[i];
        printf("%-12s %3d %2d %8.0f %6.0f %6.1f %6.1f %6.1f %6.2f %6.2f %6.2f %2d\n",
               cd->template_id, cd->level, cd->break_stage,
               cd->raw_data[0],  /* hp */
               cd->raw_data[1],  /* str */
               cd->raw_data_count > 2 ? cd->raw_data[2] : 0.0,  /* agi */
               cd->raw_data_count > 3 ? cd->raw_data[3] : 0.0,  /* wisd */
               cd->raw_data_count > 4 ? cd->raw_data[4] : 0.0,  /* will */
               cd->raw_data_count > 5 ? cd->raw_data[5] : 0.0,  /* atk */
               cd->raw_data_count > 6 ? cd->raw_data[6] : 0.0,  /* def */
               cd->raw_data_count > 7 ? cd->raw_data[7] : 0.0,  /* maxHp */
               cd->raw_data_count);
    }
}

/* ── Main ────────────────────────────────────────────────────────────── */

int main(int argc, char **argv) {
    DWORD target_pid = 0;
    const char *output_dir = ".";

    /* Parse args */
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--pid") == 0 && i + 1 < argc) {
            target_pid = atoi(argv[++i]);
        } else if (strcmp(argv[i], "--output") == 0 && i + 1 < argc) {
            output_dir = argv[++i];
        } else if (strcmp(argv[i], "--help") == 0 || strcmp(argv[i], "-h") == 0) {
            printf("Usage: memscan.exe [--pid <PID>] [--output <dir>]\n");
            printf("  If --pid not given, auto-finds ArknightsEndfield.exe\n");
            return 0;
        }
    }

    printf("========================================\n");
    printf("  IL2CPP External Memory Scanner\n");
    printf("  Read-only, no injection\n");
    printf("========================================\n\n");

    /* Find target process */
    if (target_pid == 0) {
        printf("Searching for ArknightsEndfield.exe...\n");
        target_pid = find_game_pid();
        if (target_pid == 0) {
            printf("ERROR: Game not found. Use --pid to specify manually.\n");
            return 1;
        }
        printf("  Found PID: %lu\n", target_pid);
    }

    /* Open process read-only */
    HANDLE proc = OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION,
                              FALSE, target_pid);
    if (!proc) {
        printf("ERROR: Cannot open process %lu (error %lu)\n",
               target_pid, GetLastError());
        printf("  Try running as Administrator.\n");
        return 1;
    }

    printf("  Process opened (read-only)\n");

    /* Enumerate memory regions */
    enumerate_regions(proc);
    printf("  %d committed readable regions\n", g_region_count);

    /* Create output directory */
    CreateDirectoryA(output_dir, NULL);

    /* Scan */
    scan_for_characters(proc);

    /* Output */
    print_table();
    write_jsonl(output_dir);
    write_summary(output_dir);

    /* Cleanup */
    CloseHandle(proc);

    printf("\n========================================\n");
    printf("  Characters found: %d\n", g_char_count);
    printf("  Output: %s\n", output_dir);
    if (g_char_count > 0) {
        printf("  Next: cargo run --bin char-data-probe --release -- \\\n");
        printf("          --capture %s\\il2cpp_char_events.jsonl\n", output_dir);
    }
    printf("========================================\n");

    return 0;
}
