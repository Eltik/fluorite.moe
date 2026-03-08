/*
 * IL2CPP Stat Capture DLL v18 — Service Locator Scan
 *
 * v17 confirmed:
 *   - GMC fields are StreamingSceneManagerScript, HGLODStreamingSystemScript, etc.
 *   - GameApp → GameStateMachine → Dictionary`2 + GameMainState (circular)
 *   - NO ManagerCollections in GMC or GameApp object graphs
 *   - ManagerCollections accessed via SERVICE LOCATOR pattern, not object fields
 *
 * v18 strategy:
 *   Step A: Get GMC + GameApp singletons (kept as fallback)
 *   Step B: SERVICE LOCATOR SCAN — enumerate IDs 0x0000..0x1FFF via:
 *           service_exists(id, flags) → bool   RVA 0x4930510
 *           service_get(id, flags)    → void*  RVA 0x49304A0
 *           For each registered service: identify, check for CharBagSystem,
 *           try m_managers array.
 *   Step C: Fallback probe of GMC object
 *   Step D: Fallback probe of GameApp object
 *
 * Key offsets:
 *   ManagerCollection<3>.m_managers = +24 (SZARRAY of BaseManager)
 *   CharBagSystem.<charInfos>k__BackingField = +176 (0xB0)
 *   CharBagSystem.get_charInfos RVA = 0x26CE8C0
 *   Attributes* = *(CharInfo + 200)
 *
 * Build: x86_64-w64-mingw32-gcc -shared -nostartfiles -nodefaultlibs \
 *        -s -O2 -e DllEntry -o qavif.dll proxy.c -lkernel32
 */

/* ── Windows Constants ────────────────────────────────────────────────── */

#define DLL_PROCESS_ATTACH    1
#define DLL_PROCESS_DETACH    0
#define MAX_PATH              260
#define GENERIC_WRITE         0x40000000
#define CREATE_ALWAYS         2
#define FILE_ATTRIBUTE_NORMAL 0x80
#define INVALID_HANDLE        ((void*)(long long)-1)
#define MEM_COMMIT            0x1000
#define MEM_RESERVE           0x2000
#define MEM_RELEASE           0x8000
#define PAGE_READWRITE        0x04
#define PAGE_READONLY         0x02
#define PAGE_EXECUTE_READ     0x20
#define PAGE_EXECUTE_READWRITE 0x40
#define PAGE_GUARD            0x100
#define PAGE_NOACCESS         0x01

/* ── MEMORY_BASIC_INFORMATION for VirtualQuery ─────────────────────────── */

typedef struct {
    void                *BaseAddress;
    void                *AllocationBase;
    unsigned long        AllocationProtect;
    unsigned short       PartitionId;
    unsigned short       _pad0;
    unsigned long long   RegionSize;
    unsigned long        State;
    unsigned long        Protect;
    unsigned long        Type;
} MBI;

/* ── Kernel32 Imports ─────────────────────────────────────────────────── */

__declspec(dllimport) void* __stdcall GetModuleHandleA(const char*);
__declspec(dllimport) void* __stdcall CreateThread(void*, unsigned long,
    unsigned long __stdcall (*)(void*), void*, unsigned long, unsigned long*);
__declspec(dllimport) void* __stdcall CreateFileA(const char*, unsigned long, unsigned long,
    void*, unsigned long, unsigned long, void*);
__declspec(dllimport) int __stdcall WriteFile(void*, const void*, unsigned long,
    unsigned long*, void*);
__declspec(dllimport) int __stdcall CloseHandle(void*);
__declspec(dllimport) void __stdcall Sleep(unsigned long);
__declspec(dllimport) unsigned long __stdcall GetTickCount(void);
__declspec(dllimport) void* __stdcall GetProcAddress(void*, const char*);
__declspec(dllimport) void* __stdcall VirtualAlloc(void*, unsigned long long,
    unsigned long, unsigned long);
__declspec(dllimport) int __stdcall VirtualFree(void*, unsigned long long, unsigned long);
__declspec(dllimport) int __stdcall IsBadReadPtr(const void*, unsigned long);
__declspec(dllimport) unsigned long long __stdcall VirtualQuery(const void*, MBI*, unsigned long long);

/* ── Qt 5.15 Plugin Metadata ──────────────────────────────────────────── */

__attribute__((section(".qtmetad"), used))
static const unsigned char qt_metadata[] = {
    0x51, 0x54, 0x4D, 0x45, 0x54, 0x41, 0x44, 0x41,
    0x54, 0x41, 0x20, 0x21,
    0x00, 0x05, 0x0F, 0x00,
    0xBF, 0x02, 0x78, 0x31,
    0x6F, 0x72, 0x67, 0x2E, 0x71, 0x74, 0x2D, 0x70,
    0x72, 0x6F, 0x6A, 0x65, 0x63, 0x74, 0x2E, 0x51,
    0x74, 0x2E, 0x51, 0x49, 0x6D, 0x61, 0x67, 0x65,
    0x49, 0x4F, 0x48, 0x61, 0x6E, 0x64, 0x6C, 0x65,
    0x72, 0x46, 0x61, 0x63, 0x74, 0x6F, 0x72, 0x79,
    0x49, 0x6E, 0x74, 0x65, 0x72, 0x66, 0x61, 0x63,
    0x65,
    0x03, 0x6B,
    0x51, 0x41, 0x76, 0x69, 0x66, 0x50, 0x6C, 0x75,
    0x67, 0x69, 0x6E,
    0x04, 0xA2,
    0x64, 0x4B, 0x65, 0x79, 0x73,
    0x81, 0x64, 0x61, 0x76, 0x69, 0x66,
    0x69, 0x4D, 0x69, 0x6D, 0x65, 0x54, 0x79, 0x70,
    0x65, 0x73,
    0x81, 0x6A, 0x69, 0x6D, 0x61, 0x67, 0x65, 0x2F,
    0x61, 0x76, 0x69, 0x66,
    0xFF,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
};

/* ── Output Paths ─────────────────────────────────────────────────────── */

#define DUMP_PATH    "C:\\Users\\ianle\\il2cpp_dump.json"
#define EVENTS_PATH  "C:\\Users\\ianle\\il2cpp_char_events.jsonl"
#define LOG_PATH     "C:\\Users\\ianle\\il2cpp_capture.log"

/* ── String Utilities (no CRT) ────────────────────────────────────────── */

static unsigned long my_strlen(const char *s) {
    unsigned long n = 0;
    while (s[n]) n++;
    return n;
}

static int str_eq(const char *a, const char *b) {
    if (!a || !b) return 0;
    while (*a && *b) { if (*a != *b) return 0; a++; b++; }
    return *a == *b;
}

static int str_contains(const char *hay, const char *needle) {
    if (!hay || !needle) return 0;
    for (; *hay; hay++) {
        const char *h = hay, *n = needle;
        while (*n && *h == *n) { h++; n++; }
        if (!*n) return 1;
    }
    return 0;
}

static void my_memset(void *p, int v, unsigned long n) {
    unsigned char *b = (unsigned char*)p;
    for (unsigned long i = 0; i < n; i++) b[i] = (unsigned char)v;
}

/* ── File I/O ─────────────────────────────────────────────────────────── */

static void write_file(const char *path, const char *data, unsigned long len) {
    void *h = CreateFileA(path, GENERIC_WRITE, 0, 0, CREATE_ALWAYS,
                          FILE_ATTRIBUTE_NORMAL, 0);
    if (h != INVALID_HANDLE) {
        unsigned long written;
        WriteFile(h, data, len, &written, 0);
        CloseHandle(h);
    }
}

static void append_file(const char *path, const char *data, unsigned long len) {
    void *h = CreateFileA(path, GENERIC_WRITE, 0, 0, 4 /* OPEN_ALWAYS */,
                          FILE_ATTRIBUTE_NORMAL, 0);
    if (h != INVALID_HANDLE) {
        typedef unsigned long (__stdcall *fn_sfp)(void*, long, long*, unsigned long);
        fn_sfp sfp = (fn_sfp)GetProcAddress(GetModuleHandleA("kernel32.dll"), "SetFilePointer");
        if (sfp) sfp(h, 0, 0, 2);
        unsigned long written;
        WriteFile(h, data, len, &written, 0);
        CloseHandle(h);
    }
}

static void write_marker(const char *name, const char *msg) {
    char path[MAX_PATH];
    unsigned long p = 0;
    const char *prefix = "C:\\Users\\ianle\\";
    while (*prefix) path[p++] = *prefix++;
    while (*name && p < MAX_PATH - 1) path[p++] = *name++;
    path[p] = '\0';
    write_file(path, msg, my_strlen(msg));
}

/* ── Log Buffer ───────────────────────────────────────────────────────── */

#define LOG_MAX (512 * 1024)
static char g_log[LOG_MAX];
static unsigned long g_lp;

static void log_str(const char *s) {
    while (*s && g_lp < LOG_MAX - 1) g_log[g_lp++] = *s++;
}
static void log_u32(unsigned long v) {
    char tmp[16]; int n = 0;
    if (v == 0) { if (g_lp < LOG_MAX - 1) g_log[g_lp++] = '0'; return; }
    while (v > 0) { tmp[n++] = '0' + (v % 10); v /= 10; }
    while (n > 0 && g_lp < LOG_MAX - 1) g_log[g_lp++] = tmp[--n];
}
static void log_hex(unsigned long long v) {
    const char *hc = "0123456789ABCDEF";
    log_str("0x");
    int started = 0;
    for (int i = 60; i >= 0; i -= 4) {
        int nib = (v >> i) & 0xF;
        if (nib || started || i == 0) {
            if (g_lp < LOG_MAX - 1) g_log[g_lp++] = hc[nib];
            started = 1;
        }
    }
}
static void log_nl(void) { if (g_lp < LOG_MAX - 1) g_log[g_lp++] = '\n'; }
static void log_flush(void) {
    g_log[g_lp] = '\0';
    write_file(LOG_PATH, g_log, g_lp);
}

/* ── Large Buffer (VirtualAlloc'd for JSON) ───────────────────────────── */

#define BUF_MAX (8 * 1024 * 1024)
static char *g_buf;
static unsigned long g_bp;

