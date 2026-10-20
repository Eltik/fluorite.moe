import struct, sys, mmap
ga, md = sys.argv[1], sys.argv[2]
m = open(md, 'rb').read()
# v29 header: magic, version, then (offset, size) pairs; typeDefinitions is pair index 20 (stringLiteral=0 ...)
pairs = [struct.unpack_from('<ii', m, 8 + 8 * i) for i in range(32)]
f = open(ga, 'rb'); b = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
pe = struct.unpack_from('<I', b, 0x3c)[0]
nsec = struct.unpack_from('<H', b, pe + 6)[0]; opt = struct.unpack_from('<H', b, pe + 20)[0]
base = struct.unpack_from('<Q', b, pe + 24 + 24)[0]
secs = []
for i in range(nsec):
    o = pe + 24 + opt + 40 * i
    name = b[o:o+8].rstrip(b'\0').decode(errors='replace'); vsz, va, rsz, raw = struct.unpack_from('<IIII', b, o + 8)
    secs.append((name, va, vsz, raw, rsz)); print(f'{name:10} va={va:#x} vsz={vsz:#x} raw={raw:#x} rsz={rsz:#x}')
lo, hi = base, base + max(va + vsz for _, va, vsz, _, _ in secs)
# candidate type-def counts: size/entry for plausible entry sizes
for idx, (off, size) in enumerate(pairs):
    if size > 0: print(idx, off, size)
print('image', hex(lo), hex(hi))
size = pairs[19][1]
cands = [size // e for e in range(80, 120, 4) if size % e == 0]
print('typedef count candidates', cands)
def va2off(va):
    rva = va - base
    for _, sva, vsz, raw, rsz in secs:
        if sva <= rva < sva + vsz: return raw + (rva - sva) if rva - sva < rsz else None
for name, va, vsz, raw, rsz in secs:
    if name not in ('.data', '.rdata'): continue
    data = b[raw:raw + rsz]
    for N in cands:
        pat = struct.pack('<Q', N)
        i = data.find(pat)
        while i != -1:
            if i % 8 == 0 and i + 32 <= len(data):
                n1, p1, n2, p2 = struct.unpack_from('<QQQQ', data, i)
                if n1 == N and n2 == N and lo <= p1 < hi and lo <= p2 < hi:
                    reg = base + va + i - 10 * 8
                    print(f'{name} N={N} fieldOffsetsCount at {base+va+i:#x} -> MetadataRegistration {reg:#x}')
                    q = struct.unpack_from('<16Q', data, i - 80)
                    print('   ', [hex(x) for x in q])
            i = data.find(pat, i + 1)
