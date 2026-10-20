import json, os, re, sys, collections, time
sys.path.insert(0, os.path.dirname(__file__)); import mpdecode as M
types = json.load(open(sys.argv[1])); S = M.Schema(types); root = sys.argv[2]
strings = M.load_strings(os.path.join(root, 'StringPool'))
# Candidate roots: original types that have a wrapper, keyed by header byte.
by_count = collections.defaultdict(list); unions = []
for orig, w in S.wrapper_of.items():
    if S.unions(w): unions.append(orig)
    elif 'abstract' not in types[w]['mods']: by_count[len(S.members(w))].append(orig)
def words(s): return set(w.lower() for w in re.findall(r'[A-Z]?[a-z]+|[A-Z]+(?![a-z])|\d+', s))
def score(orig, rel):
    hint = words(rel.replace('/', ' ').replace('_', ' '))
    return -len(words(types[orig]['name']) & hint)
winners = {}; stats = collections.Counter(); fails = collections.defaultdict(list); t0 = time.time()
files = sorted(f for f in (os.path.relpath(os.path.join(d, n), root) for d, _, ns in os.walk(root) for n in ns) if f.endswith('.json'))
for rel in files:
    data = open(os.path.join(root, rel), 'rb').read()
    if not data or data[:1] == b'{': stats['plain'] += 1; continue
    d = os.path.dirname(rel)
    tried = []
    order = winners.get(d, [])
    rest = sorted(by_count.get(data[0], []) + unions, key=lambda o: score(o, rel))
    ok = None
    for orig in order + [o for o in rest if o not in order]:
        try:
            M.decode(data, orig, S, strings); ok = orig; break
        except Exception as e:
            tried.append(orig)
            if len(tried) > 400: break
    if ok:
        stats['ok'] += 1; w = winners.setdefault(d, [])
        if ok in w: w.remove(ok)
        w.insert(0, ok)
    else:
        stats['fail'] += 1; fails[d].append(rel)
print(dict(stats), f'{time.time() - t0:.0f}s')
json.dump({d: w for d, w in winners.items()}, open(os.path.join(os.path.dirname(__file__), 'winners.json'), 'w'), indent=1)
for d, fs in sorted(fails.items(), key=lambda kv: -len(kv[1]))[:25]: print(len(fs), d, fs[0])
