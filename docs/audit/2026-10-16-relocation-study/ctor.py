# ctor.py <exe> <recert-13oct.json> <certificate_data.rs>: the constructor windows that locate the
# buff node vtable and the conditional item A and B vtables; for each masked hit, where its
# rip-relative fields point (to tell A from B by the predicate vtable they install next to it).
import sys, runpy
sys.argv = [sys.argv[0], sys.argv[1], sys.argv[2]]  # phase 1 only, to get the helpers
g = runpy.run_path(sys.argv[0].replace('ctor.py', 'normsig.py'), run_name='lib')
fields, masked_search, func_of_src, f, r2o = g['fields'], g['masked_search'], None, g['f'], g['r2o']
import struct, bisect
secs = g['secs']; sec = g['sec']; pd = sec['.pdata']; pb = []
for i in range(pd[2]//12):
    b, e, u = struct.unpack_from('<III', f, pd[3]+12*i)
    if b: pb.append((b, e))
pb.sort(); starts = [x[0] for x in pb]
def func_of(r):
    i = bisect.bisect_right(starts, r)-1
    return pb[i] if i >= 0 and pb[i][0] <= r < pb[i][1] else None
for site, label in [(0x12c23d6, 'buff_node'), (0x13ca71e, 'conditional item A'), (0x13caeee, 'conditional item B'), (0x13c973e, 'bag_item')]:
    fn = func_of(site); r0, r1 = fn; fl = fields(r0, r1-r0); mask = [False]*(r1-r0)
    for pos, kind, tgt in fl:
        for q in range(4): mask[pos+q] = True
    hits = masked_search(f[r2o(r0):r2o(r1)], mask)
    print(label, 'function', hex(r0), hex(r1), 'size', r1-r0, 'masked hits', len(hits))
    for h in hits:
        tg = []
        for pos, kind, tgt in fl:
            if kind == 'rip':
                v = struct.unpack_from('<i', f, r2o(h)+pos)[0]
                # next ip: the field is the last 4 bytes of the instruction unless an immediate follows;
                # here the same layout as at r0, so the offset from field to next ip is reused
                tg.append(hex((h+pos+4+v) + ((tgt - (r0+pos+4)) - struct.unpack_from('<i', f, r2o(r0)+pos)[0])))
        print('   hit', hex(h), 'rip targets', tg)
