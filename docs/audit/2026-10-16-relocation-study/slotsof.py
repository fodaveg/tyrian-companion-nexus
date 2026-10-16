# slotsof.py <exe>: for the vtables the readers use, the slots (0..0x300) that hold 0x168d10 or 0x168aa0;
# and the .rdata places whose slot 0 is 0x168d10 and that a rip-relative instruction refers to.
import struct, sys, re
f = open(sys.argv[1], 'rb').read(); IB = 0x140000000
pe = struct.unpack_from('<I', f, 0x3c)[0]; ns = struct.unpack_from('<H', f, pe+6)[0]; osz = struct.unpack_from('<H', f, pe+20)[0]
sec = {}
for i in range(ns):
    o = pe+24+osz+40*i; n = f[o:o+8].rstrip(b'\0').decode(); vs, va, rs, ro = struct.unpack_from('<IIII', f, o+8); sec[n] = (va, vs, ro)
tv, ts, to = sec['.text']; T = f[to:to+ts]; rv, rsz, ro = sec['.rdata']
def q(r): return struct.unpack_from('<Q', f, r-rv+ro)[0]
VT = [0x215dfc8, 0x2161250, 0x2163228, 0x2160be0, 0x2161518, 0x215e9d8, 0x21691b8, 0x21841a8, 0x2173178, 0x225de68, 0x225ce50, 0x225e6d0, 0x225e9e8, 0x225eec8, 0x225f1d0, 0x225ea60, 0x225ee08, 0x2184160]
for v in VT:
    hits = [(hex(s), hex(q(v+s)-IB)) for s in range(0, 0x300, 8) if q(v+s)-IB in (0x168d10, 0x168aa0)]
    print(hex(v), hits)
rip = {}
for m in re.finditer(rb'[\x48\x4c][\x8d\x8b][\x05\x0d\x15\x1d\x25\x2d\x35\x3d]', T):
    p = m.start(); rip.setdefault(tv+p+7+struct.unpack_from('<i', T, p+3)[0], []).append(tv+p)
R = f[ro:ro+rsz]; va = struct.pack('<Q', IB+0x168d10); k = -1; n = 0; refd = []
while True:
    k = R.find(va, k+1)
    if k < 0: break
    if k % 8 == 0:
        n += 1
        if rip.get(rv+k): refd.append((hex(rv+k), [hex(x) for x in rip[rv+k]][:3]))
print('places with slot0 = 0x168d10:', n, 'referred to by rip:', len(refd), refd[:10])
