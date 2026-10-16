# normsig.py <exe> <recert-13oct.json> [out.json]
# For each of the 37 guards of the 13 Oct recertification:
#  1. disassemble the range in the given exe (whose ranges are those of 13 Oct, per 15 Oct recert);
#  2. find the fields that move with the code: rel32 branches, rip-relative disp32,
#     image-relative disp32 (base register loaded with the image base by a rip lea), jump-table entries;
#  3. rebuild the bytes of the build before 13 Oct with the JSON's moved_fields and check old_sha256;
#  4. mask those fields in the OLD bytes and search the masked pattern at every byte offset of .text
#     of the given exe: number of matches and whether the true place is among them.
import json, struct, sys, hashlib, subprocess, re
exe, recert = sys.argv[1], sys.argv[2]
out_path = sys.argv[3] if len(sys.argv) > 3 else None
if out_path and not out_path.endswith('.json'): raise SystemExit('the 3rd argument is an OUTPUT .json path: %s' % out_path)
CERT_RS = sys.argv[4] if len(sys.argv) > 4 else None
f = open(exe, 'rb').read(); d = json.load(open(recert))
IB = 0x140000000
pe = struct.unpack_from('<I', f, 0x3c)[0]; ns = struct.unpack_from('<H', f, pe+6)[0]; osz = struct.unpack_from('<H', f, pe+20)[0]
secs = []
for i in range(ns):
    o = pe+24+osz+40*i; n = f[o:o+8].rstrip(b'\0').decode(); vs, va, rs, ro = struct.unpack_from('<IIII', f, o+8); secs.append((n, va, vs, ro))
sec = {s[0]: s for s in secs}
def r2o(r):
    for n, va, vs, ro in secs:
        if va <= r < va+vs: return r-va+ro
def secof(r):
    for n, va, vs, ro in secs:
        if va <= r < va+vs: return n
t = sec['.text']; TEXT = f[t[3]:t[3]+t[2]]; TEXT_RVA = t[1]
def disasm(rva, size):
    out = subprocess.run(['x86_64-w64-mingw32-objdump', '-d', '-M', 'intel', '--insn-width=16',
        '--start-address=%d' % (IB+rva), '--stop-address=%d' % (IB+rva+size), exe], capture_output=True, text=True).stdout
    ins = []
    for l in out.splitlines():
        m = re.match(r'\s+([0-9a-f]+):\t([0-9a-f ]+)\t(.*)$', l)
        if m:
            ins.append((int(m.group(1), 16)-IB, bytes.fromhex(m.group(2).replace(' ', '')), m.group(3).strip()))
    return ins
REG64 = {'eax':'rax','ecx':'rcx','edx':'rdx','ebx':'rbx','esp':'rsp','ebp':'rbp','esi':'rsi','edi':'rdi'}
def fields(rva, size):
    """(pos_in_range, kind, target_rva) for every field that moves with the code."""
    res = []; ibregs = set()
    for at, b, txt in disasm(rva, size):
        nxt = at+len(b); pos = at-rva
        if at+len(b) > rva+size: break
        mn = txt.split()[0] if txt else ''
        m = re.search(r'#\s*0x([0-9a-f]+)', txt)
        if 'rip' in txt and m:
            tgt = int(m.group(1), 16)-IB
            le = struct.pack('<i', tgt-nxt); k = b.find(le)
            if k < 0: raise SystemExit('rip field not found at %x %s' % (at, txt))
            res.append((pos+k, 'rip', tgt))
            mm = re.match(r'lea\s+(\w+),', txt)
            if mm and tgt == 0: ibregs.add(mm.group(1))
            elif mm: ibregs.discard(mm.group(1))
            continue
        if (mn == 'call' or mn.startswith('j')) and len(b) >= 5 and (b[-5] in (0xe8, 0xe9) or (len(b) >= 6 and b[-6] == 0x0f and 0x80 <= b[-5] <= 0x8f)):
            tgt = (nxt+struct.unpack('<i', b[-4:])[0]) & 0xffffffff
            res.append((pos+len(b)-4, 'rel32', tgt)); continue
        mm = re.search(r'\[(\w+)(?:\+(\w+)\*\d)?\+0x([0-9a-f]+)\]', txt)
        if mm and (mm.group(1) in ibregs or (mm.group(2) or '') in ibregs):
            disp = int(mm.group(3), 16); k = b.find(struct.pack('<I', disp))
            if k >= 0: res.append((pos+k, 'imagerel', disp))
        mw = re.match(r'(?:mov|movzx|movsxd|add|sub|xor|and|or|lea|imul|shl|shr|sar)\s+(\w+),', txt)
        if mw:
            r = REG64.get(mw.group(1), mw.group(1))
            if mn != 'add': ibregs.discard(r); ibregs.discard(mw.group(1))
    # jump tables kept inside the range: dwords from an image-relative table base that hold
    # RVAs next to the range; they move with the code
    raw = f[r2o(rva):r2o(rva)+size]; taken = {p for p, _, _ in res}
    for p, kind, tgt in list(res):
        if kind != 'imagerel' or not (rva <= tgt < rva+size): continue
        q = tgt-rva
        while q+4 <= size and q not in taken:
            v = struct.unpack_from('<I', raw, q)[0]
            if not (rva-0x1000 <= v < rva+size+0x1000): break
            res.append((q, 'jumptable', v)); taken.add(q); q += 4
    return res
