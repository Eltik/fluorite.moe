"""Prototype decoder for Endfield's MemoryPack JsonData, driven by the IL2CPP type table."""
import json, os, re, struct, sys
sys.path.insert(0, os.path.dirname(__file__)); import pool


class DecodeError(Exception):
    pass


TRACE = bool(os.environ.get('MP_TRACE'))
PRIMS = {'bool': '<?', 'byte': '<B', 'sbyte': '<b', 'short': '<h', 'ushort': '<H', 'int': '<i', 'uint': '<I',
         'long': '<q', 'ulong': '<Q', 'float': '<f', 'double': '<d', 'char': '<H'}
SIZES = {k: struct.calcsize(v) for k, v in PRIMS.items()}
LISTS = ('List', 'HashSet', 'Queue', 'Stack', 'LinkedList', 'IList', 'ICollection', 'IEnumerable', 'IReadOnlyList',
         'IReadOnlyCollection', 'SortedSet', 'ISet')
COMMON_USINGS = ('UnityEngine', 'System', 'System.Collections.Generic', 'Unity.Mathematics')
HEADED_DICTS = ('SerializeFieldDictionary', 'SerializeFieldDictionaryPaired', 'SerializeFieldDictionarySorted')
DICTS = ('Dictionary', 'SortedDictionary', 'IDictionary', 'IReadOnlyDictionary', 'ConcurrentDictionary')


def normalize(t):
    return re.sub(r'\s+', '', t)


def split_generic(t):
    """'Dictionary<ulong, List<int>>' -> ('Dictionary', ['ulong', 'List<int>'])."""
    i = t.find('<')
    if i < 0 or not t.endswith('>'):
        return t, []
    args, depth, cur = [], 0, ''
    for ch in t[i + 1:-1]:
        if ch == ',' and depth == 0:
            args.append(cur.strip()); cur = ''; continue
        depth += ch == '<'; depth -= ch == '>'; cur += ch
    args.append(cur.strip())
    return t[:i], args


