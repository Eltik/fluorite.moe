import struct, sys, os
def lz4_block(src, out_len):
    dst = bytearray(); i = 0
    while i < len(src):
        tok = src[i]; i += 1
        lit = tok >> 4
        if lit == 15:
            while True:
                b = src[i]; i += 1; lit += b
                if b != 255: break
        dst += src[i:i+lit]; i += lit
        if i >= len(src): break
        off = src[i] | src[i+1] << 8; i += 2
        ml = tok & 15
        if ml == 15:
            while True:
                b = src[i]; i += 1; ml += b
                if b != 255: break
        ml += 4
        for _ in range(ml): dst.append(dst[-off])
    assert len(dst) == out_len, (len(dst), out_len)
    return bytes(dst)
def read_block(path, first):
    b = open(path, 'rb').read(); p = 4 if first else 0
    magic, ver, flags, count, raw_len, stored = struct.unpack_from('<6i', b, p)
    assert magic == 0x43505355, hex(magic)
    data = b[p+24:p+24+stored]
    raw = lz4_block(data, raw_len) if flags & 1 else data
    return count, raw
def strings(raw, count):
    offs = struct.unpack_from(f'<{count+1}i', raw, 0)
    base = 4 * (count + 1)
    return [raw[base+offs[k]:base+offs[k+1]].decode('utf-8') for k in range(count)]
if __name__ == '__main__':
    d = sys.argv[1]
    count, raw = read_block(os.path.join(d, 'StringPool0.byte'), True)
    print('count', count, 'raw', len(raw), raw[:48].hex())
    try:
        s = strings(raw, count); print(len(s), s[:5], 'idx 0x5c0:', s[0x5c0], s[0x5c1])
    except Exception as e: print('layout guess failed', e)
