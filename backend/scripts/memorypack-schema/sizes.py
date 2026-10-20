import struct, sys, mmap, json
f = open(sys.argv[1], 'rb'); b = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
pe = struct.unpack_from('<I', b, 0x3c)[0]; nsec = struct.unpack_from('<H', b, pe + 6)[0]; opt = struct.unpack_from('<H', b, pe + 20)[0]
base = struct.unpack_from('<Q', b, pe + 48)[0]
secs = [struct.unpack_from('<IIII', b, pe + 24 + opt + 40 * i + 8) for i in range(nsec)]
def off(va):
    rva = va - base
    for vsz, sva, rsz, raw in secs:
        if sva <= rva < sva + rsz: return raw + rva - sva
    raise ValueError(hex(va))
meta = int(sys.argv[2], 16)
q = struct.unpack_from('<16Q', b, off(meta))
count, arr = q[12], q[13]
ptrs = struct.unpack_from(f'<{count}Q', b, off(arr))
sizes = [struct.unpack_from('<4I', b, off(p)) for p in ptrs]
json.dump([s[0] for s in sizes], open(sys.argv[3], 'w'))
print(count, sizes[:3])
