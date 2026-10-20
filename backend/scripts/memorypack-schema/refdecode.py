"""Reference decoder for the compiled MemoryPack schema (the behavior the Rust port must match).

Usage: refdecode.py schema.json json_root out_dir   -> writes <out_dir>/<rel>.json for every decoded file
"""
import json, math, os, struct, sys
sys.path.insert(0, os.path.dirname(__file__))
import mpdecode as M

FMT = {'bool': '<?', 'u8': '<B', 'i8': '<b', 'i16': '<h', 'u16': '<H', 'i32': '<i', 'u32': '<I', 'i64': '<q',
       'u64': '<Q', 'f32': '<f', 'f64': '<d'}


def shortest_f32(v):
    """The shortest decimal that rounds back to the same f32 (so 0.1f prints as 0.1)."""
    bits = struct.pack('<f', v)
    for digits in range(1, 10):
        cand = float(f'{v:.{digits}g}')
        try:
            if struct.pack('<f', cand) == bits:
                return cand
        except OverflowError:  # rounding near f32::MAX can leave the f32 range
            continue
    return v


class Fail(Exception):
    pass


class R:
    def __init__(self, nodes, strings, data):
        self.n, self.s, self.b, self.p = nodes, strings, data, 0

    def take(self, fmt):
        k = struct.calcsize(fmt)
        if self.p + k > len(self.b):
            raise Fail('eof')
        v = struct.unpack_from(fmt, self.b, self.p); self.p += k
        return v

    def count(self):
        c = self.take('<i')[0]
        if c == -1:
            return None
        if c < 0 or c > len(self.b) - self.p:
            raise Fail('bad count')
        return c

    def prim(self, t):
        v = self.take(FMT[t])[0]
        if t in ('f32', 'f64') and not math.isfinite(v):
            return None
        if t == 'f32':
            return shortest_f32(v)
        return v

    def v(self, i, depth=0):
        if depth > 128:
            raise Fail('too deep')
        node = self.n[i]
        k = node['k']
        if k == 'p':
            return self.prim(node['t'])
        if k == 'str':
            idx = self.take('<i')[0]
            if idx == -1:
                return None
            if idx == -2:
                return ''
            if not 0 <= idx < len(self.s):
                raise Fail('string index')
            return self.s[idx]
        if k == 'list':
            c = self.count()
            return None if c is None else [self.v(node['e'], depth + 1) for _ in range(c)]
        if k == 'dict':
            if node['hdr'] and self.take('<B')[0] == 255:
                return None
            c = self.count()
            if c is None:
                return None
            pairs = [(self.v(node['key'], depth + 1), self.v(node['val'], depth + 1)) for _ in range(c)]
            # A JSON object when every key is a string or integer (not bool) and the keys are unique.
            if all(isinstance(key, str) or (isinstance(key, int) and not isinstance(key, bool)) for key, _ in pairs):
                keys = [key if isinstance(key, str) else str(key) for key, _ in pairs]
                if len(set(keys)) == len(keys):
                    return dict(zip(keys, (val for _, val in pairs)))
            return [[key, val] for key, val in pairs]
        if k == 'raw':
            start = self.p
            if start + node['size'] > len(self.b):
                raise Fail('eof')
            out = {}
            for off, name, fi in node['f']:
                self.p = start + off
                out[name] = self.v(fi, depth + 1)
            self.p = start + node['size']
            return out
        if k == 'nullable':
            start = self.p
            has = self.take('<?')[0]
            self.p = start + node['off']
            val = self.v(node['v'], depth + 1)
            return val if has else None
        if k == 'ref':
            return self.v(node['to'], depth + 1)
        if k == 'obj':
            h = self.take('<B')[0]
            if h == 255:
                return None
            if h != len(node['m']):
                raise Fail('header')
            return {name: self.v(mi, depth + 1) for name, mi in node['m']}
        if k == 'union':
            tag = self.take('<B')[0]
            if tag == 255:
                return None
            if tag == 250:
                tag = self.take('<H')[0]
            case = node['c'].get(str(tag))
            if case is None:
                raise Fail('union tag')
            val = self.v(case[1], depth + 1)
            return {'$type': case[0], **val} if isinstance(val, dict) else val
        if k == 'curve':
            if self.take('<B')[0] == 255:
                return None
            post, pre = self.take('<ii')
            c = self.count()
            keys = None
            if c is not None:
                keys = []
                for _ in range(c):
                    vals = [self.prim('f32'), self.prim('f32'), self.prim('f32'), self.prim('f32'), self.prim('i32'),
                            self.prim('f32'), self.prim('f32')]
                    keys.append(dict(zip(('time', 'value', 'inTangent', 'outTangent', 'weightedMode', 'inWeight',
                                          'outWeight'), vals)))
            return {'postWrapMode': post, 'preWrapMode': pre, 'keys': keys}
        if k == 'gradient':
            if self.take('<B')[0] == 255:
                return None
            c = self.count()
            colors = None if c is None else [dict(zip(('r', 'g', 'b', 'a', 'time'), [self.prim('f32') for _ in range(5)])) for _ in range(c)]
            a = self.count()
            alphas = None if a is None else [dict(zip(('alpha', 'time'), [self.prim('f32') for _ in range(2)])) for _ in range(a)]
            return {'colorKeys': colors, 'alphaKeys': alphas, 'mode': self.prim('i32')}
        if k == 'rect_offset':
            if self.take('<B')[0] == 255:
                return None
            return dict(zip(('left', 'right', 'top', 'bottom'), [self.prim('i32') for _ in range(4)]))
        raise Fail(node.get('why', k))


def candidates(schema, rel_dir, first):
    """Root types to try, in order: the directory's known roots, then every root whose
    header byte matches (objects with that many members, then unions), by type name."""
    out = list(schema['dir_roots'].get(rel_dir, []))
    seen = set(out)
    objs = sorted(r for r, i in schema['roots'].items()
                  if schema['nodes'][i]['k'] == 'obj' and len(schema['nodes'][i]['m']) == first)
    unions = sorted(r for r, i in schema['roots'].items() if schema['nodes'][i]['k'] == 'union')
    return out + [r for r in objs + unions if r not in seen]


def decode_file(schema, strings, data, rel_dir):
    for root in candidates(schema, rel_dir, data[0]):
        r = R(schema['nodes'], strings, data)
        try:
            val = r.v(schema['roots'][root])
        except (Fail, RecursionError):
            continue
        if r.p == len(data):
            return root, val
    return None, None


def main():
    schema = json.load(open(sys.argv[1]))
    root_dir, out_dir = sys.argv[2], sys.argv[3]
    strings = M.load_strings(os.path.join(root_dir, 'StringPool'))
    ok = fail = 0
    for d, _, names in os.walk(root_dir):
        for n in sorted(names):
            if not n.endswith('.json'):
                continue
            path = os.path.join(d, n)
            data = open(path, 'rb').read()
            if not data or data[:1] == b'{':
                continue
            rel_dir = os.path.relpath(d, root_dir)
            root, val = decode_file(schema, strings, data, rel_dir)
            if root is None:
                fail += 1
                continue
            ok += 1
            dst = os.path.join(out_dir, os.path.relpath(path, root_dir))
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            with open(dst, 'w', encoding='utf-8') as f:
                json.dump(val, f, indent=2, ensure_ascii=False)
    print('ok', ok, 'fail', fail)


if __name__ == '__main__':
    main()
