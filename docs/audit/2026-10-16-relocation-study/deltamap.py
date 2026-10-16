# deltamap.py <recert-13oct.json>: for every field the 13 Oct recertification moved, is its target's
# delta the one of the anchors around it (the old RVAs of the guards, vtables and functions)?
import json, sys, bisect
d = json.load(open(sys.argv[1]))
def sect(r): return '.text' if r < 0x1913000 else ('.rdata' if r < 0x2553000 else '.data')
anchors = []
for g in d['guards']: anchors.append((int(g['old_rva'], 16), int(g['new_rva'], 16)-int(g['old_rva'], 16), ','.join(g['names'])))
for v in d['vtables'] + d['compared_by_address_only']: anchors.append((int(v['old_rva'], 16), int(v['new_rva'], 16)-int(v['old_rva'], 16), v['name']))
anchors.sort(); olds = [a[0] for a in anchors]
seen = {}; 
for g in d['guards']:
    for m in g['moved_fields']:
        o, n = int(m['old'], 16), int(m['new'], 16); seen[o] = (n-o, sect(o))
both = one = none = exact = 0; rows = []
for o, (dt, s) in sorted(seen.items()):
    if o in olds: exact += 1; continue
    i = bisect.bisect_right(olds, o)
    lo = anchors[i-1] if i > 0 and sect(anchors[i-1][0]) == s else None
    hi = anchors[i] if i < len(anchors) and sect(anchors[i][0]) == s else None
    ok_lo = lo is not None and lo[1] == dt; ok_hi = hi is not None and hi[1] == dt
    if ok_lo and ok_hi: both += 1
    elif ok_lo or ok_hi: one += 1
    else: none += 1
    rows.append((hex(o), s, hex(dt), lo and (hex(lo[0]), hex(lo[1])), hi and (hex(hi[0]), hex(hi[1])), 'both' if ok_lo and ok_hi else ('one' if ok_lo or ok_hi else 'NONE')))
print('distinct moved targets', len(seen), '| an anchor itself', exact, '| both neighbours agree', both, '| one agrees', one, '| none agrees', none)
for r in rows:
    if r[-1] != 'both': print('  ', r)
