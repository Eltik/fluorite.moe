"""Compile the IL2CPP type table into a flat, fully resolved MemoryPack schema.

Every type reference the decoder can meet is resolved once, in the same order as
mpdecode.Reader.value, into a node; nodes refer to each other by index. The output needs no
C# name resolution at decode time.

Usage: compile.py types.json winners.json game_version out.json
"""
import json, os, sys
sys.path.insert(0, os.path.dirname(__file__))
import mpdecode as M

PRIM = {'bool': 'bool', 'byte': 'u8', 'sbyte': 'i8', 'short': 'i16', 'ushort': 'u16', 'char': 'u16', 'int': 'i32',
        'uint': 'u32', 'long': 'i64', 'ulong': 'u64', 'float': 'f32', 'double': 'f64'}


class Compiler:
    def __init__(self, schema):
        self.s = schema
        self.nodes = []
        self.memo = {}
        self.objects = {}

    def add(self, node):
        self.nodes.append(node)
        return len(self.nodes) - 1

    def placeholder(self):
        return self.add(None)

    def value(self, t, ctx, raw_ok=False):
        t = t.strip()
        key = (t, ctx, raw_ok)
        if key in self.memo:
            return self.memo[key]
        slot = self.placeholder()
        self.memo[key] = slot
        self.nodes[slot] = self._value(t, ctx, raw_ok)
        return slot

    def _raw(self, size, fields, ctx):
        return {'k': 'raw', 'size': size, 'f': [[off, name, self.value(ft, ctx, True)] for off, name, ft in fields]}

    def _value(self, t, ctx, raw_ok):
        s = self.s
        if t.endswith('[]'):
            return {'k': 'list', 'e': self.value(t[:-2], ctx, True)}
        if t in PRIM:
            return {'k': 'p', 't': PRIM[t]}
        if t == 'string':
            return {'k': 'str'}
        if t == 'StringPathHash':
            return {'k': 'p', 't': 'i64'}
        base, args = M.split_generic(t)
        if base in M.LISTS:
            return {'k': 'list', 'e': self.value(args[0], ctx)}
        if base in M.HEADED_DICTS:
            return {'k': 'dict', 'hdr': True, 'key': self.value(args[0], ctx), 'val': self.value(args[1], ctx)}
        if base in M.DICTS:
            return {'k': 'dict', 'hdr': False, 'key': self.value(args[0], ctx), 'val': self.value(args[1], ctx)}
        if raw_ok and args:
            lay = s.generic_struct(base, args, ctx)
            if lay:
                return self._raw(lay[0], lay[1], ctx)
        special = {'AnimationCurve': 'curve', 'Gradient': 'gradient', 'RectOffset': 'rect_offset'}.get(t.split('.')[-1])
        if special:
            return {'k': special}
        gw = s.generic_wrappers.get(M.normalize(t))
        if gw:
            return {'k': 'ref', 'to': self.object(gw)}
        if base == 'Nullable' or t.endswith('?'):
            inner = args[0] if args else t[:-1]
            if inner in M.SIZES:
                off = M.SIZES[inner]
            else:
                f = s.resolve(inner, ctx)
                lay = f and s.unmanaged(f)
                if not lay:
                    return {'k': 'unsupported', 'why': f'nullable {t} of non-blittable type'}
                off = lay[2]
            return {'k': 'nullable', 'off': off, 'v': self.value(inner, ctx, True)}
        full = s.resolve(t, ctx)
        if not full:
            return {'k': 'unsupported', 'why': f'unknown type {t} (from {ctx})'}
        kind = s.types[full]['kind']
        if kind == 'enum':
            ut = next((f['type'] for f in s.types[full]['fields'] if f['name'] == 'value__'), 'int')
            return {'k': 'p', 't': PRIM[ut]}
        w = s.wrapper_of.get(full)
        if kind == 'struct' and (raw_ok or not w):
            lay = s.unmanaged(full)
            if lay:
                return self._raw(lay[0], lay[1], full)
        if w:
            return {'k': 'ref', 'to': self.object(w)}
        return {'k': 'unsupported', 'why': f'unsupported type {t} ({full}, {kind})'}

    def object(self, wfull):
        """Node index of the object (or union) serialized by wrapper `wfull`."""
        if wfull in self.objects:
            return self.objects[wfull]
        slot = self.placeholder()
        self.objects[wfull] = slot
        s = self.s
        try:
            unions = s.unions(wfull)
        except M.DecodeError as e:
            self.nodes[slot] = {'k': 'unsupported', 'why': str(e)}
            return slot
        if unions:
            cases = {}
            for tag, sub in sorted(unions.items()):
                o = s.orig_of[sub]
                cases[str(tag)] = [s.types[o]['name'] if o else s.types[sub]['name'], self.object(sub)]
            self.nodes[slot] = {'k': 'union', 'c': cases}
            return slot
        ctx = s.orig_of[wfull]
        members = [[name, self.value(mt, ctx, allow)] for name, mt, allow in s.members(wfull)]
        self.nodes[slot] = {'k': 'obj', 'm': members}
        return slot


def main():
    types = json.load(open(sys.argv[1]))
    winners = json.load(open(sys.argv[2]))
    schema = M.Schema(types)
    c = Compiler(schema)
    roots = {}
    for orig, w in sorted(schema.wrapper_of.items()):
        roots[orig] = c.object(w)
    out = {
        'format': 1,
        'game_version': sys.argv[3],
        'nodes': c.nodes,
        'roots': roots,
        # Root types that decoded files in each JsonData directory, most common first.
        'dir_roots': {d: w for d, w in sorted(winners.items())},
    }
    json.dump(out, open(sys.argv[4], 'w'), separators=(',', ':'))
    kinds = {}
    for n in c.nodes:
        kinds[n['k']] = kinds.get(n['k'], 0) + 1
    print(len(c.nodes), 'nodes', kinds, len(roots), 'roots', os.path.getsize(sys.argv[4]) // 1024, 'KiB')


if __name__ == '__main__':
    main()
