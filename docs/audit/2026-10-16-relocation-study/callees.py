# callees.py <exe> <recert-13oct.json> <certificate_data.rs>: the targets of the 51 spans'
# relocatable fields that are not anchors and not inside the span itself: how many, in which
# section, how many start a .pdata function and how many bytes those functions total.
import sys, runpy, struct, re, bisect
rs = open(sys.argv[3]).read()
sys.argv = sys.argv[:3]  # phase 1 of normsig only: a 4th argument there is an OUTPUT path
g = runpy.run_path(sys.argv[0].replace('callees.py', 'normsig.py'), run_name='lib')
fields, f, secs, sec, secof = g['fields'], g['f'], g['secs'], g['sec'], g['secof']
def o2r(o):
    for n, va, vs, ro in secs:
        if ro <= o < ro+vs: return o-ro+va
spans = [(m.group(1), o2r(int(m.group(2), 16)), int(m.group(3))) for m in re.finditer(r'Span \{ name: "([^"]+)", offset: (0x[0-9a-f]+), len: (\d+)', rs)]
anchor_rvas = {r for _, r, _ in spans} | {int(m.group(1), 16) for m in re.finditer(r'rva: (0x[0-9a-f]+) \}', rs)}
pd = sec['.pdata']; fn = {}
for i in range(pd[2]//12):
    b, e, u = struct.unpack_from('<III', f, pd[3]+12*i)
    if b: fn[b] = e
tx = set(); rd = set(); other = set(); linked = 0
for name, rva, size in spans:
    if secof(rva) != '.text' or size <= 4: continue
    for pos, kind, tgt in fields(rva, size):
        if rva <= tgt < rva+size or tgt == 0 or kind == 'jumptable': continue
        if tgt in anchor_rvas: linked += 1; continue
        s = secof(tgt)
        (tx if s == '.text' else rd if s == '.rdata' else other).add(tgt)
starts = [t for t in tx if t in fn]
print('fields linked to an anchor:', linked)
print('.text targets not anchors:', len(tx), '| start a .pdata function:', len(starts), '| their bytes:', sum(fn[t]-t for t in starts), '| largest:', max(fn[t]-t for t in starts))
print('.text targets not a .pdata start:', sorted(hex(t) for t in tx if t not in fn))
print('.rdata targets:', len(rd), '| other sections:', sorted(hex(t) for t in other))