def masked_search(pat, mask):
    best = (0, 0); i = 0; n = len(pat)
    while i < n:
        if mask[i]: i += 1; continue
        j = i
        while j < n and not mask[j]: j += 1
        if j-i > best[1]-best[0]: best = (i, j)
        i = j
    if best[1] == best[0]: return None
    needle = pat[best[0]:best[1]]; hits = []; s = 0
    while True:
        k = TEXT.find(needle, s)
        if k < 0: break
        start = k-best[0]; s = k+1
        if start < 0 or start+n > len(TEXT): continue
        w = TEXT[start:start+n]
        if all(mask[q] or w[q] == pat[q] for q in range(n)): hits.append(TEXT_RVA+start)
    return hits
rows = []
for g in d['guards']:
    name = ','.join(g['names']); rva = int(g['new_rva'], 16); size = g['size']; off = int(g['file_offset'], 16)
    new = bytearray(f[off:off+size]); assert hashlib.sha256(new).hexdigest() == g['new_sha256'], name
    dc = int(g['delta'], 16)
    if secof(rva) != '.text':
        rows.append(dict(name=name, size=size, section=secof(rva), note='data range (.rdata)')); continue
    if name == 'bag:item_factory_table_bag_entry':
        fl = [(0, 'jumptable', struct.unpack('<I', new[:4])[0])]
    else:
        fl = fields(rva, size)
    moved = {int(m['new'], 16): int(m['delta'], 16) for m in g['moved_fields']}
    old = bytearray(new); unexplained = []
    for pos, kind, tgt in fl:
        dt = moved.get(tgt)
        if dt is None:
            # not in the JSON: the image base does not move; a .text target next to the range
            # moves with it (internal branch or jump-table target)
            dt = 0 if tgt == 0 else (dc if secof(tgt) == '.text' and abs(tgt-rva) < 0x10000 else None)
            if dt is None: unexplained.append(hex(tgt)); dt = 0
        v = struct.unpack_from('<i' if kind in ('rel32', 'rip') else '<I', new, pos)[0]
        nv = v+dc-dt if kind in ('rel32', 'rip') else v-dt
        struct.pack_into('<i' if kind in ('rel32', 'rip') else '<I', old, pos, nv)
    old_ok = hashlib.sha256(old).hexdigest() == g['old_sha256']
    mask = [False]*size
    for pos, kind, tgt in fl:
        for q in range(4): mask[pos+q] = True
    hits = masked_search(bytes(old), mask)
    exact = masked_search(bytes(old), [False]*size)
    rows.append(dict(name=name, size=size, rva=hex(rva), identical=g['identical_bytes'], code_delta=hex(dc),
        fields=len(fl), masked_bytes=sum(mask), moved_fields_json=len(g['moved_fields']),
        rebuilt_old_matches_old_sha256=old_ok, unexplained_targets=unexplained,
        masked_hits=None if hits is None else len(hits), true_place_found=None if hits is None else (rva in hits),
        exact_old_bytes_hits=len(exact) if exact is not None else None,
        field_targets=[(p, k, hex(t), secof(t)) for p, k, t in fl]))