static void buf_init(void) {
    g_buf = (char*)VirtualAlloc(0, BUF_MAX, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    g_bp = 0;
}
static void buf_free(void) {
    if (g_buf) { VirtualFree(g_buf, 0, MEM_RELEASE); g_buf = 0; }
}
static void b_ch(char c) { if (g_buf && g_bp < BUF_MAX - 1) g_buf[g_bp++] = c; }
static void b_raw(const char *s) { while (*s) b_ch(*s++); }
static void b_str(const char *s) {
    b_ch('"');
    if (s) while (*s && g_bp < BUF_MAX - 8) {
        if (*s == '"') { b_ch('\\'); b_ch('"'); }
        else if (*s == '\\') { b_ch('\\'); b_ch('\\'); }
        else if ((unsigned char)*s < 0x20) { }
        else b_ch(*s);
        s++;
    }
    b_ch('"');
}
static void b_u64(unsigned long long v) {
    char tmp[24]; int n = 0;
    if (v == 0) { b_ch('0'); return; }
    while (v > 0) { tmp[n++] = '0' + (v % 10); v /= 10; }
    while (n > 0) b_ch(tmp[--n]);
}
static void b_i32(int v) {
    if (v < 0) { b_ch('-'); b_u64((unsigned long long)(unsigned int)(-v)); }
    else b_u64((unsigned long long)(unsigned int)v);
}
static void b_double(double v) {
    if (v != v) { b_raw("null"); return; }
    if (v < 0) { b_ch('-'); v = -v; }
    unsigned long long integer = (unsigned long long)v;
    b_u64(integer);
    double frac = v - (double)integer;
    if (frac < 0.00005) { b_raw(".0"); return; }
    b_ch('.');
    unsigned long f4 = (unsigned long)(frac * 10000.0 + 0.5);
    if (f4 >= 10000) f4 = 9999;
    char digits[5];
    for (int i = 3; i >= 0; i--) { digits[i] = '0' + (f4 % 10); f4 /= 10; }
    digits[4] = 0;
    int last = 3;
    while (last > 0 && digits[last] == '0') last--;
    for (int i = 0; i <= last; i++) b_ch(digits[i]);
}
static void b_hex_str(unsigned long long v) {
    b_ch('"'); b_ch('0'); b_ch('x');
    const char *hc = "0123456789ABCDEF";
    int started = 0;
    for (int i = 60; i >= 0; i -= 4) {
        int nib = (v >> i) & 0xF;
        if (nib || started || i == 0) { b_ch(hc[nib]); started = 1; }
    }
    b_ch('"');
}
static void b_key(const char *k) { b_str(k); b_ch(':'); }
static void b_sep(int *first) { if (!*first) b_ch(','); *first = 0; }
static void b_bool(int v) { b_raw(v ? "true" : "false"); }

static void buf_write_to(const char *path) {
    if (!g_buf) return;
    g_buf[g_bp] = '\0';
    write_file(path, g_buf, g_bp);
    g_bp = 0;
}

/* ── IL2CPP API ───────────────────────────────────────────────────────── */

typedef void* (*t_domain_get)(void);
typedef void* (*t_thread_attach)(void*);
typedef void** (*t_domain_get_assemblies)(void*, unsigned long long*);
typedef void* (*t_assembly_get_image)(void*);
typedef const char* (*t_image_get_name)(void*);
typedef unsigned long long (*t_image_get_class_count)(void*);
typedef void* (*t_image_get_class)(void*, unsigned long long);
typedef const char* (*t_class_get_name)(void*);
typedef const char* (*t_class_get_namespace)(void*);
typedef void* (*t_class_get_fields)(void*, void**);
typedef const char* (*t_field_get_name)(void*);
typedef int (*t_field_get_offset)(void*);
typedef void* (*t_class_get_method_from_name)(void*, const char*, int);
typedef void* (*t_class_get_methods)(void*, void**);
typedef const char* (*t_method_get_name)(void*);
typedef int (*t_method_get_param_count)(void*);
typedef void* (*t_runtime_invoke)(void*, void*, void**, void**);
typedef void (*t_field_static_get_value)(void*, void*);
typedef int (*t_field_is_literal)(void*);
typedef void* (*t_field_get_type)(void*);
typedef unsigned int (*t_type_get_attrs)(void*);
typedef void* (*t_class_get_parent)(void*);
typedef unsigned int (*t_field_get_flags)(void*);
typedef void* (*t_object_get_class)(void*);
typedef int (*t_type_get_type)(void*);

static t_domain_get            p_domain_get;
static t_thread_attach         p_thread_attach;
static t_domain_get_assemblies p_domain_get_assemblies;
static t_assembly_get_image    p_assembly_get_image;
static t_image_get_name        p_image_get_name;
static t_image_get_class_count p_image_get_class_count;
static t_image_get_class       p_image_get_class;
static t_class_get_name        p_class_get_name;
static t_class_get_namespace   p_class_get_namespace;
static t_class_get_fields      p_class_get_fields;
static t_field_get_name        p_field_get_name;
static t_field_get_offset      p_field_get_offset;
static t_class_get_method_from_name p_class_get_method_from_name;
static t_class_get_methods     p_class_get_methods;
static t_method_get_name       p_method_get_name;
static t_method_get_param_count p_method_get_param_count;
static t_runtime_invoke        p_runtime_invoke;
static t_field_static_get_value p_field_static_get_value;
static t_field_is_literal      p_field_is_literal;
static t_field_get_type        p_field_get_type;
static t_type_get_attrs        p_type_get_attrs;
static t_class_get_parent      p_class_get_parent;
static t_field_get_flags       p_field_get_flags;
static t_object_get_class      p_object_get_class;
static t_type_get_type         p_type_get_type;

static void *g_ga_base;

#define FIELD_ATTR_STATIC  0x10
#define FIELD_ATTR_LITERAL 0x40

static int resolve_api(void *ga) {
    typedef struct { const char *n; void **s; } E;
    static E e[] = {
        {"il2cpp_domain_get",            (void**)&p_domain_get},
        {"il2cpp_thread_attach",         (void**)&p_thread_attach},
        {"il2cpp_domain_get_assemblies", (void**)&p_domain_get_assemblies},
        {"il2cpp_assembly_get_image",    (void**)&p_assembly_get_image},
        {"il2cpp_image_get_name",        (void**)&p_image_get_name},
        {"il2cpp_image_get_class_count", (void**)&p_image_get_class_count},
        {"il2cpp_image_get_class",       (void**)&p_image_get_class},
        {"il2cpp_class_get_name",        (void**)&p_class_get_name},
        {"il2cpp_class_get_namespace",   (void**)&p_class_get_namespace},
        {"il2cpp_class_get_fields",      (void**)&p_class_get_fields},
        {"il2cpp_field_get_name",        (void**)&p_field_get_name},
        {"il2cpp_field_get_offset",      (void**)&p_field_get_offset},
        {"il2cpp_class_get_method_from_name", (void**)&p_class_get_method_from_name},
        {"il2cpp_class_get_methods",     (void**)&p_class_get_methods},
        {"il2cpp_method_get_name",       (void**)&p_method_get_name},
        {"il2cpp_method_get_param_count",(void**)&p_method_get_param_count},
        {"il2cpp_runtime_invoke",        (void**)&p_runtime_invoke},
        {"il2cpp_field_static_get_value",(void**)&p_field_static_get_value},
        {"il2cpp_field_is_literal",      (void**)&p_field_is_literal},
        {"il2cpp_field_get_type",        (void**)&p_field_get_type},
        {"il2cpp_type_get_attrs",        (void**)&p_type_get_attrs},
        {"il2cpp_class_get_parent",      (void**)&p_class_get_parent},
        {"il2cpp_field_get_flags",       (void**)&p_field_get_flags},
        {"il2cpp_object_get_class",      (void**)&p_object_get_class},
        {"il2cpp_type_get_type",         (void**)&p_type_get_type},
        {0,0}
    };
    int ok = 0;
    for (E *p = e; p->n; p++) {
        *p->s = GetProcAddress(ga, p->n);
        if (*p->s) ok++;
        log_str("  "); log_str(p->n); log_str(*p->s ? " OK\n" : " FAIL\n");
    }
    return ok;
}

/* ── Attribute Names (47 stats, indices 0-46) ─────────────────────────── */

static const char *ATTR_NAMES[] = {
    "hp","str","agi","wisd","will","atk","def","maxHp",
    "maxUltimateSp","maxPoise","poiseRecTime","poiseRecTimeScalar",
    "physicalDmgTaken","fireDmgTaken","pulseDmgTaken","crystDmgTaken",
    "naturalDmgTaken","etherDmgTaken","weight","critRate","critDmg",
    "moveSpeed","turnRate","atkRate","skillCooldown","normalAtkRange",
    "hpRegen","hpRegenByRatio","poiseTaken","poiseOutput",
    "breakAtkDmgTaken","knockdownTimeAdd","healOutput","healTaken",
    "lifeSteal","ultSpGain","atbCostAdd","skillCooldownAdd",
    "comboSkillCooldown","igniteDmg","physInflictDmg",
    "physDmgIncrease","fireDmgIncrease","pulseDmgIncrease",
    "crystDmgIncrease","naturalDmgIncrease","etherDmgIncrease"
};
#define NUM_ATTRS 47

/* ── Scalar Getter RVAs ───────────────────────────────────────────────── */

typedef struct { const char *name; unsigned long long rva; } ScalarInfo;
static ScalarInfo SCALARS[] = {
    {"hpBaseAddFromStr",                0x3B06C80},
    {"atkFinalScalarFromMain",          0x3B06E30},
    {"atkFinalScalarFromSub",           0x3B06F60},
    {"atkFinalScalarFromStr",           0x3B07090},
    {"atkFinalScalarFromAgi",           0x3B07150},
    {"atkFinalScalarFromWisd",          0x3B07210},
    {"atkFinalScalarFromWill",          0x3B072D0},
    {"strAtkFactorFromMainSub",         0x3B07390},
    {"agiAtkFactorFromMainSub",         0x3B074D0},
    {"wisdAtkFactorFromMainSub",        0x3B07610},
    {"willAtkFactorFromMainSub",        0x3B07750},
    {"physResistFromAgi",               0x3B07890},
    {"energyResistFromWisd",            0x3B07A60},
    {"healScalarFromWill",              0x3B07C30},
    {"healTakenScalarFromWill",         0x3B07DE0},
    {0, 0}
};

/* ── IL2CPP Object Layout Constants ───────────────────────────────────── */

#define CI_TEMPLATE_ID   0x18  /* offset=24: Il2CppString* templateId */
#define CI_LEVEL         0x44  /* offset=68: int level */
#define CI_EXP           0x48  /* offset=72: int exp */
#define CI_BREAK_STAGE   0x4C  /* offset=76: int breakStage */
#define CI_POTENTIAL     0x60  /* offset=96: int potentialLevel */
#define CI_ATTRS         0xC8  /* offset=200: Attributes* (confirmed via RVA deduction) */
#define CI_ATTRS_PACK    0xB8  /* offset=184: attributesPack (fallback) */
#define AT_RAW_DATA      0x60  /* offset=96: Attributes.m_rawData */
#define RVA_GET_VALUE    0x3B05D60

/* ── Character Data Storage ───────────────────────────────────────────── */

#define MAX_CHARS 64

typedef struct {
    void *charinfo_ptr;
    void *attrs_ptr;
    char template_id[32];
    int level;
    int break_stage;
    int exp;
    unsigned int potential;
    int has_raw_data;
    int raw_data_len;
    double raw_data[128];
    double get_values[NUM_ATTRS];
    int has_get_values;
    double scalars[16];
    int has_scalars;
} CharData;

static CharData g_chars[MAX_CHARS];
static volatile int g_char_count;
static void *g_charinfo_klass;
static volatile int g_should_exit;

/* Klass set: CharInfo + all subclasses */
#define MAX_KLASS 16
static void *g_klass_set[MAX_KLASS];
static int g_klass_count;

static int is_charinfo_klass(void *klass) {
    for (int i = 0; i < g_klass_count; i++)
        if (g_klass_set[i] == klass) return 1;
    return 0;
}

/* ── Hex Dump Helper ──────────────────────────────────────────────────── */

static void log_obj_pointers(void *obj, int num_slots) {
    if (!obj || IsBadReadPtr(obj, num_slots * 8)) return;
    for (int i = 0; i < num_slots; i++) {
        void *val = *(void**)((char*)obj + i * 8);
        if (i % 2 == 0) {
            log_str("    +"); log_hex(i * 8); log_str(": ");
        }
        log_hex((unsigned long long)val);
        if (i % 2 == 1) log_nl();
        else log_str("  ");
    }
    if (num_slots % 2 != 0) log_nl();
}

/* ── Il2CppString Reader ─────────────────────────────────────────────── */

static int read_il2cpp_string(void *str_ptr, char *out, int max_len) {
    if (!str_ptr || IsBadReadPtr(str_ptr, 0x18)) return 0;
    int len = *(int*)((char*)str_ptr + 0x10);
    if (len <= 0 || len > 128) return 0;
    if (len >= max_len) len = max_len - 1;
    unsigned short *chars = (unsigned short*)((char*)str_ptr + 0x14);
    if (IsBadReadPtr(chars, len * 2)) return 0;
    for (int i = 0; i < len; i++) {
        unsigned short c = chars[i];
        out[i] = (c < 128) ? (char)c : '?';
    }
    out[len] = '\0';
    return len;
}

static int starts_with_chr(const char *s) {
    return s[0]=='c' && s[1]=='h' && s[2]=='r' && s[3]=='_';
}

/* ── Read Stats from Attributes ───────────────────────────────────────── */

static int read_raw_data(void *attrs, CharData *cd) {
    if (!attrs || IsBadReadPtr(attrs, AT_RAW_DATA + 8)) return 0;
    void *raw_arr = *(void**)((char*)attrs + AT_RAW_DATA);
    if (!raw_arr || IsBadReadPtr(raw_arr, 0x28)) return 0;
    int arr_len = *(int*)((char*)raw_arr + 0x18);
    if (arr_len <= 0 || arr_len > 128) return 0;
    double *data = (double*)((char*)raw_arr + 0x20);
    if (IsBadReadPtr(data, arr_len * 8)) return 0;
    cd->raw_data_len = arr_len;
    for (int i = 0; i < arr_len && i < 128; i++)
        cd->raw_data[i] = data[i];
    cd->has_raw_data = 1;
    return 1;
}

static int call_get_values(void *attrs, CharData *cd) {
    typedef double (*fn_get_value)(void*, int);
    fn_get_value get_val = (fn_get_value)((char*)g_ga_base + RVA_GET_VALUE);
    if (IsBadReadPtr((void*)get_val, 16)) return 0;
    for (int i = 0; i < NUM_ATTRS; i++)
        cd->get_values[i] = get_val(attrs, i);
    cd->has_get_values = 1;
    return 1;
}

static int call_scalars(void *attrs, CharData *cd) {
    typedef double (*fn_scalar)(void*);
    int count = 0;
    for (int i = 0; SCALARS[i].name; i++) {
        fn_scalar fn = (fn_scalar)((char*)g_ga_base + SCALARS[i].rva);
        if (!IsBadReadPtr((void*)fn, 16)) {
            cd->scalars[i] = fn(attrs);
            count++;
        }
    }
    cd->has_scalars = (count > 0);
    return count;
}

/* ── Process a CharInfo object ────────────────────────────────────────── */

/*
 * Try to find a "chr_XXXX" Il2CppString at various offsets within obj.
 * Returns the offset where found, or -1 if not found.
 */
static int probe_template_id(void *obj, char *out_tid, int max_len) {
    /* Strategy 1: Try expected offset first */
    void *str_ptr = *(void**)((char*)obj + CI_TEMPLATE_ID);
    if (read_il2cpp_string(str_ptr, out_tid, max_len) && starts_with_chr(out_tid))
        return CI_TEMPLATE_ID;

    /* Strategy 2: Try all pointer-aligned offsets 0x10..0x80 */
    for (int off = 0x10; off <= 0x80; off += 8) {
        if (off == CI_TEMPLATE_ID) continue; /* already tried */
        if (IsBadReadPtr((char*)obj + off, 8)) continue;
        void *ptr = *(void**)((char*)obj + off);
        if (!ptr || IsBadReadPtr(ptr, 0x18)) continue;
        int len = *(int*)((char*)ptr + 0x10);
        if (len < 5 || len > 32) continue;
        unsigned short *chars = (unsigned short*)((char*)ptr + 0x14);
        if (IsBadReadPtr(chars, len * 2)) continue;
        if (chars[0]=='c' && chars[1]=='h' && chars[2]=='r' && chars[3]=='_' &&
            chars[4] >= '0' && chars[4] <= '9') {
            int cl = len < max_len - 1 ? len : max_len - 1;
            for (int i = 0; i < cl; i++)
                out_tid[i] = (chars[i] < 128) ? (char)chars[i] : '?';
            out_tid[cl] = '\0';
            return off;
        }
    }

    return -1;
}

static int process_charinfo(void *obj) {
    if (g_char_count >= MAX_CHARS || g_should_exit) return 0;
    if (!obj || IsBadReadPtr(obj, 0xF0)) return 0;

    char tid[32];
    int tid_offset = probe_template_id(obj, tid, sizeof(tid));
    if (tid_offset < 0) return 0;

    int level = *(int*)((char*)obj + CI_LEVEL);
    int brk   = *(int*)((char*)obj + CI_BREAK_STAGE);
    int exp   = *(int*)((char*)obj + CI_EXP);

    if (level < 0 || level > 200 || brk < 0 || brk > 20) return 0;

    for (int i = 0; i < g_char_count; i++)
        if (str_eq(g_chars[i].template_id, tid)) return 0;

    CharData *cd = &g_chars[g_char_count];
    my_memset(cd, 0, sizeof(CharData));
    cd->charinfo_ptr = obj;
    for (int i = 0; tid[i] && i < 31; i++) cd->template_id[i] = tid[i];
    cd->template_id[31] = '\0';
    cd->level       = level;
    cd->break_stage = brk;
    cd->exp         = exp;
    cd->potential   = *(unsigned int*)((char*)obj + CI_POTENTIAL);

    /* Navigate: CharInfo → Attributes
     * Primary: offset 0xC8 (200) — confirmed via RVA deduction
     * Fallback: offset 0xB8 (184) — attributesPack with inner walk */
    void *attrs = 0;

    /* Try 0xC8 first */
    if (!IsBadReadPtr((char*)obj + CI_ATTRS, 8)) {
        void *ptr_c8 = *(void**)((char*)obj + CI_ATTRS);
        if (ptr_c8 && !IsBadReadPtr(ptr_c8, 0x70)) {
            void *raw_c = *(void**)((char*)ptr_c8 + AT_RAW_DATA);
            if (raw_c && !IsBadReadPtr(raw_c, 0x28)) {
                int alen = *(int*)((char*)raw_c + 0x18);
                if (alen > 40 && alen <= 128) attrs = ptr_c8;
            }
        }
    }

    /* Fallback: 0xB8 attributesPack */
    if (!attrs) {
        void *attrs_pack = *(void**)((char*)obj + CI_ATTRS_PACK);
        if (attrs_pack && !IsBadReadPtr(attrs_pack, 0x70)) {
            void *raw_candidate = *(void**)((char*)attrs_pack + AT_RAW_DATA);
            if (raw_candidate && !IsBadReadPtr(raw_candidate, 0x28)) {
                int alen = *(int*)((char*)raw_candidate + 0x18);
                if (alen > 40 && alen <= 128) attrs = attrs_pack;
            }
            if (!attrs) {
                for (int woff = 8; woff <= 24; woff += 8) {
                    if (IsBadReadPtr((char*)attrs_pack + woff, 8)) continue;
                    void *inner = *(void**)((char*)attrs_pack + woff);
                    if (!inner || IsBadReadPtr(inner, 0x70)) continue;
                    void *raw2 = *(void**)((char*)inner + AT_RAW_DATA);
                    if (!raw2 || IsBadReadPtr(raw2, 0x28)) continue;
                    int alen2 = *(int*)((char*)raw2 + 0x18);
                    if (alen2 > 40 && alen2 <= 128) { attrs = inner; break; }
                }
            }
        }
    }

    cd->attrs_ptr = attrs;
    if (attrs) {
        read_raw_data(attrs, cd);
        if (cd->has_raw_data) {
            call_get_values(attrs, cd);
            call_scalars(attrs, cd);
        }
    }

    log_str("  CAPTURED: "); log_str(tid);
    log_str(" @+"); log_hex(tid_offset);
    log_str(" lv="); log_u32(level);
    log_str(" attrs="); log_hex((unsigned long long)attrs);
    if (cd->has_raw_data) {
        log_str(" rawData["); log_u32(cd->raw_data_len); log_str("]");
    }
    log_nl();

    g_char_count++;
    return 1;
}

/* ── Check if a field is static ────────────────────────────────────────── */

static unsigned int field_get_flags_safe(void *field) {
    if (p_field_get_flags) return p_field_get_flags(field);
    if (p_field_get_type && p_type_get_attrs) {
        void *ftype = p_field_get_type(field);
        if (ftype) return p_type_get_attrs(ftype);
    }
    return 0;
}

static int field_is_static(void *field) {
    return (field_get_flags_safe(field) & FIELD_ATTR_STATIC) != 0;
}

/* NOTE: il2cpp_field_is_literal is BROKEN in this build (returns TRUE for
 * all fields). Use manual flag check instead. */
static int field_is_literal(void *field) {
    return (field_get_flags_safe(field) & FIELD_ATTR_LITERAL) != 0;
}

/* ── Find a class by name ─────────────────────────────────────────────── */

static void *find_class(void *domain, const char *class_name, const char *ns,
                        const char *asm_name) {
    unsigned long long asm_count = 0;
    void **assemblies = p_domain_get_assemblies(domain, &asm_count);
    if (!assemblies) return 0;

    for (unsigned long long i = 0; i < asm_count; i++) {
        if (!assemblies[i]) continue;
        void *image = p_assembly_get_image(assemblies[i]);
        if (!image) continue;
        if (asm_name) {
            const char *iname = p_image_get_name ? p_image_get_name(image) : "";
            if (!str_eq(iname, asm_name)) continue;
        }
        unsigned long long cc = p_image_get_class_count(image);
        for (unsigned long long ci = 0; ci < cc; ci++) {
            void *klass = p_image_get_class(image, ci);
            if (!klass) continue;
            const char *name = p_class_get_name(klass);
            if (!name || !str_eq(name, class_name)) continue;
            if (ns && p_class_get_namespace) {
                const char *kns = p_class_get_namespace(klass);
                if (!kns || !str_eq(kns, ns)) continue;
            }
            return klass;
        }
    }
    return 0;
}

/* ── Get method pointer from MethodInfo ────────────────────────────────── */

static void *get_method_pointer(void *method_info) {
    if (!method_info || IsBadReadPtr(method_info, 8)) return 0;
    return *(void**)method_info;
}

/* ──────────────────────────────────────────────────────────────────────── */
/*  IL2CPP Type Constants                                                  */
/* ──────────────────────────────────────────────────────────────────────── */

#define IL2CPP_TYPE_STRING     0x0E
#define IL2CPP_TYPE_CLASS      0x12
#define IL2CPP_TYPE_ARRAY      0x14
#define IL2CPP_TYPE_GENERICINST 0x15
#define IL2CPP_TYPE_OBJECT     0x1C
#define IL2CPP_TYPE_SZARRAY    0x1D

static int type_is_object_ref(void *field) {
    if (!p_field_get_type || !p_type_get_type) return -1;
    void *ftype = p_field_get_type(field);
    if (!ftype) return -1;
    int t = p_type_get_type(ftype);
    return (t == IL2CPP_TYPE_CLASS || t == IL2CPP_TYPE_OBJECT ||
            t == IL2CPP_TYPE_GENERICINST || t == IL2CPP_TYPE_SZARRAY ||
            t == IL2CPP_TYPE_ARRAY || t == IL2CPP_TYPE_STRING);
}

/* Get class name safely (handles NULL, bad ptrs) */
static const char *safe_class_name(void *obj) {
    if (!obj || IsBadReadPtr(obj, 16)) return 0;
    void *klass;
    if (p_object_get_class) {
        klass = p_object_get_class(obj);
    } else {
        klass = *(void**)obj;
    }
    if (!klass || IsBadReadPtr(klass, 8)) return 0;
    return p_class_get_name(klass);
}

/* Get class pointer safely */
static void *safe_get_class(void *obj) {
    if (!obj || IsBadReadPtr(obj, 16)) return 0;
    if (p_object_get_class) return p_object_get_class(obj);
    void *klass = *(void**)obj;
    if (!klass || IsBadReadPtr(klass, 8)) return 0;
    return klass;
}

/* ──────────────────────────────────────────────────────────────────────── */
/*  Phase 2: API-Based Object Graph Navigation                             */
/* ──────────────────────────────────────────────────────────────────────── */

/*
 * Walk all instance fields of obj's class (+ parent chain).
 * For object-reference fields, read the pointer, identify its runtime class,
 * and log it. If it's a CharInfo, call process_charinfo.
 * Recurse into "interesting" objects (managers, collections, etc.)
 * max depth = 4 to avoid infinite loops.
 */

#define MAX_VISITED 256
static void *g_visited[MAX_VISITED];
static int g_visited_count;

static int already_visited(void *ptr) {
    for (int i = 0; i < g_visited_count; i++)
        if (g_visited[i] == ptr) return 1;
    return 0;
}

static void mark_visited(void *ptr) {
    if (g_visited_count < MAX_VISITED)
        g_visited[g_visited_count++] = ptr;
}

static int is_interesting_class(const char *name) {
    return str_contains(name, "Manager") ||
           str_contains(name, "Collection") ||
           str_contains(name, "Factory") ||
           str_contains(name, "Character") ||
           str_contains(name, "CharInfo") ||
           str_contains(name, "Player") ||
           str_contains(name, "Container") ||
           str_contains(name, "Context") ||
           str_contains(name, "Ability") ||
           str_contains(name, "Squad") ||
           str_contains(name, "Deck") ||
           str_contains(name, "Bag") ||
           str_contains(name, "Roster") ||
           str_contains(name, "DataManager") ||
           str_contains(name, "GameApp");
}

static void walk_object_fields(void *obj, const char *prefix, int depth);

/* Try to iterate a List<T> or T[] and process CharInfo elements */
static int try_iterate_collection(void *obj, const char *prefix) {
    if (!obj || IsBadReadPtr(obj, 0x20)) return 0;
    void *klass = safe_get_class(obj);
    if (!klass) return 0;
    const char *kname = p_class_get_name(klass);
    if (!kname) return 0;

    int found = 0;

    /* Check if it's a List<T> — has _items (T[]) at some offset and _size */
    /* Standard List<T> layout: +0x10 = T[] _items, +0x18 = int _size */
    if (str_contains(kname, "List")) {
        if (IsBadReadPtr((char*)obj + 0x18, 4)) return 0;
        int size = *(int*)((char*)obj + 0x18);
        if (size <= 0 || size > 1024) return 0;

        if (IsBadReadPtr((char*)obj + 0x10, 8)) return 0;
        void *items = *(void**)((char*)obj + 0x10);
        if (!items || IsBadReadPtr(items, 0x20 + size * 8)) return 0;

        int arr_len = *(int*)((char*)items + 0x18);
        if (arr_len < size) return 0;

        log_str("  "); log_str(prefix);
        log_str(" → List["); log_u32(size); log_str("]\n");

        void **data = (void**)((char*)items + 0x20);
        for (int i = 0; i < size && i < 64; i++) {
            void *elem = data[i];
            if (!elem || IsBadReadPtr(elem, 16)) continue;
            void *elem_klass = safe_get_class(elem);
            if (is_charinfo_klass(elem_klass)) {
                if (process_charinfo(elem)) found++;
            }
        }
        return found;
    }

    /* Check if it's a raw array (Il2CppArray) */
    /* Layout: +0x18 = int length, +0x20 = T[] data */
    if (str_contains(kname, "[]")) {
        if (IsBadReadPtr((char*)obj + 0x18, 4)) return 0;
        int arr_len = *(int*)((char*)obj + 0x18);
        if (arr_len <= 0 || arr_len > 1024) return 0;
        if (IsBadReadPtr((char*)obj + 0x20, arr_len * 8)) return 0;

        log_str("  "); log_str(prefix);
        log_str(" → Array["); log_u32(arr_len); log_str("]\n");

        void **data = (void**)((char*)obj + 0x20);
        for (int i = 0; i < arr_len && i < 64; i++) {
            void *elem = data[i];
            if (!elem || IsBadReadPtr(elem, 16)) continue;
            void *elem_klass = safe_get_class(elem);
            if (is_charinfo_klass(elem_klass)) {
                if (process_charinfo(elem)) found++;
            }
        }
        return found;
    }

    return 0;
}

static void walk_object_fields(void *obj, const char *prefix, int depth) {
    if (depth > 4 || !obj || g_should_exit) return;
    if (IsBadReadPtr(obj, 16)) return;
    if (already_visited(obj)) return;
    mark_visited(obj);

    void *klass = safe_get_class(obj);
    if (!klass) return;

    /* Walk the class hierarchy (this class + parents) */
    void *cur_klass = klass;
    while (cur_klass && !g_should_exit) {
        const char *kname = p_class_get_name(cur_klass);
        if (!kname || str_eq(kname, "Object") || str_eq(kname, "MonoBehaviour") ||
            str_eq(kname, "Component") || str_eq(kname, "Behaviour")) break;

        void *fiter = 0; void *field;
        while ((field = p_class_get_fields(cur_klass, &fiter)) != 0) {
            if (g_should_exit) return;
            const char *fname = p_field_get_name(field);
            if (!fname) continue;
            int foff = p_field_get_offset(field);
            if (field_is_static(field)) continue;
            if (field_is_literal(field)) continue;

            /* Check if this field is an object reference */
            int is_ref = type_is_object_ref(field);
            if (is_ref != 1) continue;

            /* Sanity check offset */
            if (foff < 0x10 || foff > 4096) continue;
            if (IsBadReadPtr((char*)obj + foff, 8)) continue;

            void *field_val = *(void**)((char*)obj + foff);
            if (!field_val || IsBadReadPtr(field_val, 16)) continue;

            void *val_klass = safe_get_class(field_val);
            if (!val_klass) continue;
            const char *val_cname = p_class_get_name(val_klass);
            if (!val_cname) continue;

            /* Log this field */
            for (int i = 0; i < depth; i++) log_str("  ");
            log_str("  "); log_str(prefix); log_str(".");
            log_str(fname);
            log_str(" (+"); log_hex(foff); log_str(") → ");
            log_str(val_cname);
            log_str(" @"); log_hex((unsigned long long)field_val);
            log_nl();

            /* Check for CharInfo! */
            if (is_charinfo_klass(val_klass)) {
                if (process_charinfo(field_val)) {
                    log_str("    *** CharInfo FOUND via API navigation! ***\n");
                }
                continue;
            }

            /* Try iterating if it looks like a collection */
            if (str_contains(val_cname, "List") || str_contains(val_cname, "[]")) {
                char cpath[64];
                int cp = 0;
                const char *pp = prefix;
                while (*pp && cp < 50) cpath[cp++] = *pp++;
                cpath[cp++] = '.';
                while (*fname && cp < 62) cpath[cp++] = *fname++;
                cpath[cp] = '\0';
                try_iterate_collection(field_val, cpath);
                continue;
            }

            /* Recurse into interesting objects */
            if (depth < 3 && is_interesting_class(val_cname)) {
                char nprefix[64];
                int np = 0;
                while (*fname && np < 62) nprefix[np++] = *fname++;
                nprefix[np] = '\0';
                walk_object_fields(field_val, nprefix, depth + 1);
            }
        }

        cur_klass = p_class_get_parent ? p_class_get_parent(cur_klass) : 0;
    }
}

/* Call a static 0-arg method via il2cpp_runtime_invoke, return result */
static void *invoke_static_method(void *klass, const char *method_name) {
    if (!klass || !p_class_get_method_from_name || !p_runtime_invoke) return 0;
    void *method = p_class_get_method_from_name(klass, method_name, 0);
    if (!method) {
        log_str("  method "); log_str(method_name); log_str(" not found\n");
        return 0;
    }
    void *exc = 0;
    void *result = p_runtime_invoke(method, 0, 0, &exc);
    if (exc) {
        log_str("  "); log_str(method_name); log_str("() threw exception\n");
        return 0;
    }
    return result;
}

/* Call an instance 0-arg method via il2cpp_runtime_invoke */
static void *invoke_instance_method(void *klass, void *obj, const char *method_name) {
    if (!klass || !obj || !p_class_get_method_from_name || !p_runtime_invoke) return 0;
    void *method = p_class_get_method_from_name(klass, method_name, 0);
    if (!method) return 0;
    void *exc = 0;
    void *result = p_runtime_invoke(method, obj, 0, &exc);
    if (exc) return 0;
    return result;
}

/* ── Known klass pointers for target classes ─────────────────────────── */

static void *g_charbag_klass;
static void *g_datamgr_klass;
static void *g_gamemgr_coll_klass;
static void *g_playermgr_coll_klass;

#define CBS_CHAR_INFOS       0xB0  /* +176: <charInfos>k__BackingField */
#define CBS_CLIENT_CHARINFOS 0xC8  /* +200: m_clientCharInfos */

/*
 * Given a CharBagSystem instance, extract all CharInfo objects.
 * Tries API methods first, then direct field reads, then dictionary parsing.
 */
static int extract_from_charbag(void *charbag) {
    if (!charbag || IsBadReadPtr(charbag, CBS_CLIENT_CHARINFOS + 8)) return 0;
    int found = 0;

    log_str("\n*** CharBagSystem instance found @");
    log_hex((unsigned long long)charbag); log_str(" ***\n");

    /* Strategy 1: Call get_charInfos / get_charList / get_clientCharInfos */
    if (g_charbag_klass) {
        const char *getters[] = {
            "get_charInfos", "get_charList", "get_clientCharInfos", 0
        };
        for (int gi = 0; getters[gi] && found == 0; gi++) {
            void *method = p_class_get_method_from_name(g_charbag_klass, getters[gi], 0);
            if (!method) continue;
            log_str("  Calling "); log_str(getters[gi]); log_str("()...\n");
            void *exc = 0;
            void *result = p_runtime_invoke(method, charbag, 0, &exc);
            if (!result || exc || IsBadReadPtr(result, 16)) {
                log_str("    → null/exception\n"); continue;
            }
            const char *rcname = safe_class_name(result);
            log_str("    → "); if (rcname) log_str(rcname);
            log_str(" @"); log_hex((unsigned long long)result); log_nl();
            found += try_iterate_collection(result, getters[gi]);
        }
    }

    /* Strategy 2: Direct field reads + dictionary parsing */
    int field_offsets[] = {CBS_CHAR_INFOS, CBS_CLIENT_CHARINFOS, 0x70, 0x78, 0};
    for (int fi = 0; field_offsets[fi] && found == 0; fi++) {
        int off = field_offsets[fi];
        if (IsBadReadPtr((char*)charbag + off, 8)) continue;
        void *fval = *(void**)((char*)charbag + off);
        if (!fval || IsBadReadPtr(fval, 0x30)) continue;

        const char *fcname = safe_class_name(fval);
        log_str("  +"); log_hex(off); log_str(" → ");
        if (fcname) log_str(fcname); else log_str("?");
        log_str(" @"); log_hex((unsigned long long)fval); log_nl();

        found += try_iterate_collection(fval, "cbs_field");

        /* Try as Dictionary<int, CharInfo>: count at +0x38, entries at +0x20 */
        if (found == 0 && !IsBadReadPtr((char*)fval + 0x40, 8)) {
            /* Try multiple count offsets — Dict layout varies */
            int count_offsets[] = {0x28, 0x30, 0x38, 0x40, 0};
            for (int ci = 0; count_offsets[ci] && found == 0; ci++) {
                int co = count_offsets[ci];
                if (IsBadReadPtr((char*)fval + co, 4)) continue;
                int cnt = *(int*)((char*)fval + co);
                if (cnt <= 0 || cnt > 128) continue;

                /* Find entries array — try offsets 0x18, 0x20 */
                int entry_arr_offsets[] = {0x18, 0x20, 0};
                for (int eai = 0; entry_arr_offsets[eai] && found == 0; eai++) {
                    int eao = entry_arr_offsets[eai];
                    if (IsBadReadPtr((char*)fval + eao, 8)) continue;
                    void *earr = *(void**)((char*)fval + eao);
                    if (!earr || IsBadReadPtr(earr, 0x28)) continue;

                    int alen = *(int*)((char*)earr + 0x18);
                    if (alen < cnt || alen > 512) continue;

                    log_str("    dict? cnt="); log_u32(cnt);
                    log_str(" entries_len="); log_u32(alen);
                    log_str(" co="); log_hex(co);
                    log_str(" eao="); log_hex(eao); log_nl();

                    /* Scan entries for CharInfo pointers using klass comparison */
                    char *edata = (char*)earr + 0x20;
                    /* Try entry sizes 24, 32, 40 */
                    int esizes[] = {24, 32, 40, 0};
                    for (int esi = 0; esizes[esi] && found == 0; esi++) {
                        int esz = esizes[esi];
                        int tf = 0;
                        for (int ei = 0; ei < alen && ei < 64; ei++) {
                            char *entry = edata + ei * esz;
                            if (IsBadReadPtr(entry, esz)) break;
                            for (int vo = 8; vo < esz; vo += 8) {
                                void *val = *(void**)(entry + vo);
                                if (!val || IsBadReadPtr(val, 16)) continue;
                                void *vk = safe_get_class(val);
                                if (is_charinfo_klass(vk)) {
                                    if (process_charinfo(val)) { tf++; found++; }
                                }
                            }
                        }
                        if (tf > 0) {
                            log_str("    Dict(esz="); log_u32(esz);
                            log_str("): "); log_u32(tf); log_str(" CharInfo\n");
                        }
                    }
                }
            }
        }
    }

    log_str("  CharBagSystem extraction: ");
    log_u32(found); log_str(" CharInfo found\n");
    log_flush();
    return found;
}

/* Store known klasses for safe comparison */
#define MAX_KNOWN_KLASS 32
static struct { void *klass; const char *name; } g_known[MAX_KNOWN_KLASS];
static int g_known_count;

static void register_known_klass(void *klass, const char *name) {
    if (!klass || g_known_count >= MAX_KNOWN_KLASS) return;
    for (int i = 0; i < g_known_count; i++)
        if (g_known[i].klass == klass) return;
    g_known[g_known_count].klass = klass;
    g_known[g_known_count].name = name;
    g_known_count++;
}

static const char *identify_by_known_klass(void *obj) {
    if (!obj || IsBadReadPtr(obj, 8)) return 0;
    void *klass = *(void**)obj;
    for (int i = 0; i < g_known_count; i++)
        if (g_known[i].klass == klass) return g_known[i].name;
    return 0;
}

/*
 * Try to identify an object via IL2CPP API with multi-level safety checks.
 * This is safe for objects at field offsets of known-valid IL2CPP objects:
 *   1. Check ptr readable (16 bytes)
 *   2. Read klass pointer manually (*(void**)ptr)
 *   3. Check klass readable (256 bytes — Il2CppClass is large)
 *   4. Call p_class_get_name(klass) — just reads a pointer from the struct
 *   5. Check name readable (4 bytes)
 *   6. Verify first chars are printable ASCII (rejects garbage)
 * Returns class name or NULL.
 */
static const char *try_identify_object(void *ptr) {
    if (!ptr || IsBadReadPtr(ptr, 16)) return 0;
    void *klass = *(void**)ptr;
    if (!klass || IsBadReadPtr(klass, 256)) return 0;
    const char *name = p_class_get_name(klass);
    if (!name || IsBadReadPtr(name, 4)) return 0;
    /* Verify printable ASCII — rejects garbage pointers */
    for (int i = 0; i < 4 && name[i]; i++)
        if ((unsigned char)name[i] < 0x20 || (unsigned char)name[i] > 0x7E) return 0;
    if (name[0] == '\0') return 0;
    return name;
}

/* Get namespace of an object (via klass) — same safety as try_identify_object */
static const char *try_get_namespace(void *ptr) {
    if (!ptr || IsBadReadPtr(ptr, 16) || !p_class_get_namespace) return 0;
    void *klass = *(void**)ptr;
    if (!klass || IsBadReadPtr(klass, 256)) return 0;
    const char *ns = p_class_get_namespace(klass);
    if (!ns || IsBadReadPtr(ns, 1)) return "";
    return ns;
}

/*
 * Try reading m_managers (SZARRAY at offset +24) from a potential ManagerCollection.
 * Iterates the array elements, identifies each by klass comparison.
 * Returns count of CharInfo found (via extract_from_charbag).
 */
static int try_read_managers_array(void *obj, const char *label) {
    if (!obj || IsBadReadPtr((char*)obj + 0x18 + 8, 8)) return 0;

    /* m_managers is at offset +24 (0x18) from ManagerCollection */
    void *arr = *(void**)((char*)obj + 0x18);
    if (!arr || IsBadReadPtr(arr, 0x28)) return 0;

    /* SZARRAY: +0x18 = int32 length, +0x20 = data[] */
    int alen = *(int*)((char*)arr + 0x18);
    if (alen <= 0 || alen > 64) return 0;
    if (IsBadReadPtr((char*)arr + 0x20, alen * 8)) return 0;

    void **data = (void**)((char*)arr + 0x20);
    int found_managers = 0;
    int found_chars = 0;

    log_str("  "); log_str(label);
    log_str(".m_managers["); log_u32(alen); log_str("]:\n");

    for (int i = 0; i < alen; i++) {
        void *mgr = data[i];
        if (!mgr || IsBadReadPtr(mgr, 16)) continue;

        /* First try known klass comparison (fast, safe) */
        const char *kname = identify_by_known_klass(mgr);
        if (!kname) {
            /* Fall back to API identification */
            kname = try_identify_object(mgr);
        }

        if (kname) {
            log_str("    ["); log_u32(i); log_str("] ");
            log_str(kname);
            log_str(" @"); log_hex((unsigned long long)mgr);
            log_nl();
            found_managers++;

            /* Check if this is CharBagSystem */
            void *mk = *(void**)mgr;
            if (mk == g_charbag_klass) {
                found_chars += extract_from_charbag(mgr);
            }
        }
    }

    if (found_managers > 0) {
        log_str("  → "); log_u32(found_managers);
        log_str(" managers identified, ");
        log_u32(found_chars); log_str(" chars found\n");
    }
    log_flush();
    return found_chars;
}

/*
 * Probe an object at all pointer-aligned offsets.
 * For each non-null pointer:
 *   1. Try known klass comparison (fast)
 *   2. Try IL2CPP API identification (with safety checks)
 *   3. Try reading as ManagerCollection (m_managers at +24)
 *   4. If depth < max, recursively probe interesting sub-objects
 * Returns count of CharInfo found.
 */
static int identify_and_probe(void *obj, const char *label, int max_offset, int depth) {
    if (!obj || IsBadReadPtr(obj, 16) || depth > 2) return 0;

    log_str("\n--- Probe: "); log_str(label);
    log_str(" @"); log_hex((unsigned long long)obj);
    log_str(" (0x10.."); log_hex(max_offset);
    log_str(") depth="); log_u32(depth);
    log_str(" ---\n");

    int found = 0;

    for (int off = 0x10; off <= max_offset; off += 8) {
        if (IsBadReadPtr((char*)obj + off, 8)) continue;
        void *ptr = *(void**)((char*)obj + off);
        if (!ptr) continue;
        if (IsBadReadPtr(ptr, 16)) continue;

        /* Try known klass first */
        const char *kname = identify_by_known_klass(ptr);
        int is_known = (kname != 0);

        /* If not known, try IL2CPP API identification */
        if (!kname) kname = try_identify_object(ptr);

        log_str("  +"); log_hex(off); log_str(": ");
        if (kname) {
            log_str(kname);
            const char *ns = try_get_namespace(ptr);
            if (ns && ns[0]) { log_str(" ("); log_str(ns); log_str(")"); }
            log_str(" @"); log_hex((unsigned long long)ptr);
        } else {
            log_str("ptr="); log_hex((unsigned long long)ptr);
            void *mk = *(void**)ptr;
            log_str(" klass?="); log_hex((unsigned long long)mk);
        }
        log_nl();

        /* Direct CharBagSystem check */
        void *pk = *(void**)ptr;
        if (pk == g_charbag_klass) {
            found += extract_from_charbag(ptr);
            if (found > 0) return found;
        }

        /* Try reading m_managers from this object (ManagerCollection test) */
        if (kname && (str_contains(kname, "Collection") ||
                      str_contains(kname, "Manager") ||
                      is_known)) {
            char mlabel[64];
            int p = 0;
            const char *lp = label;
            while (*lp && p < 40) mlabel[p++] = *lp++;
            mlabel[p++] = '+';
            /* Write hex offset */
            const char *hc2 = "0123456789ABCDEF";
            int h = off;
            mlabel[p++] = '0'; mlabel[p++] = 'x';
            if (h >= 16) mlabel[p++] = hc2[(h >> 4) & 0xF];
            mlabel[p++] = hc2[h & 0xF];
            mlabel[p] = 0;
            int r = try_read_managers_array(ptr, mlabel);
            if (r > 0) { found += r; return found; }
        }

        /* Always try m_managers blindly (even for unidentified objects) */
        if (!kname || (!str_contains(kname, "Collection") &&
                       !str_contains(kname, "Manager"))) {
            int r = try_read_managers_array(ptr, "blind");
            if (r > 0) { found += r; return found; }
        }

        /* Recursive probe for interesting sub-objects */
        if (depth < 2 && kname && (
            str_contains(kname, "Manager") ||
            str_contains(kname, "Collection") ||
            str_contains(kname, "Context") ||
            str_contains(kname, "Container") ||
            str_contains(kname, "State"))) {
            int r = identify_and_probe(ptr, kname, 0x60, depth + 1);
            if (r > 0) { found += r; return found; }
        }
    }

    log_flush();
    return found;
}

/*
 * Walk class hierarchy via il2cpp_class_get_parent, enumerating fields
 * at each level. This reveals hidden fields from generic base classes.
 */
static void dump_class_hierarchy_fields(void *klass, const char *label) {
    log_str("\n--- Class hierarchy for "); log_str(label); log_str(" ---\n");

    void *cur = klass;
    while (cur) {
        const char *cname = p_class_get_name(cur);
        if (!cname || str_eq(cname, "Object")) break;
        const char *cns = p_class_get_namespace ? p_class_get_namespace(cur) : "";

        log_str("  ["); if (cns && cns[0]) { log_str(cns); log_str("."); }
        log_str(cname); log_str("]\n");

        void *fiter = 0; void *field;
        int fcount = 0;
        while ((field = p_class_get_fields(cur, &fiter)) != 0) {
            const char *fname = p_field_get_name(field);
            int foff = p_field_get_offset(field);
            int is_static = field_is_static(field);
            int type_enum = -1;
            if (p_field_get_type && p_type_get_type) {
                void *ftype = p_field_get_type(field);
                if (ftype) type_enum = p_type_get_type(ftype);
            }

            log_str("    "); if (is_static) log_str("[S] ");
            log_str(fname ? fname : "?");
            log_str(" off="); log_u32(foff);
            if (type_enum >= 0) { log_str(" type="); log_hex(type_enum); }
            log_nl();
            fcount++;
        }
        if (fcount == 0) log_str("    (no fields)\n");

        cur = p_class_get_parent ? p_class_get_parent(cur) : 0;
    }
    log_flush();
}

/*
 * Read raw bytes at a code RVA and log as hex.
 * This lets us reverse-engineer what offsets a function accesses.
 */
static void dump_code_bytes(unsigned long long rva, int count, const char *label) {
    if (!g_ga_base || rva == 0) return;
    char *addr = (char*)g_ga_base + rva;
    if (IsBadReadPtr(addr, count)) return;

    log_str("\n--- Code bytes: "); log_str(label);
    log_str(" (RVA "); log_hex(rva); log_str(") ---\n  ");

    const char *hc = "0123456789ABCDEF";
    for (int i = 0; i < count; i++) {
        unsigned char b = (unsigned char)addr[i];
        if (g_lp < LOG_MAX - 4) {
            g_log[g_lp++] = hc[b >> 4];
            g_log[g_lp++] = hc[b & 0xF];
            g_log[g_lp++] = ' ';
        }
        if ((i + 1) % 16 == 0 && i + 1 < count) {
            log_nl(); log_str("  ");
        }
    }
    log_nl();
    log_flush();
}

static int api_discover_characters(void *domain) {
    log_str("\n=== Phase 2: Object Graph Navigation (v18 service locator scan) ===\n");
    log_flush();

    g_visited_count = 0;
    g_known_count = 0;

    /* ── Pre-resolve target klasses ── */
    g_charbag_klass = find_class(domain, "CharBagSystem", "Beyond.Gameplay", 0);
    if (!g_charbag_klass)
        g_charbag_klass = find_class(domain, "CharBagSystem", 0, 0);
    g_datamgr_klass = find_class(domain, "DataManager", "Beyond.Gameplay", 0);
    g_gamemgr_coll_klass = find_class(domain, "GameManagerCollection", 0, 0);
    g_playermgr_coll_klass = find_class(domain, "PlayerManagerCollection",
                                         "Beyond.Gameplay", 0);

    register_known_klass(g_charbag_klass, "CharBagSystem");
    register_known_klass(g_datamgr_klass, "DataManager");
    register_known_klass(g_gamemgr_coll_klass, "GameManagerCollection");
    register_known_klass(g_playermgr_coll_klass, "PlayerManagerCollection");
    for (int i = 0; i < g_klass_count; i++)
        register_known_klass(g_klass_set[i], "CharInfo/sub");

    void *k;
    k = find_class(domain, "SquadManager", "Beyond.Gameplay", 0);
    register_known_klass(k, "SquadManager");
    k = find_class(domain, "DeckManager", "Beyond.Gameplay", 0);
    register_known_klass(k, "DeckManager");
    k = find_class(domain, "WorldManagerCollection", 0, 0);
    register_known_klass(k, "WorldManagerCollection");
    k = find_class(domain, "ClientDataManager", "Beyond.Gameplay", 0);
    register_known_klass(k, "ClientDataManager");
    k = find_class(domain, "BattleManager", 0, 0);
    register_known_klass(k, "BattleManager");

    log_str("CharBagSystem klass: "); log_hex((unsigned long long)g_charbag_klass); log_nl();
    log_str("DataManager klass: "); log_hex((unsigned long long)g_datamgr_klass); log_nl();
    log_str("GameMgrColl klass: "); log_hex((unsigned long long)g_gamemgr_coll_klass); log_nl();
    log_str("PlayerMgrColl klass: "); log_hex((unsigned long long)g_playermgr_coll_klass); log_nl();
    log_str("Known klasses: "); log_u32(g_known_count); log_nl();

    /* ── Step A: Get singletons ── */
    void *gmc_klass = find_class(domain, "GameplayManagerContext",
                                  "Beyond.Gameplay", 0);
    if (!gmc_klass)
        gmc_klass = find_class(domain, "GameplayManagerContext", 0, 0);

    void *gmc_obj = 0;
    if (gmc_klass) {
        gmc_obj = invoke_static_method(gmc_klass, "get_currentManagerContext");
        log_str("GMC = "); log_hex((unsigned long long)gmc_obj); log_nl();
    }

    void *gameapp_klass = find_class(domain, "GameApp", 0, 0);
    void *gameapp_obj = 0;
    if (gameapp_klass) {
        gameapp_obj = invoke_static_method(gameapp_klass, "get_instance");
        log_str("GameApp = "); log_hex((unsigned long long)gameapp_obj); log_nl();
    }
    log_flush();

    /* ── Step B: Service Locator Scan ── */
    /*
     * The game uses a service locator pattern. Functions at:
     *   service_exists(int id, int flags) → bool   RVA 0x4930510
     *   service_get(int id, int flags)    → void*  RVA 0x49304A0
     * Derived from EarlyUpdate and GetOrCreateManagerContext call sites.
     * Enumerate IDs 0..0x2000 to find all registered services.
     */
    {
        typedef int (*fn_svc_exists)(int, int);
        typedef void* (*fn_svc_get)(int, int);

        fn_svc_exists svc_exists = (fn_svc_exists)((char*)g_ga_base + 0x4930510);
        fn_svc_get svc_get = (fn_svc_get)((char*)g_ga_base + 0x49304A0);

        if (!IsBadReadPtr((void*)svc_exists, 16) && !IsBadReadPtr((void*)svc_get, 16)) {
            log_str("\n--- Service Locator Scan (0x0000..0x1FFF) ---\n");
            int svc_found = 0;

            for (int id = 0; id < 0x2000 && !g_should_exit; id++) {
                if (!svc_exists(id, 0)) continue;
                void *svc = svc_get(id, 0);
                if (!svc) continue;

                /* Log: service ID and instance */
                log_str("  svc["); log_hex(id); log_str("] = ");

                /* Try to identify the object */
                const char *sname = identify_by_known_klass(svc);
                if (!sname) sname = try_identify_object(svc);

                if (sname) {
                    log_str(sname);
                    const char *sns = try_get_namespace(svc);
                    if (sns && sns[0]) { log_str(" ("); log_str(sns); log_str(")"); }
                } else {
                    log_str("?");
                }
                log_str(" @"); log_hex((unsigned long long)svc); log_nl();
                svc_found++;

                /* Check if this IS CharBagSystem directly */
                if (!IsBadReadPtr(svc, 16)) {
                    void *sk = *(void**)svc;
                    if (sk == g_charbag_klass) {
                        log_str("  *** CharBagSystem found via service locator! ***\n");
                        int r = extract_from_charbag(svc);
                        if (r > 0) goto phase2_done;
                    }
                }

                /* Try reading m_managers from this service (ManagerCollection test) */
                int r = try_read_managers_array(svc, "svc");
                if (r > 0) goto phase2_done;
            }

            log_str("Total services found: "); log_u32(svc_found); log_nl();
            log_flush();
        } else {
            log_str("Service locator functions not readable, skipping scan\n");
            log_flush();
        }
    }

    /* ── Step C: Identify & probe GMC object ── */
    if (g_char_count == 0 && gmc_obj && !IsBadReadPtr(gmc_obj, 16)) {
        int r = identify_and_probe(gmc_obj, "GMC", 0x80, 0);
        if (r > 0) goto phase2_done;
    }

    /* ── Step D: Identify & probe GameApp ── */
    if (g_char_count == 0 && gameapp_obj && !IsBadReadPtr(gameapp_obj, 32)) {
        int r = identify_and_probe(gameapp_obj, "GameApp", 0x80, 0);
        if (r > 0) goto phase2_done;
    }

phase2_done:
    log_str("\nPhase 2 complete. ");
    log_u32(g_char_count); log_str(" CharInfo objects found\n");
    log_flush();

    return g_char_count;
}

/* ──────────────────────────────────────────────────────────────────────── */
/*  il2cpp_dump.json generation (same as v6)                               */
/* ──────────────────────────────────────────────────────────────────────── */

static int is_target_class(const char *name) {
    return str_eq(name, "CharInfo") ||
           str_eq(name, "Attributes") ||
           str_eq(name, "AbilitySystem") ||
           str_eq(name, "AbilitySystemData") ||
           str_eq(name, "DataManager") ||
           str_eq(name, "CharacterManager") ||
           str_eq(name, "CharacterFactory") ||
           str_eq(name, "RemoteFactory") ||
           str_eq(name, "AttributesCalculator") ||
           str_eq(name, "CharacterTemplateData") ||
           str_eq(name, "CharacterAttributeData") ||
           str_eq(name, "CharacterAttributeTable") ||
           str_eq(name, "CharacterAttributeKeyFrame") ||
           str_eq(name, "PlayerData") ||
           str_eq(name, "DeckManager") ||
           str_eq(name, "BattleCharacter") ||
           str_eq(name, "BattleManager") ||
           str_eq(name, "BaseManager") ||
           str_eq(name, "BaseFrameworkManager") ||
           str_eq(name, "BaseGameManager") ||
           str_eq(name, "GameManagerCollection") ||
           str_eq(name, "WorldManagerCollection") ||
           str_eq(name, "ClientDataManager") ||
           str_eq(name, "ServerCharInfo") ||
           str_eq(name, "ClientCharInfo") ||
           str_eq(name, "GameApp") ||
           str_eq(name, "GameplayManagerContext") ||
           str_eq(name, "GameplayRootContainer") ||
           str_eq(name, "ManagerCollectionBase") ||
           str_eq(name, "HGManagerContextBase") ||
           str_eq(name, "CharBagSystem") ||
           str_eq(name, "SquadManager") ||
           str_eq(name, "PlayerManagerCollection") ||
           str_contains(name, "CharacterAttribute") ||
           str_contains(name, "StatData") ||
           str_contains(name, "CombatStat") ||
           str_contains(name, "Singleton");
}

static void write_dump_json(void *domain) {
    buf_init();
    if (!g_buf) return;

    unsigned long long asm_count = 0;
    void **assemblies = p_domain_get_assemblies(domain, &asm_count);
    if (!assemblies) { buf_free(); return; }

    unsigned long total_classes = 0, total_methods = 0, total_fields = 0;
    unsigned long matched_count = 0;

    for (unsigned long long ai = 0; ai < asm_count; ai++) {
        if (!assemblies[ai]) continue;
        void *image = p_assembly_get_image(assemblies[ai]);
        if (!image) continue;
        unsigned long long cc = p_image_get_class_count(image);
        total_classes += (unsigned long)cc;
        for (unsigned long long ci = 0; ci < cc; ci++) {
            void *klass = p_image_get_class(image, ci);
            if (!klass) continue;
            const char *cname = p_class_get_name(klass);
            if (cname && is_target_class(cname)) matched_count++;
            void *miter = 0; void *method;
            while ((method = p_class_get_methods(klass, &miter)) != 0) total_methods++;
            void *fiter = 0; void *field;
            while ((field = p_class_get_fields(klass, &fiter)) != 0) total_fields++;
        }
    }

    b_ch('{');
    b_key("meta"); b_ch('{');
    b_key("timestamp"); b_u64(GetTickCount()); b_ch(',');
    b_key("processId"); b_i32(0); b_ch(',');
    b_key("arch"); b_str("x64"); b_ch(',');
    b_key("gameAssemblyBase"); b_hex_str((unsigned long long)g_ga_base); b_ch(',');
    b_key("gameAssemblySize"); b_i32(0); b_ch(',');
    b_key("totalAssemblies"); b_u64(asm_count); b_ch(',');
    b_key("totalClasses"); b_u64(total_classes); b_ch(',');
    b_key("totalMethods"); b_u64(total_methods); b_ch(',');
    b_key("totalFields"); b_u64(total_fields); b_ch(',');
    b_key("matchedClasses"); b_u64(matched_count); b_ch(',');
    b_key("signatureHits"); b_i32(0); b_ch(',');
    b_key("source"); b_str("qavif_dll_v18");
    b_ch('}'); b_ch(',');

    b_key("matched"); b_ch('[');
    int m_first = 1;

    for (unsigned long long ai = 0; ai < asm_count; ai++) {
        if (!assemblies[ai]) continue;
        void *image = p_assembly_get_image(assemblies[ai]);
        if (!image) continue;
        const char *iname = p_image_get_name ? p_image_get_name(image) : "";
        unsigned long long cc = p_image_get_class_count(image);

        for (unsigned long long ci = 0; ci < cc; ci++) {
            if (g_bp > BUF_MAX - 4096) goto dump_done;

            void *klass = p_image_get_class(image, ci);
            if (!klass) continue;
            const char *cname = p_class_get_name(klass);
            if (!cname || !is_target_class(cname)) continue;

            const char *cns = p_class_get_namespace ? p_class_get_namespace(klass) : "";
            const char *parent_name = "";
            if (p_class_get_parent) {
                void *parent = p_class_get_parent(klass);
                if (parent) parent_name = p_class_get_name(parent);
                if (!parent_name) parent_name = "";
            }

            b_sep(&m_first);
            b_ch('{');
            b_key("name"); b_str(cname); b_ch(',');
            b_key("namespace"); b_str(cns ? cns : ""); b_ch(',');
            b_key("image"); b_str(iname); b_ch(',');
            b_key("parent"); b_str(parent_name); b_ch(',');

            b_key("fields"); b_ch('[');
            int ff = 1;
            void *fiter = 0; void *field;
            int num_fields = 0;
            while ((field = p_class_get_fields(klass, &fiter)) != 0) {
                const char *fname = p_field_get_name(field);
                if (!fname) continue;
                int foff = p_field_get_offset(field);
                int is_static = field_is_static(field);

                b_sep(&ff);
                b_ch('{');
                b_key("name"); b_str(fname); b_ch(',');
                b_key("offset"); b_i32(foff); b_ch(',');
                b_key("typeEnum"); b_i32(0); b_ch(',');
                b_key("typeName"); b_str(""); b_ch(',');
                b_key("isStatic"); b_bool(is_static); b_ch(',');
                b_key("isNumeric"); b_bool(0);
                b_ch('}');
                num_fields++;
            }
            b_ch(']'); b_ch(',');

            b_key("methods"); b_ch('[');
            int mf = 1;
            void *miter = 0; void *method;
            while ((method = p_class_get_methods(klass, &miter)) != 0) {
                const char *mname = p_method_get_name(method);
                if (!mname) continue;
                int pcnt = p_method_get_param_count(method);
                void *mptr = get_method_pointer(method);
                unsigned long long rva = mptr ? (unsigned long long)((char*)mptr - (char*)g_ga_base) : 0;

                b_sep(&mf);
                b_ch('{');
                b_key("name"); b_str(mname); b_ch(',');
                b_key("paramCount"); b_i32(pcnt); b_ch(',');
                b_key("rva"); b_u64(rva); b_ch(',');
                b_key("rvaHex"); b_hex_str(rva); b_ch(',');
                b_key("returnType"); b_str("");
                b_ch('}');
            }
            b_ch(']'); b_ch(',');

            b_key("numericFieldCount"); b_i32(num_fields); b_ch(',');
            b_key("matchTier"); b_i32(1); b_ch(',');
            b_key("matchReason"); b_str("target_class"); b_ch(',');
            b_key("isSignatureHit"); b_bool(0);
            b_ch('}');
        }
    }
dump_done:
    b_ch(']'); b_ch(',');
    b_key("signatureHits"); b_raw("[]"); b_ch(',');

    b_key("classIndex"); b_ch('[');
    int ci_first = 1;
    for (unsigned long long ai = 0; ai < asm_count; ai++) {
        if (!assemblies[ai]) continue;
        void *image = p_assembly_get_image(assemblies[ai]);
        if (!image) continue;
        const char *iname = p_image_get_name ? p_image_get_name(image) : "";
        unsigned long long cc = p_image_get_class_count(image);

        for (unsigned long long ci = 0; ci < cc; ci++) {
            if (g_bp > BUF_MAX - 512) goto index_done;
            void *klass = p_image_get_class(image, ci);
            if (!klass) continue;
            const char *cname = p_class_get_name(klass);
            if (!cname) continue;
            const char *cns = p_class_get_namespace ? p_class_get_namespace(klass) : "";

            b_sep(&ci_first);
            b_ch('{');
            b_key("name"); b_str(cname); b_ch(',');
            b_key("namespace"); b_str(cns ? cns : ""); b_ch(',');
            b_key("image"); b_str(iname);
            b_ch('}');
        }
    }
index_done:
    b_ch(']');
    b_ch('}');

    buf_write_to(DUMP_PATH);
    buf_free();
}

/* ──────────────────────────────────────────────────────────────────────── */
/*  JSONL event output (same as v6)                                        */
/* ──────────────────────────────────────────────────────────────────────── */

static void write_char_event(CharData *cd) {
    buf_init();
    if (!g_buf) return;

    b_ch('{');
    b_key("type"); b_str("FULL_CHARACTER_CAPTURE"); b_ch(',');
    b_key("templateId"); b_str(cd->template_id); b_ch(',');
    b_key("level"); b_i32(cd->level); b_ch(',');
    b_key("source"); b_str("dll_v18_svc_locator"); b_ch(',');
    b_key("identificationMethod"); b_str("klass_match"); b_ch(',');
    b_key("attrPtr"); b_hex_str((unsigned long long)cd->attrs_ptr); b_ch(',');

    b_key("rawData"); b_ch('[');
    if (cd->has_raw_data) {
        for (int i = 0; i < cd->raw_data_len && i < 128; i++) {
            if (i > 0) b_ch(',');
            b_double(cd->raw_data[i]);
        }
    }
    b_ch(']'); b_ch(',');

    b_key("namedAttrs"); b_ch('{');
    if (cd->has_raw_data) {
        int limit = cd->raw_data_len < NUM_ATTRS ? cd->raw_data_len : NUM_ATTRS;
        int first = 1;
        for (int i = 0; i < limit; i++) {
            b_sep(&first); b_key(ATTR_NAMES[i]); b_double(cd->raw_data[i]);
        }
    }
    b_ch('}'); b_ch(',');

    b_key("finalCachedData"); b_ch('{');
    if (cd->has_get_values) {
        b_key("data"); b_ch('[');
        for (int i = 0; i < NUM_ATTRS; i++) {
            if (i > 0) b_ch(',');
            b_double(cd->get_values[i]);
        }
        b_ch(']'); b_ch(',');
        b_key("namedAttrs"); b_ch('{');
        int first = 1;
        for (int i = 0; i < NUM_ATTRS; i++) {
            b_sep(&first); b_key(ATTR_NAMES[i]); b_double(cd->get_values[i]);
        }
        b_ch('}');
    } else {
        b_key("data"); b_raw("[]"); b_ch(',');
        b_key("namedAttrs"); b_raw("{}");
    }
    b_ch('}'); b_ch(',');

    b_key("scalars"); b_ch('{');
    if (cd->has_scalars) {
        int first = 1;
        for (int i = 0; SCALARS[i].name; i++) {
            b_sep(&first); b_key(SCALARS[i].name); b_double(cd->scalars[i]);
        }
    }
    b_ch('}'); b_ch(',');

    b_key("mainAttrType"); b_raw("null"); b_ch(',');
    b_key("subAttrType"); b_raw("null"); b_ch(',');
    b_key("breakStage"); b_i32(cd->break_stage); b_ch(',');
    b_key("exp"); b_i32(cd->exp); b_ch(',');
    b_key("potential"); b_u64(cd->potential); b_ch(',');
    b_key("timestamp"); b_u64(GetTickCount());
    b_ch('}'); b_ch('\n');

    g_buf[g_bp] = '\0';
    append_file(EVENTS_PATH, g_buf, g_bp);
    buf_free();
}

static void write_all_events(void) {
    write_file(EVENTS_PATH, "", 0);
    for (int i = 0; i < g_char_count; i++)
        write_char_event(&g_chars[i]);
}

/* ──────────────────────────────────────────────────────────────────────── */
/*  Capture Thread                                                         */
/* ──────────────────────────────────────────────────────────────────────── */

static unsigned long __stdcall capture_thread(void *param) {
    g_lp = 0;
    g_char_count = 0;

    write_marker("qavif_entry.txt", "v18 service locator scan");
    log_str("=== IL2CPP Stat Capture v18 (service locator scan) ===\n");

    /* ── Poll for GameAssembly.dll (every 2s, up to 120s) ── */
    log_str("Polling for GameAssembly.dll...\n");
    log_flush();
    void *ga = 0;
    for (int i = 0; i < 60 && !g_should_exit; i++) {
        ga = GetModuleHandleA("GameAssembly.dll");
        if (ga) break;
        Sleep(2000);
    }
    if (!ga) {
        log_str("FATAL: GameAssembly.dll not found after 120s\n");
        log_flush();
        write_marker("qavif_FAIL.txt", "GameAssembly.dll not found");
        return 1;
    }
    g_ga_base = ga;
    log_str("GA base: "); log_hex((unsigned long long)ga); log_nl();

    /* ── Resolve IL2CPP API ── */
    log_str("Resolving IL2CPP API...\n");
    resolve_api(ga);
    if (!p_domain_get || !p_thread_attach || !p_domain_get_assemblies ||
        !p_assembly_get_image || !p_image_get_class_count ||
        !p_image_get_class || !p_class_get_name) {
        log_str("FATAL: Critical API missing\n");
        log_flush();
        write_marker("qavif_FAIL.txt", "IL2CPP API missing");
        return 1;
    }

    /* ── Wait for game init (60s — longer for API nav since we need managers loaded) ── */
    log_str("Waiting 60s for game init...\n");
    log_flush();
    for (int i = 0; i < 30 && !g_should_exit; i++) Sleep(2000);
    if (g_should_exit) return 0;

    /* ── Attach to IL2CPP domain ── */
    void *domain = p_domain_get();
    if (!domain) {
        log_str("FATAL: domain NULL\n"); log_flush();
        write_marker("qavif_FAIL.txt", "domain NULL");
        return 1;
    }
    p_thread_attach(domain);
    log_str("Domain attached\n");
    log_flush();

    /* ── Phase 1: il2cpp_dump.json ── */
    log_str("\n=== Phase 1: Generating il2cpp_dump.json ===\n");
    log_flush();
    write_dump_json(domain);
    log_str("il2cpp_dump.json written\n");
    write_marker("qavif_stage1_dump.txt", "il2cpp_dump.json complete");
    log_flush();

    /* ── Find CharInfo class + all subclasses ── */
    void *charinfo_klass = find_class(domain, "CharInfo", "Beyond.Gameplay",
                                       "Gameplay.Beyond.dll");
    if (!charinfo_klass)
        charinfo_klass = find_class(domain, "CharInfo", 0, 0);
    g_charinfo_klass = charinfo_klass;
    g_klass_count = 0;

    if (charinfo_klass) {
        g_klass_set[g_klass_count++] = charinfo_klass;
        log_str("CharInfo klass: "); log_hex((unsigned long long)charinfo_klass); log_nl();

        /* Find ALL subclasses of CharInfo */
        log_str("Searching for CharInfo subclasses...\n");
        unsigned long long asm_count2 = 0;
        void **assemblies2 = p_domain_get_assemblies(domain, &asm_count2);
        if (assemblies2 && p_class_get_parent) {
            for (unsigned long long ai = 0; ai < asm_count2; ai++) {
                if (!assemblies2[ai]) continue;
                void *image = p_assembly_get_image(assemblies2[ai]);
                if (!image) continue;
                unsigned long long cc = p_image_get_class_count(image);
                for (unsigned long long ci = 0; ci < cc && g_klass_count < MAX_KLASS; ci++) {
                    void *klass = p_image_get_class(image, ci);
                    if (!klass) continue;
                    void *parent = p_class_get_parent(klass);
                    if (parent == charinfo_klass && klass != charinfo_klass) {
                        g_klass_set[g_klass_count++] = klass;
                        const char *subname = p_class_get_name(klass);
                        const char *subns = p_class_get_namespace ? p_class_get_namespace(klass) : "";
                        log_str("  SUBCLASS: ");
                        if (subns && subns[0]) { log_str(subns); log_str("."); }
                        log_str(subname ? subname : "?");
                        log_str(" klass="); log_hex((unsigned long long)klass); log_nl();
                    }
                }
            }
        }
        log_str("Total klass set: "); log_u32(g_klass_count); log_nl();
    } else {
        log_str("CharInfo class NOT FOUND\n");
    }
    log_flush();

    /* ── Phase 2: API-based object graph navigation ── */
    api_discover_characters(domain);

    /* ── Phase 2 retry: if nothing found, wait 90s and try again ── */
    if (g_char_count == 0 && !g_should_exit) {
        log_str("\n=== No chars yet. Waiting 90s for retry... ===\n");
        log_str(">>> Navigate to character screen if possible <<<\n");
        write_marker("qavif_WAITING.txt",
            "No chars found yet. Navigate to character screen, then wait 90s.");
        log_flush();
        for (int i = 0; i < 45 && !g_should_exit; i++) Sleep(2000);

        if (!g_should_exit) {
            log_str("\n=== Retry: API navigation ===\n");
            log_flush();
            g_visited_count = 0; /* reset visited set */
            api_discover_characters(domain);
        }
    }
    log_flush();

    /* ── Phase 3: Write results ── */
    if (g_char_count > 0) {
        log_str("\n=== Phase 3: Writing il2cpp_char_events.jsonl ===\n");
        write_all_events();
        log_str("Written "); log_u32(g_char_count);
        log_str(" FULL_CHARACTER_CAPTURE events\n\n");
        for (int i = 0; i < g_char_count; i++) {
            log_str("  "); log_str(g_chars[i].template_id);
            log_str(" lv="); log_u32(g_chars[i].level);
            if (g_chars[i].has_raw_data) {
                log_str(" rawData["); log_u32(g_chars[i].raw_data_len); log_str("]");
            }
            if (g_chars[i].has_get_values) log_str(" +GetValue");
            if (g_chars[i].has_scalars) log_str(" +scalars");
            log_nl();
        }
        write_marker("qavif_DONE.txt", "capture complete");
    } else {
        log_str("\nNo CharInfo objects captured.\n");
        log_str("il2cpp_dump.json written with class metadata.\n");
        log_str("Check log for object graph exploration results.\n");
        write_marker("qavif_DONE.txt", "navigation complete, no char data");
    }

    log_flush();
    return 0;
}

/* ── DLL Entry Point ──────────────────────────────────────────────────── */

static volatile int g_started = 0;

int __stdcall DllEntry(void *hinstDLL, unsigned long fdwReason, void *lpvReserved) {
    if (fdwReason == DLL_PROCESS_ATTACH) {
        if (g_started) return 1;
        g_started = 1;
        CreateThread(0, 0, capture_thread, 0, 0, 0);
    }
    if (fdwReason == DLL_PROCESS_DETACH) {
        g_should_exit = 1;
    }
    return 1;
}
