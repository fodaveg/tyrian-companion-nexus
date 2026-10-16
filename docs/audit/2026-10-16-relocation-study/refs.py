# refs.py <exe> <rva>...: 8-aligned .rdata places holding IB+rva, rip-relative refs in .text to rva
# and to each of those places, and rel32 call/jmp refs to rva.
import struct, sys, re
f = open(sys.argv[1], 'rb').read(); IB = 0x140000000
pe = struct.unpack_from('<I', f, 0x3c)[0]; ns = struct.unpack_from('<H', f, pe+6)[0]; osz = struct.unpack_from('<H', f, pe+20)[0]
sec = {}
for i in range(ns):
    o = pe+24+osz+40*i; n = f[o:o+8].rstrip(b'\0').decode(); vs, va, rs, ro = struct.unpack_from('<IIII', f, o+8); sec[n] = (va, vs, ro)
tv, ts, to = sec['.text']; T = f[to:to+ts]; rv, rsz, ro = sec['.rdata']; R = f[ro:ro+rsz]
rip = {}
for m in re.finditer(rb'[\x48\x4c][\x8d\x8b][\x05\x0d\x15\x1d\x25\x2d\x35\x3d]', T):
    p = m.start(); rip.setdefault(tv+p+7+struct.unpack_from('<i', T, p+3)[0], []).append(tv+p)
rel = {}
for m in re.finditer(rb'[\xe8\xe9]', T):
    p = m.start()
    if p+5 <= len(T): rel.setdefault(tv+p+5+struct.unpack_from('<i', T, p+1)[0], []).append(tv+p)
for a in sys.argv[2:]:
    r = int(a, 16); va = struct.pack('<Q', IB+r); places = []; k = -1
    while True:
        k = R.find(va, k+1)
        if k < 0: break
        if k % 8 == 0: places.append(rv+k)
    print(hex(r), 'rdata places', len(places), [hex(x) for x in places[:12]])
    print('   rip refs to it', [hex(x) for x in rip.get(r, [])][:8], ' rel32 (e8/e9 candidates)', len(rel.get(r, [])))
    for pl in places[:12]:
        for back in range(0, 0x400, 8):
            if rip.get(pl-back):
                print('   place', hex(pl), 'is slot', hex(back), 'of', hex(pl-back), 'rip refs', [hex(x) for x in rip[pl-back]][:4]); break