for r in rows:
    print('%-50s %5s id=%-5s f=%-3s jsonf=%-3s mask=%-4s oldsha=%-5s hits=%-6s true=%-5s exact=%s %s' % (r['name'][:50], r['size'], r.get('identical'), r.get('fields'), r.get('moved_fields_json'), r.get('masked_bytes'), r.get('rebuilt_old_matches_old_sha256'), r.get('masked_hits'), r.get('true_place_found'), r.get('exact_old_bytes_hits'), r.get('note', '')))
if out_path: json.dump(rows, open(out_path, 'w'), indent=1)

# ---------------------------------------------------------------------------------------------
# Phase 2 (with certificate_data.rs as 4th argument): every anchor the readers rely on, in the
# given exe. For each of the 51 spans: masked uniqueness of its CURRENT bytes and the anchors whose
# fields point at it. For each vtable: places in .rdata that hold its checked slots, and the
# rip-relative lea sites in .text that refer to it, with the masked uniqueness of the function
# around each site. Then a fixpoint: what can be located starting from unique matches only.
if CERT_RS:
    import bisect
    rs = open(CERT_RS).read()
    spans = [(m.group(1), int(m.group(2), 16), int(m.group(3))) for m in re.finditer(r'Span \{ name: "([^"]+)", offset: (0x[0-9a-f]+), len: (\d+)', rs)]
    slots = [(m.group(1), int(m.group(2), 16), int(m.group(3), 16)) for m in re.finditer(r'Slot \{ name: "([^"]+)", offset: (0x[0-9a-f]+), rva: (0x[0-9a-f]+)', rs)]
    def o2r(o):
        for n, va, vs, ro in secs:
            if ro <= o < ro+vs: return o-ro+va
    pd = sec['.pdata']; pbeg = []; pend = []
    for i in range(pd[2]//12):
        b, e, u = struct.unpack_from('<III', f, pd[3]+12*i)
        if b: pbeg.append(b); pend.append(e)
    order = sorted(range(len(pbeg)), key=lambda i: pbeg[i]); pbeg = [pbeg[i] for i in order]; pend = [pend[i] for i in order]
    def func_of(r):
        i = bisect.bisect_right(pbeg, r)-1
        return (pbeg[i], pend[i]) if i >= 0 and pbeg[i] <= r < pend[i] else None
    anchors = {}
    for name, off, ln in spans:
        r = o2r(off); anchors[name] = dict(kind='span', rva=r, size=ln, section=secof(r))
    old_hits = {x['name']: x for x in rows}
    for name, a in anchors.items():
        if a['section'] != '.text': continue
        if name == 'bag:item_factory_table_bag_entry':
            a['fields'] = [(0, 'jumptable', struct.unpack_from('<I', f, r2o(a['rva']))[0])]; a['hits'] = None; continue
        fl = fields(a['rva'], a['size']); a['fields'] = fl
        mask = [False]*a['size']
        for pos, kind, tgt in fl:
            for q in range(4): mask[pos+q] = True
        h = masked_search(f[r2o(a['rva']):r2o(a['rva'])+a['size']], mask)
        a['hits'] = None if h is None else len(h)
    # .rdata data span (hash table): exact search at 8-aligned places of .rdata
    rd = sec['.rdata']; RDATA = f[rd[3]:rd[3]+rd[2]]
    for name, a in anchors.items():
        if a['section'] == '.rdata':
            pat = f[r2o(a['rva']):r2o(a['rva'])+a['size']]; k = -1; n = 0
            while True:
                k = RDATA.find(pat, k+1)
                if k < 0: break
                n += (k % 8 == 0)
            a['hits'] = n
    # vtables: group the checked slots by vtable name
    vt = {}
    for name, off, frva in slots:
        m = re.match(r'vtable (.+) \+(0x[0-9a-f]+)$', name); v = m.group(1); so = int(m.group(2), 16)
        base = o2r(off)-so; vt.setdefault(v, dict(rva=base, slots=[]))['slots'].append((so, frva))
    span_at = {a['rva']: n for n, a in anchors.items()}
    # rip-relative lea sites in .text that refer to each vtable
    leas = {}
    for m in re.finditer(rb'[\x48\x4c]\x8d[\x05\x0d\x15\x1d\x25\x2d\x35\x3d]', TEXT):
        p = m.start(); disp = struct.unpack_from('<i', TEXT, p+3)[0]; tgt = TEXT_RVA+p+7+disp
        leas.setdefault(tgt, []).append(TEXT_RVA+p)
    def masked_hits_of_window(r0, r1):
        fl = fields(r0, r1-r0); mask = [False]*(r1-r0)
        for pos, kind, tgt in fl:
            for q in range(4): mask[pos+q] = True
        h = masked_search(f[r2o(r0):r2o(r1)], mask); return None if h is None else len(h)
    for v, x in vt.items():
        base = x['rva']; want = [(so, IB+frva) for so, frva in x['slots']]
        places = 0
        for q in range(0, len(RDATA)-0x400, 8):
            if all(struct.unpack_from('<Q', RDATA, q+so)[0] == va for so, va in want): places += 1
        x['places_holding_checked_slots'] = places
        sites = leas.get(base, []); x['lea_sites'] = []
        for s_ in sites:
            fn = func_of(s_)
            if fn and fn[1]-fn[0] <= 4096: w = fn; h = masked_hits_of_window(*fn)
            else: w = (s_-48, s_+48); h = masked_hits_of_window(*w)  # long or unlisted function: a window
            x['lea_sites'].append(dict(site=hex(s_), window=[hex(w[0]), hex(w[1])], masked_hits=h))
        x['slot_functions'] = [(hex(so), hex(frva), span_at.get(frva)) for so, frva in x['slots']]
    # edges: an anchor whose field points at another anchor (or at a jump-table slot of it)
    edges = []
    for n, a in anchors.items():
        for pos, kind, tgt in a.get('fields', []):
            if tgt in span_at and span_at[tgt] != n: edges.append((n, span_at[tgt], kind))
            for m2, b in anchors.items():  # jump table entry inside a table the field points at
                if b['size'] == 4 and kind == 'imagerel' and 0 < b['rva']-tgt < 0x400 and m2 != n: edges.append((n, m2, 'table+%d' % (b['rva']-tgt)))
    # fixpoint from unique matches
    located = {n for n, a in anchors.items() if a.get('hits') == 1}
    vloc = set(); how = {n: 'unique match' for n in located}
    changed = True
    while changed:
        changed = False
        for a_, b_, k in edges:
            if a_ in located and b_ not in located: located.add(b_); how[b_] = 'field of ' + a_; changed = True
        for v, x in vt.items():
            if v not in vloc:
                if any(s['masked_hits'] == 1 for s in x['lea_sites']): vloc.add(v); how['vt:'+v] = 'lea site, unique'; changed = True
                elif x['places_holding_checked_slots'] == 1 and any(fn in located for _, _, fn in x['slot_functions'] if fn):
                    vloc.add(v); how['vt:'+v] = 'slots of located functions, unique place'; changed = True
            if v in vloc:
                for so, frva, fn in x['slot_functions']:
                    if fn and fn not in located: located.add(fn); how[fn] = 'slot %s of vtable %s' % (so, v); changed = True
    print('\n== spans (current bytes, masked)')
    for n, a in anchors.items(): print('%-62s %-6s size=%-5s hits=%-5s located=%s' % (n[:62], a['section'], a['size'], a.get('hits'), how.get(n, 'NO')))
    print('\n== vtables')
    for v, x in vt.items(): print('%-34s rva=%s places=%s lea=%s located=%s' % (v, hex(x['rva']), x['places_holding_checked_slots'], [(s['site'], s['masked_hits']) for s in x['lea_sites']], how.get('vt:'+v, 'NO')))
    print('\n== edges'); [print('  ', e) for e in edges]
    print('\nlocated spans %d/%d, vtables %d/%d' % (sum(1 for n in anchors if n in located), len(anchors), len(vloc), len(vt)))
    if out_path: json.dump(dict(rows=rows, anchors={n: {k: v for k, v in a.items() if k != 'fields'} for n, a in anchors.items()}, vtables=vt, edges=edges, how=how), open(out_path, 'w'), indent=1)
