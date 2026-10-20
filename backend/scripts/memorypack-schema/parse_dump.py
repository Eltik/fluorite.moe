"""Parse Il2CppDumper's dump.cs into a JSON type table (declaration order preserved)."""
import json, re, sys
src, out = sys.argv[1], sys.argv[2]
decl = re.compile(r'^(?P<mods>(?:[a-z]+ )*)(?P<kind>class|struct|enum|interface) (?P<name>[^\s:]+(?:<[^>]*>)?)(?: : (?P<bases>.*?))? // TypeDefIndex: (?P<idx>\d+)$')
field = re.compile(r'^\t(?P<mods>(?:(?:public|private|protected|internal|static|readonly|const|volatile|new|fixed) )*)(?P<type>.+?) (?P<name>[^\s;]+)(?: = (?P<value>.*?))?; // (?P<off>0x[0-9A-F]+)?')
prop = re.compile(r'^\t(?P<mods>(?:(?:public|private|protected|internal|static|virtual|override|abstract|sealed|new) )*)(?P<type>.+?) (?P<name>[^\s{]+) \{ (?P<acc>[^}]*)\}')
types = {}; ns = ''; attrs = []; cur = None; section = None; member_attrs = []
for line in open(src, encoding='utf-8', errors='replace'):
    line = line.rstrip('\n')
    if line.startswith('// Namespace: '):
        ns = line[len('// Namespace: '):].strip(); attrs = []; continue
    if cur is None:
        if line.startswith('['):
            attrs.append(line); continue
        m = decl.match(line)
        if m:
            name = m['name']
            full = f'{ns}.{name}' if ns else name
            cur = {'ns': ns, 'name': name, 'kind': m['kind'], 'mods': m['mods'].split(),
                   'bases': [b.strip() for b in re.split(r',\s*(?![^<]*>)', m['bases'])] if m['bases'] else [],
                   'attrs': attrs, 'idx': int(m['idx']), 'fields': [], 'props': []}
            types[full] = cur; attrs = []; section = None; member_attrs = []
        continue
    if line == '{}':
        cur = None; continue
    if line == '}':
        cur = None; continue
    s = line.strip()
    if s == '// Fields': section = 'f'; continue
    if s == '// Properties': section = 'p'; continue
    if s == '// Methods': section = 'm'; continue
    if section in ('f', 'p') and s.startswith('[') and s.endswith(']'):
        member_attrs.append(s); continue
    if section == 'f':
        m = field.match(line)
        if m:
            cur['fields'].append({'type': m['type'], 'name': m['name'], 'mods': m['mods'].split(), 'off': m['off'], 'value': m['value'], 'attrs': member_attrs})
        member_attrs = []
    elif section == 'p':
        m = prop.match(line)
        if m:
            cur['props'].append({'type': m['type'], 'name': m['name'], 'acc': m['acc'].split(), 'attrs': member_attrs})
        member_attrs = []
    elif section == 'm' and not s.startswith('['):
        member_attrs = []
# Nested types are printed with an empty namespace; key them under their outer type.
tops = {}
for full, t in types.items():
    if t['ns'] or '.' not in t['name']:
        tops.setdefault(t['name'], []).append((t['idx'], full))
for _pass in range(6):
  fixed = {}
  for full, t in sorted(types.items(), key=lambda kv: kv[1]['idx']):
      if not t['ns'] and '.' in t['name']:
          outer_name = t['name'].rsplit('.', 1)[0]
          outers = tops.get(outer_name, [])
          if outers:
              outer = min(outers, key=lambda o: abs(o[0] - t['idx']))[1]
              t['ns'] = fixed.get(outer, types.get(outer, {})).get('ns', '')
              full = f"{t['ns']}.{t['name']}" if t['ns'] else t['name']
              tops.setdefault(t['name'], []).append((t['idx'], full))
      fixed[full] = t
  types = fixed
json.dump(types, open(out, 'w'))
print(len(types), 'types')