class Schema:
    def __init__(self, types):
        self.types = types
        self.by_last = {}
        for full, t in types.items():
            self.by_last.setdefault(t['name'].split('.')[-1], []).append(full)
        munged = {}
        for full in types:
            munged.setdefault(full.replace('.', '_'), full)
        # Wrapper `Beyond_Gameplay_Core_BuffDataForMemoryPack` mirrors `Beyond.Gameplay.Core.BuffData`.
        self.wrapper_of, self.orig_of, self.generic_wrappers = {}, {}, {}
        for full, t in types.items():
            if not (t['kind'] == 'class' and t['name'].endswith('ForMemoryPack') and t['ns'] == 'Beyond.MemoryPack'):
                continue
            arg = next((split_generic(b)[1][0] for b in t['bases']
                        if split_generic(b)[0] == 'IMemoryPackDeSerializeWrapper' and split_generic(b)[1]), None)
            orig = munged.get(t['name'].removesuffix('ForMemoryPack'))
            if orig is None and arg and '<' in arg:
                # Closed generic, e.g. BlackboardParamBase<int, int>: members carry concrete types.
                gbase = split_generic(arg)[0]
                gdefs = [f for f in self.by_last.get(gbase.split('.')[-1], []) if split_generic(self.types[f]['name'])[0].split('.')[-1] == gbase.split('.')[-1]]
                self.generic_wrappers[normalize(arg)] = full
                self.orig_of[full] = gdefs[0] if gdefs else ''
                continue
            if orig is None and arg:
                cands = [f for f in self.by_last.get(arg.split('.')[-1], []) if self.types[f]['kind'] != 'interface']
                stem = t['name'].removesuffix('ForMemoryPack')
                orig = next((f for f in cands if stem.endswith(f.replace('.', '_'))), cands[0] if len(cands) == 1 else None)
            if orig:
                self.wrapper_of[orig] = full; self.orig_of[full] = orig
        self._members, self._unmanaged, self._resolved = {}, {}, {}
        self.generic_defs = {}
        for full, t in types.items():
            gbase, gparams = split_generic(t['name'].split('.')[-1])
            if gparams:
                self.generic_defs.setdefault((gbase, len(gparams)), []).append(full)

    def resolve(self, t, ctx=''):
        """Full name of type t as referenced from type ctx: nested types first, then the nearest namespace."""
        key = (t, ctx)
        if key in self._resolved:
            return self._resolved[key]
        outers = []
        c = ctx
        while c in self.types:
            outers.append(c)
            c = c.rsplit('.', 1)[0] if '.' in self.types[c]['name'] else None
        # A type nested in some other class is only visible by its short name from inside it.
        cands = [f for f in self.by_last.get(t.split('.')[-1], [])
                 if self.types[f]['name'] == t
                 or (self.types[f]['name'].endswith('.' + t) and f[:-len(t) - 1] in outers)]
        best = None
        if len(cands) == 1:
            best = cands[0]
        elif cands:
            cns = self.types[ctx]['ns'].split('.') if ctx in self.types else []
            outers = []
            c = ctx
            while c in self.types:
                outers.append(c)
                c = c.rsplit('.', 1)[0] if '.' in self.types[c]['name'] else None
            def rank(f):
                # C# lookup: nested types, then the referencing namespace and its ancestors, then
                # `using` imports (unknown here; UnityEngine/System are the usual ones).
                nested = any(f.startswith(o + '.') for o in outers)
                ns = self.types[f]['ns'].split('.') if self.types[f]['ns'] else []
                ancestor = len(ns) if ns == cns[:len(ns)] else -1
                common_using = self.types[f]['ns'] in COMMON_USINGS
                common = 0
                for a, b in zip(ns, cns):
                    if a != b: break
                    common += 1
                return (nested, ancestor, common_using, common, self.types[f]['name'] == t, -len(f))
            best = max(cands, key=rank)
        self._resolved[key] = best
        return best

    def members(self, wfull):
        if wfull in self._members:
            return self._members[wfull]
        t = self.types[wfull]
        out = []
        for b in t['bases']:
            if b.endswith('ForMemoryPack'):
                base = next((f for f in self.by_last.get(b, []) if f in self.orig_of), None)
                if base:
                    out += self.members(base)
        out += [(p['name'][2:-2], p['type'], '[MemoryPackAllowSerialize]' in p['attrs']) for p in t['props']
                if p['acc'] == ['set;'] and p['name'].startswith('__') and p['name'].endswith('__')]
        self._members[wfull] = out
        return out

    def unions(self, wfull):
        out = {}
        for a in self.types[wfull]['attrs']:
            m = re.match(r'\[MemoryPackUnion\((\d+), typeof\((\w+)\)\)\]', a)
            if m:
                w = next((f for f in self.by_last.get(m[2], []) if f in self.orig_of), None)
                if w is None:
                    raise DecodeError(f'union member {m[2]} has no wrapper')
                out[int(m[1])] = w
        return out

    def size_align(self, full):
        t = self.types[full]
        if t['kind'] == 'enum':
            ut = next((f['type'] for f in t['fields'] if f['name'] == 'value__'), 'int')
            return SIZES[ut], SIZES[ut]
        lay = self.unmanaged(full)
        return (lay[0], lay[2]) if lay else None

    def blittable(self, t, ctx):
        """(size, align) of a blittable type reference, or None."""
        if t in SIZES:
            return SIZES[t], SIZES[t]
        base, args = split_generic(t)
        if args:
            lay = self.generic_struct(base, args, ctx)
            return (lay[0], lay[2]) if lay else None
        f = self.resolve(t, ctx)
        if not f or self.types[f]['kind'] not in ('struct', 'enum'):
            return None
        return self.size_align(f)

    def generic_struct(self, base, args, ctx):
        """Raw layout of a closed generic struct such as Optional<Vector3>, using natural alignment."""
        cands = [f for f in self.generic_defs.get((base.split('.')[-1], len(args)), [])]
        if not cands:
            return None
        full = cands[0] if len(cands) == 1 else max(cands, key=lambda f: self.types[f]['ns'] == (self.types[ctx]['ns'] if ctx in self.types else ''))
        t = self.types[full]
        if t['kind'] != 'struct':
            return None
        params = split_generic(t['name'])[1]
        subst = dict(zip(params, args))
        off, align, lay = 0, 1, []
        for f in t['fields']:
            if 'static' in f['mods'] or 'const' in f['mods']:
                continue
            ft = subst.get(f['type'], f['type'])
            sa = self.blittable(ft, full)
            if sa is None:
                return None
            off = (off + sa[1] - 1) // sa[1] * sa[1]
            lay.append((off, f['name'], ft))
            off += sa[0]; align = max(align, sa[1])
        if not lay:
            return None
        return ((off + align - 1) // align * align, lay, align)

    def unmanaged(self, full, stack=()):
        """(size, [(offset, name, type)], align) for a blittable struct, else None."""
        if full in self._unmanaged:
            return self._unmanaged[full]
        if full in stack:
            return None
        t = self.types[full]
        res = None
        fields = [f for f in t['fields'] if 'static' not in f['mods'] and 'const' not in f['mods']]
        if t['kind'] == 'struct' and fields and all(f['off'] for f in fields):
            lay, end, align = [], 0, 1
            for f in fields:
                if f['type'] in SIZES:
                    sa = (SIZES[f['type']], SIZES[f['type']])
                else:
                    ff = self.resolve(f['type'], full)
                    if not ff or self.types[ff]['kind'] not in ('struct', 'enum'):
                        lay = None; break
                    sub = self.unmanaged(ff, stack + (full,)) if self.types[ff]['kind'] == 'struct' else None
                    sa = (sub[0], sub[2]) if sub else self.size_align(ff) if self.types[ff]['kind'] == 'enum' else None
                    if sa is None:
                        lay = None; break
                off = int(f['off'], 16)
                lay.append((off, f['name'], f['type']))
                end, align = max(end, off + sa[0]), max(align, sa[1])
            if lay is not None:
                # The IL2CPP instance size is authoritative (explicit Pack values are not in the dump).
                size = t.get('size', (end + align - 1) // align * align)
                res = (size, lay, align)
        self._unmanaged[full] = res
        return res


def _curve(r):
    n = r.take('<B')[0]
    if n == 255:
        return None
    post, pre = r.take('<ii')
    count = r.count()
    keys = None if count is None else [dict(zip(('time', 'value', 'inTangent', 'outTangent', 'weightedMode', 'inWeight', 'outWeight'),
                                                r.take('<ffffiff'))) for _ in range(count)]
    return {'postWrapMode': post, 'preWrapMode': pre, 'keys': keys}


def _gradient(r):
    n = r.take('<B')[0]
    if n == 255:
        return None
    c = r.count()
    colors = None if c is None else [dict(zip(('r', 'g', 'b', 'a', 'time'), r.take('<fffff'))) for _ in range(c)]
    a = r.count()
    alphas = None if a is None else [dict(zip(('alpha', 'time'), r.take('<ff'))) for _ in range(a)]
    return {'colorKeys': colors, 'alphaKeys': alphas, 'mode': r.take('<i')[0]}


def _rect_offset(r):
    n = r.take('<B')[0]
    return None if n == 255 else dict(zip(('left', 'right', 'top', 'bottom'), r.take('<iiii')))


# Types with hand-written MemoryPack formatters (MemoryPack's Unity extensions).
SPECIAL = {'AnimationCurve': _curve, 'Gradient': _gradient, 'RectOffset': _rect_offset}


class Reader:
    def __init__(self, data, schema, strings):
        self.b, self.p, self.s, self.strings = data, 0, schema, strings

    def take(self, fmt):
        n = struct.calcsize(fmt)
        if self.p + n > len(self.b):
            raise DecodeError(f'eof at {self.p} reading {fmt}')
        v = struct.unpack_from(fmt, self.b, self.p); self.p += n
        return v

    def i32(self):
        return self.take('<i')[0]

    def count(self):
        n = self.i32()
        if n == -1:
            return None
        if n < 0 or n > len(self.b) - self.p:
            raise DecodeError(f'bad count {n} at {self.p - 4}')
        return n

    def string(self):
        i = self.i32()
        if i == -1:
            return None
        if i == -2:
            return ''
        if not 0 <= i < len(self.strings):
            raise DecodeError(f'string index {i} out of range at {self.p - 4}')
        return self.strings[i]

    def value(self, t, ctx, depth=0, raw_ok=False):
        if depth > 96:
            raise DecodeError('too deep')
        t = t.strip()
        if t.endswith('[]'):
            n = self.count()
            # T[] of a blittable T goes through MemoryPack's raw array formatter.
            return None if n is None else [self.value(t[:-2], ctx, depth + 1, raw_ok=True) for _ in range(n)]
        if t in PRIMS:
            return self.take(PRIMS[t])[0]
        if t == 'string':
            return self.string()
        if t == 'StringPathHash':
            # StringPathHashFormatter writes only the 64-bit path hash.
            return self.take('<q')[0]
        base, args = split_generic(t)
        if base in LISTS:
            n = self.count()
            return None if n is None else [self.value(args[0], ctx, depth + 1) for _ in range(n)]
        if base in HEADED_DICTS:
            # Beyond's SerializeFieldDictionary formatters put an object header before the entries.
            if self.take('<B')[0] == 255:
                return None
            base = 'Dictionary'
        if base in DICTS:
            n = self.count()
            if n is None:
                return None
            pairs = []
            for _ in range(n):
                k = self.value(args[0], ctx, depth + 1)
                pairs.append([k, self.value(args[1], ctx, depth + 1)])
            keys = [str(k) for k, _ in pairs]
            if all(isinstance(k, (str, int)) for k, _ in pairs) and len(set(keys)) == len(keys):
                return dict(zip(keys, (v for _, v in pairs)))
            return pairs
        if raw_ok and args:
            lay = self.s.generic_struct(base, args, ctx)
            if lay:
                start, out = self.p, {}
                size, fields, _ = lay
                if self.p + size > len(self.b):
                    raise DecodeError(f'eof reading struct {t} at {self.p}')
                for off, name, ft in fields:
                    self.p = start + off; out[name] = self.value(ft, ctx, depth + 1, raw_ok=True)
                self.p = start + size
                return out
        special = SPECIAL.get(t.split('.')[-1])
        if special:
            return special(self)
        gw = self.s.generic_wrappers.get(normalize(t))
        if gw:
            return self.object(gw, depth)
        if base == 'Nullable' or t.endswith('?'):
            inner = args[0] if args else t[:-1]
            if inner in SIZES:
                size = SIZES[inner]
            else:
                f = self.s.resolve(inner, ctx); lay = f and self.s.unmanaged(f)
                if not lay:
                    raise DecodeError(f'nullable {t} of non-blittable type at {self.p}')
                size = lay[2]
            # Unmanaged Nullable<T> is copied raw: hasValue, padding to T's alignment, then T.
            start = self.p
            has = self.take('<?')[0]
            self.p = start + size
            v = self.value(inner, ctx, depth + 1, raw_ok=True)
            return v if has else None
        full = self.s.resolve(t, ctx)
        if not full:
            raise DecodeError(f'unknown type {t!r} (from {ctx}) at {self.p}')
        kind = self.s.types[full]['kind']
        if kind == 'enum':
            ut = next((f['type'] for f in self.s.types[full]['fields'] if f['name'] == 'value__'), 'int')
            return self.take(PRIMS[ut])[0]
        w = self.s.wrapper_of.get(full)
        if kind == 'struct' and (raw_ok or not w):
            lay = self.s.unmanaged(full)
            if lay:
                size, fields, _ = lay
                if self.p + size > len(self.b):
                    raise DecodeError(f'eof reading struct {t} at {self.p}')
                start, out = self.p, {}
                for off, name, ft in fields:
                    self.p = start + off; out[name] = self.value(ft, full, depth + 1, raw_ok=True)
                self.p = start + size
                return out
        if w:
            return self.object(w, depth)
        raise DecodeError(f'unsupported type {t!r} ({full}, {kind}) at {self.p}')

    def object(self, wfull, depth):
        un = self.s.unions(wfull)
        if un:
            tag = self.take('<B')[0]
            if tag == 255:
                return None
            if tag == 250:
                # Wide tag: unions with 250+ cases follow the marker with a ushort.
                tag = self.take('<H')[0]
            if tag not in un:
                raise DecodeError(f'unknown union tag {tag} for {wfull} at {self.p - 1}')
            v = self.object(un[tag], depth + 1)
            if isinstance(v, dict):
                o = self.s.orig_of[un[tag]]
                v = {'$type': self.s.types[o]['name'] if o else self.s.types[un[tag]]['name']} | v
            return v
        n = self.take('<B')[0]
        if n == 255:
            return None
        members = self.s.members(wfull)
        if n != len(members):
            raise DecodeError(f'{wfull}: header {n} != {len(members)} members at {self.p - 1}')
        ctx = self.s.orig_of[wfull]
        out = {}
        for name, mt, allow in members:
            if TRACE:
                print('  ' * depth + f'{self.p:6} {name}: {mt}' + (' [raw]' if allow else ''))
            out[name] = self.value(mt, ctx, depth + 1, raw_ok=allow)
        return out


def load_strings(pool_dir):
    out = []
    for k in range(64):
        path = os.path.join(pool_dir, f'StringPool{k}.byte')
        if not os.path.exists(path):
            break
        _, raw = pool.read_block(path, k == 0)
        n = struct.unpack_from('<i', raw, 0)[0]
        offs = struct.unpack_from(f'<{n}i', raw, 4)
        base = 4 + 4 * n
        for o in offs:
            L = struct.unpack_from('<i', raw, base + o)[0]
            out.append(raw[base + o + 4:base + o + 4 + L].decode())
    return out


def decode(data, root_full, schema, strings):
    r = Reader(data, schema, strings)
    w = schema.wrapper_of.get(root_full)
    if not w:
        raise DecodeError(f'no wrapper for {root_full}')
    v = r.object(w, 0)
    if r.p != len(data):
        raise DecodeError(f'{len(data) - r.p} trailing bytes')
    return v
