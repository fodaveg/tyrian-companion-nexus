#!/usr/bin/env python3
"""Checks of the relocation table and its resolver over a copy of the certified executable.

    python3 -I test_relocation.py <certified Gw2-64.exe> --tmp <directory outside the repo>
        [--recert13 docs/audit/2026-10-13-recertification-build-208574.json]
        [--table core/src/relocation_data.rs] [--certificate core/src/certificate_data.rs]
        [--only identity,replay13,moved,negatives,leak]

Prints the literal outcome of each check and exits 1 if any did not come out as expected. The
executables it makes (a copy with a block of .text moved, copies with one change each) are
written under --tmp only, and deleted at the end. It prints RVAs, counts and digests, never
bytes of the game.

- identity: the copy resolves to the certified RVAs by the identity path and by search alone;
- replay13: the figures of the relocation study, section 3, over the recertification of
  13 October 2026: the 37 old digests rebuilt from the table's fields, how many of the 19
  ranges that changed are unique by search alone, how many the graph adds, 37/37 in all;
- moved: a copy where a block of .text holding several anchors is rotated (two runs of whole
  functions swap places), with every rel32 and rip-relative reference of .text, the
  image-relative fields and jump tables of the moved functions, the base-relocated pointers into
  it and its .pdata entries rewritten; the resolver must find the moved RVAs;
- negatives, each seen to fail, then seen to pass with the check that catches it broken on
  purpose: a field offset changed inside an anchor, the twin of mf:buff_lookup presented as
  the target of modifier_value's call, an anchor found at two places that both pass every check;
- leak: no 8-byte window of any range the table keeps a digest of appears in the generated
  files, in hex or in decimal.
"""
import argparse
import importlib.util
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
sys.dont_write_bytecode = True  # no __pycache__ next to the scripts
spec =importlib.util.spec_from_file_location('gen_relocation', os.path.join(HERE, 'gen_relocation.py'))
g = importlib.util.module_from_spec(spec)
spec.loader.exec_module(g)

FAILED = []


def check(name, ok, detail=''):
    print('%s %s%s' % ('PASS' if ok else 'FAIL', name, (': ' + detail) if detail else ''))
    if not ok:
        FAILED.append(name)


def resolve_or_fail(rt, pe, cert, force=False, stats=None):
    try:
        return g.resolve(rt, pe, cert, force, stats), None
    except g.Fail as e:
        return None, e


def by_name(rt, name):
    return next(a for a in rt['anchors'] if a['name'] == name)


# ------------------------------------------------------------------------------------------
def test_identity(rt, cert, pe):
    print('\n== identity: the certified copy')
    res, err = resolve_or_fail(rt, pe, cert)
    check('identity path', err is None and res['mode'] == 'identity', str(err) if err else 'mode %s' % res['mode'])
    if res:
        same = sum(res['anchors'][a['index']] == a['rva'] for a in rt['anchors'])
        vsame = sum(res['vtables'][v['index']] == v['rva'] for v in rt['vtables'])
        check('identity RVAs', same == len(rt['anchors']) and vsame == len(rt['vtables']),
              '%d/%d anchors, %d/%d vtables at the certified RVA' % (same, len(rt['anchors']), vsame, len(rt['vtables'])))
    stats = {}
    res, err = resolve_or_fail(rt, pe, cert, True, stats)
    check('search path (identity skipped)', err is None, str(err) if err else '')
    if res:
        same = sum(res['anchors'].get(a['index']) == a['rva'] for a in rt['anchors'])
        vsame = sum(res['vtables'][v['index']] == v['rva'] for v in rt['vtables'])
        check('search path RVAs', same == len(rt['anchors']) and vsame == len(rt['vtables']),
              '%d/%d anchors, %d/%d vtables at the certified RVA; TLS index %#x, image size %#x' % (
                  same, len(rt['anchors']), vsame, len(rt['vtables']), res['tls_index_rva'], res['image_size']))
        kinds = {}
        for i, h in res['how'].items():
            k = re.sub(r' .*', '', h) if not h.startswith('search (') else ('search unique' if h == 'search (1 match)' else 'search, twins')
            kinds[k] = kinds.get(k, 0) + 1
        print('   how: %s' % ', '.join('%s %d' % kv for kv in sorted(kinds.items())))
        fc = {k: v for k, v in stats['false_candidates'].items() if v}
        print('   cost (Python): %.2f s over %d bytes of .text and .rdata, %d hash hits, %d false candidates before the digest (%s)' % (
            stats['seconds'], stats['bytes'], stats['hash_hits'], sum(fc.values()), fc or 'none'))
    return stats


# ------------------------------------------------------------------------------------------
def test_replay13(rt, pe, recert13):
    print('\n== replay13: the figures of the relocation study, section 3')
    d = json.load(open(recert13))
    rebuilt = norm_equal = 0
    unexplained = []
    changed, same = [], []
    for gd in d['guards']:
        name = ' + '.join(gd['names'])
        a = by_name(rt, name)
        rva, size = int(gd['new_rva'], 16), gd['size']
        assert (a['rva'], a['len']) == (rva, size), name
        new = pe.read(rva, size)
        assert g.sha(new) == gd['new_sha256'], name
        dc = int(gd['delta'], 16)
        moved = {int(m['new'], 16): int(m['delta'], 16) for m in gd['moved_fields']}
        old = bytearray(new)
        for x in a['fields']:
            t = g.field_target(pe, rva, x)
            dt = moved.get(t)
            if dt is None:
                if t == 0:
                    dt = 0
                elif x['to'][0] in ('internal', 'same_function') or x['kind'] == 'jumptable':
                    dt = dc
                else:
                    unexplained.append((name, x['at'], hex(t)))
                    dt = 0
            v = struct.unpack_from('<i' if x['kind'] in ('rel32', 'rip') else '<I', new, x['at'])[0]
            nv = v + dc - dt if x['kind'] in ('rel32', 'rip') else v - dt
            struct.pack_into('<i' if x['kind'] in ('rel32', 'rip') else '<I', old, x['at'], nv)
        rebuilt += g.sha(old) == gd['old_sha256']
        norm_equal += g.sha(g.normalized(old, a['mask'])) == a['normalized_sha256']
        (same if gd['identical_bytes'] else changed).append(a)
    check('old digests rebuilt from the table fields', rebuilt == 37, '%d/37 old_sha256, unexplained targets %s' % (rebuilt, unexplained or 'none'))
    check('old normalized digest = the table digest', norm_equal == 37,
          '%d/37 (a table taken before 13 October would have held these digests)' % norm_equal)
    found = g.search(rt, pe)
    uniq = [a['name'] for a in changed if a['search'] and len(found[a['index']]) == 1]
    multi = [(a['name'], len(found[a['index']])) for a in changed if a['search'] and len(found[a['index']]) > 1]
    nowin = [(a['name'], a['certified_matches']) for a in changed if not a['search']]
    check('changed ranges unique by search alone', len(changed) == 19 and len(uniq) == 15,
          '%d of %d; several matches: %s; no window of %d bytes free of fields: %s' % (len(uniq), len(changed), multi, g.K, nowin))
    res, err = resolve_or_fail(rt, pe, None, True)
    if err:
        check('the 37 resolved', False, str(err))
        return
    want = {' + '.join(gd['names']): int(gd['new_rva'], 16) for gd in d['guards']}
    graph = [(a['name'], res['how'][a['index']]) for a in changed if a['name'] not in uniq]
    ok19 = all(res['anchors'][a['index']] == want[a['name']] for a in changed)
    check('changed ranges with the graph', ok19 and len(changed) == 19, '19/19 at their 13 October RVA; the 4: %s' % graph)
    ok37 = sum(res['anchors'][by_name(rt, n)['index']] == r for n, r in want.items())
    how_same = {}
    for a in same:
        h = res['how'][a['index']]
        k = 'search' if h.startswith('search') else re.sub(r' \+0x[0-9a-f]+ of vtable .*', ' of a vtable', h)
        how_same[k] = how_same.get(k, 0) + 1
    check('all 37', ok37 == 37, '%d/37; the 18 that kept their bytes: %s' % (ok37, how_same))


# ------------------------------------------------------------------------------------------
LINE = re.compile(r'\s+([0-9a-f]+):\t([0-9a-f ]+)\t(.*)$')


def code_references(pe, inside):
    """Every rel32 branch and rip-relative disp32 of .text whose instruction or target is
    inside(), by a linear disassembly of the whole section: (insn, length, field pos, target)."""
    n, va, vs, ro, rs = pe.sec['.text']
    p = subprocess.Popen([g.OBJDUMP, '-d', '-M', 'intel', '--insn-width=16', '--start-address=%d' % (g.IB + va),
                          '--stop-address=%d' % (g.IB + va + vs), pe.path], stdout=subprocess.PIPE, text=True, bufsize=1 << 20)
    out = []
    for l in p.stdout:
        if 'rip' not in l and '\tcall' not in l and '\tj' not in l:
            continue
        m = LINE.match(l)
        if not m:
            continue
        at = int(m.group(1), 16) - g.IB
        b = bytes.fromhex(m.group(2).replace(' ', ''))
        txt = m.group(3).strip()
        nxt = at + len(b)
        mn = txt.split()[0] if txt else ''
        mt = re.search(r'#\s*0x([0-9a-f]+)', txt)
        if 'rip' in txt and mt:
            t = int(mt.group(1), 16) - g.IB
            k = b.find(struct.pack('<i', t - nxt))
            if k < 0:
                continue
        elif (mn == 'call' or mn.startswith('j')) and len(b) >= 5 and (
                b[-5] in (0xe8, 0xe9) or (len(b) >= 6 and b[-6] == 0x0f and 0x80 <= b[-5] <= 0x8f)):
            t = (nxt + struct.unpack('<i', b[-4:])[0]) & 0xffffffff
            k = len(b) - 4
        else:
            continue
        if inside(at) or inside(t):
            out.append((at, len(b), k, t))
    p.wait()
    return out


def rotation(blocks):
    """The RVA map of rotating each block (A, B, C): [A, B) and [B, C) swap places."""
    def m(r):
        for A, B, C in blocks:
            if A <= r < B:
                return r + (C - B)
            if B <= r < C:
                return r - (B - A)
        return r
    return m


def build_moved(pe, text_block, rdata_block, path):
    """A copy of pe where, in one block of .text and one of .rdata, two runs swap places, kept
    coherent for everything the resolver reads: every rel32 and rip-relative reference of .text
    that leaves from or lands in a block, the image-relative fields and jump tables of the moved
    functions, the base-relocated pointers into a block (vtable slots), the base relocations of
    the moved .rdata pages and the .pdata entries of the moved functions. Not kept: 32-bit RVAs
    held in .rdata (RTTI, unwind and exception data), which the resolver does not read."""
    blocks = [text_block, rdata_block]
    m = rotation(blocks)
    inside = lambda r: any(A <= r < C for A, B, C in blocks)
    A2, B2, C2 = rdata_block
    assert A2 % 0x1000 == 0 and B2 % 0x1000 == 0 and C2 % 0x1000 == 0, 'the .rdata block is whole pages'
    f = bytearray(pe.f)
    for A, B, C in blocks:
        oA, oB, oC = pe.r2o(A), pe.r2o(B), pe.r2o(C)
        f[oA:oC] = pe.f[oB:oC] + pe.f[oA:oB]
    t0 = time.perf_counter()
    refs = code_references(pe, inside)
    scan = time.perf_counter() - t0
    for at, n, k, t in refs:
        new_at = m(at)
        struct.pack_into('<i', f, pe.r2o(new_at) + k, m(t) - (new_at + n))
    A, B, C = text_block
    bs, es = pe.pdata()
    fns = [(b, e) for b, e in zip(bs, es) if A <= b < C]
    imagerel = 0
    for b, e in fns:
        for x in g.fields(pe, b, e - b):
            if x['kind'] in ('imagerel', 'jumptable') and inside(x['target']):
                struct.pack_into('<I', f, pe.r2o(m(b + x['at'])), m(x['target']))
                imagerel += 1
    relocs = 0
    for r in pe.relocs():
        if A <= r < C:
            raise SystemExit('a base relocation inside the moved .text block: pick another one')
        o = pe.r2o(r)
        if o is None:
            continue
        q = struct.unpack_from('<Q', pe.f, o)[0] - g.IB
        if inside(q):
            struct.pack_into('<Q', f, pe.r2o(m(r)), g.IB + m(q))
            relocs += 1
    # The base relocation blocks of the moved .rdata pages: same offsets in the page, new page.
    va, size = pe.dirs[5]
    o = pe.r2o(va)
    end = o + size
    pages = 0
    while o + 8 <= end:
        page, bsz = struct.unpack_from('<II', pe.f, o)
        if bsz < 8:
            break
        if A2 <= page < C2:
            struct.pack_into('<I', f, o, m(page))
            pages += 1
        o += bsz
    # .pdata: the entries of the moved functions, moved and sorted again in their own slots.
    n_, va, vs, ro, rs = pe.sec['.pdata']
    slots, entries = [], []
    for i in range(vs // 12):
        b, e, u = struct.unpack_from('<III', pe.f, ro + 12 * i)
        if A <= b < C:
            slots.append(ro + 12 * i)
            entries.append((m(b), m(b) + (e - b), u))
    if slots != list(range(slots[0], slots[0] + 12 * len(slots), 12)):
        raise SystemExit('.pdata entries of the block are not contiguous')
    for o, ent in zip(slots, sorted(entries)):
        struct.pack_into('<III', f, o, *ent)
    with open(path, 'wb') as fh:
        fh.write(f)
    return m, dict(code_references=len(refs), scan_seconds=round(scan, 1), imagerel_and_jump_table_entries=imagerel,
                   relocated_pointers=relocs, reloc_pages=pages, pdata_entries=len(entries), functions=len(fns))


def blocks_for(pe, rt):
    """The .text block: from item_definition_getter to the end of the function that holds the
    twin of bag_item_constructor (0x13cb0f0), split after the item factory, so the factory moves
    one way and the bag item constructor, the conditional items and their predicates the other.
    The .rdata block: the pages 0x225c000..0x2260000, split after the first, so the item context
    vtable moves one way and the bag item, conditional item and predicate vtables the other."""
    A = by_name(rt, 'bag:item_definition_getter')['rva']
    assert pe.func_at(A)
    C = pe.func_of(0x13cb0f0)[1]
    factory_end = pe.func_of(by_name(rt, 'bag:item_factory_bag_case')['rva'])[1]
    bs, es = pe.pdata()
    B = next(b for b in bs if b >= factory_end)
    rdata = (0x225c000, 0x225d000, 0x2260000)
    # Nothing the resolver reads in .rdata may straddle a cut.
    pieces = [(v['rva'] + s['offset'], 8) for v in rt['vtables'] for s in v['slots']] + [(c['rva'], c['len']) for c in rt['contents']] + \
             [(a['rva'], a['len']) for a in rt['anchors'] if a['section'] == '.rdata']
    for r, n in pieces:
        for cut in rdata:
            assert not (r < cut < r + n), 'a piece at %#x straddles %#x' % (r, cut)
    return (A, B, C), rdata


def test_moved(rt, cert, pe, tmp):
    print('\n== moved: a copy with a block of .text and a block of .rdata rotated')
    text_block, rdata_block = blocks_for(pe, rt)
    path = os.path.join(tmp, 'moved.exe')
    m, info = build_moved(pe, text_block, rdata_block, path)
    for label, (A, B, C) in (('.text', text_block), ('.rdata', rdata_block)):
        print('   %s [%#x, %#x) split at %#x: [%#x, %#x) moves by +%#x, [%#x, %#x) by -%#x; %d anchors and %d vtables inside' % (
            label, A, C, B, A, B, C - B, B, C, B - A, sum(A <= a['rva'] < C for a in rt['anchors']), sum(A <= v['rva'] < C for v in rt['vtables'])))
    print('   rewritten: %d code references (whole .text disassembled in %.1f s), %d image-relative and jump-table entries, %d relocated pointers, %d base relocation pages, %d .pdata entries of %d functions' % (
        info['code_references'], info['scan_seconds'], info['imagerel_and_jump_table_entries'], info['relocated_pointers'], info['reloc_pages'], info['pdata_entries'], info['functions']))
    moved = g.PE(path)
    # The copy is coherent for what the resolver reads: same normalized bytes at the new place,
    # every field pointing at the new place of what it pointed at, every slot holding the new
    # place of its function, every content where its field now points.
    bad = []
    for a in rt['anchors']:
        if not g.digest_at(moved, a, m(a['rva'])):
            bad.append((a['name'], 'digest'))
        for x in a['fields']:
            if g.field_target(moved, m(a['rva']), x) != m(g.field_target(pe, a['rva'], x)):
                bad.append((a['name'], x['at']))
    for v in rt['vtables']:
        for s in v['slots']:
            if moved.u64(m(v['rva']) + s['offset']) != g.IB + m(rt['anchors'][s['anchor']]['rva']):
                bad.append((v['name'], s['offset']))
    for c in rt['contents']:
        if not g.content_ok(moved, c, m(c['rva'])):
            bad.append(('content', hex(c['rva'])))
    check('moved copy coherent for what the resolver reads', not bad, '%d anchors, %d vtables, %d contents: %d mismatches %s' % (
        len(rt['anchors']), len(rt['vtables']), len(rt['contents']), len(bad), bad[:5]))
    res, err = resolve_or_fail(rt, moved, cert)
    if err:
        check('moved copy resolved', False, str(err))
        return path, m
    wrong = [(a['name'], hex(res['anchors'].get(a['index'], 0)), hex(m(a['rva']))) for a in rt['anchors'] if res['anchors'].get(a['index']) != m(a['rva'])]
    vwrong = [v['name'] for v in rt['vtables'] if res['vtables'][v['index']] != m(v['rva'])]
    check('moved copy resolved to the moved RVAs', res['mode'] == 'relocated' and not wrong and not vwrong,
          'mode %s; %d/%d anchors at the expected RVA (%d of them moved), %d/%d vtables (%d of them moved); wrong %s %s' % (
              res['mode'], len(rt['anchors']) - len(wrong), len(rt['anchors']), sum(m(a['rva']) != a['rva'] for a in rt['anchors']),
              len(rt['vtables']) - len(vwrong), len(rt['vtables']), sum(m(v['rva']) != v['rva'] for v in rt['vtables']), wrong[:5], vwrong))
    for a in rt['anchors']:
        if m(a['rva']) != a['rva']:
            print('   %-55s %#9x -> %#9x  %s' % (a['name'][:55], a['rva'], res['anchors'][a['index']], res['how'][a['index']]))
    for v in rt['vtables']:
        if m(v['rva']) != v['rva']:
            print('   vtable %-48s %#9x -> %#9x  %s' % (v['name'][:48], v['rva'], res['vtables'][v['index']], res['how']['vt%d' % v['index']]))
    return path, m


# ------------------------------------------------------------------------------------------
def broken(attr, replacement):
    """Runs with g.<attr> replaced, then restores it: the check that should catch a change,
    broken on purpose."""
    class Ctx:
        def __enter__(self):
            self.old = getattr(g, attr)
            setattr(g, attr, replacement(self.old))

        def __exit__(self, *a):
            setattr(g, attr, self.old)
    return Ctx()


def expect_fail(name, rt, cert, path, anchor, rule, force=False):
    pe = g.PE(path)
    res, err = resolve_or_fail(rt, pe, cert, force)
    ok = err is not None and err.anchor == anchor and err.rule == rule
    check(name, ok, 'not resolved: %s: rule %s: %s' % (err.anchor, err.rule, err.detail) if err else 'resolved (%s)' % res['mode'])
    return pe


def test_negatives(rt, cert, pe, tmp, moved_path, m):
    print('\n== negatives')
    # (a) A field offset inside an anchor: character_inventory_getter reads character +0x3f0.
    a = by_name(rt, 'bag:character_inventory_getter')
    ins = g.disasm(pe.path, a['rva'], a['len'])
    at, b, txt = next(i for i in ins if re.search(r'\+0x3[ef][08]\]', i[2]))
    disp = int(re.search(r'\+0x(3[ef][08])\]', txt).group(1), 16)
    pos = at + b.find(struct.pack('<I', disp))
    assert pos - a['rva'] not in [p + q for p in a['mask'] for q in range(4)], 'a field offset is never masked'
    f = bytearray(pe.f)
    f[pe.r2o(pos)] ^= 0x08
    path = os.path.join(tmp, 'offset.exe')
    open(path, 'wb').write(f)
    print('   (a) %s: the field offset %#x at +%d changed to %#x' % (a['name'], disp, pos - a['rva'], disp ^ 0x08))
    expect_fail('(a) a field offset changed fails that anchor', rt, cert, path, a['name'], '3')
    with broken('digest_at', lambda old: lambda p, x, at: True if x['name'] == a['name'] else old(p, x, at)):
        res, err = resolve_or_fail(rt, g.PE(path), cert)
    check('(a) with its digest check broken, the change goes through', err is None,
          'resolved, %s at %#x with the changed offset' % (a['name'], res['anchors'][a['index']]) if res else str(err))
    os.remove(path)

    # (b) The twin of buff_lookup as the target of modifier_value's call.
    mv = by_name(rt, 'mf:modifier_value')
    bl = by_name(rt, 'mf:buff_lookup')
    x = next(x for x in mv['fields'] if x['to'][0] == 'anchor' and x['to'][1] == bl['index'])
    twin = 0x431bc0
    f = bytearray(pe.f)
    p = mv['rva'] + x['at']
    struct.pack_into('<i', f, pe.r2o(p), twin - (p + 4 + x['tail']))
    path = os.path.join(tmp, 'twin.exe')
    open(path, 'wb').write(f)
    tp = g.PE(path)
    print('   (b) modifier_value +%d now calls %#x; mf:buff_lookup matches there: %s' % (x['at'], twin, g.digest_at(tp, bl, twin)))
    expect_fail('(b) the twin of buff_lookup is rejected', rt, cert, path, bl['name'], '4')
    with broken('callee_ok', lambda old: lambda rt_, pe_, c, t: True):
        res, err = resolve_or_fail(rt, g.PE(path), cert)
    check('(b) with the callee check broken, the twin is taken', err is None and res['anchors'][bl['index']] == twin,
          'resolved, mf:buff_lookup at %#x' % res['anchors'][bl['index']] if res else str(err))
    os.remove(path)

    # (c) Ambiguity: on the moved copy, stackable_at_0x98 (found only by search) copied over a
    # function nothing in the table reads, so that two places pass every check.
    s = by_name(rt, 'item:stackable_at_0x98')
    assert not s['fields'] and s['function'] is None and s['certified_matches'] == 1
    assert not any(x['to'][:2] == ('anchor', s['index']) for b_ in rt['anchors'] for x in b_['fields'])
    assert not any(sl['anchor'] == s['index'] for v in rt['vtables'] for sl in v['slots'])
    mp = g.PE(moved_path)
    used = [(a_['rva'], a_['rva'] + a_['len']) for a_ in rt['anchors']] + [(c['rva'] - c['offset'], c['rva'] - c['offset'] + c['len']) for c in rt['callees']]
    bs, es = mp.pdata()
    dst = next(b_ for b_, e_ in zip(bs, es) if 0x500000 <= b_ and e_ - b_ >= 64 and not any(lo < e_ and b_ < hi for lo, hi in used))
    f = bytearray(mp.f)
    f[mp.r2o(dst):mp.r2o(dst) + s['len']] = mp.read(m(s['rva']), s['len'])
    path = os.path.join(tmp, 'ambiguous.exe')
    open(path, 'wb').write(f)
    print('   (c) %s copied to %#x (the start of a %d-byte function no anchor or callee is in)' % (s['name'], dst, es[bs.index(dst)] - dst))
    expect_fail('(c) two places that pass every check fail closed', rt, cert, path, s['name'], '2')
    with broken('search_accepts', lambda old: lambda a_, places: a_['certified_matches'] == 1 and len(places) >= 1):
        res, err = resolve_or_fail(rt, g.PE(path), cert)
    check('(c) with the uniqueness rule broken, one of the two is taken', err is None,
          'resolved, %s at %#x (true place %#x)' % (s['name'], res['anchors'][s['index']], m(s['rva'])) if res else str(err))
    os.remove(path)


# ------------------------------------------------------------------------------------------
def representations(w):
    out = [w.hex(), ' '.join('%02x' % c for c in w), ','.join('0x%02x' % c for c in w), ', '.join('0x%02x' % c for c in w),
           ''.join('\\x%02x' % c for c in w), ', '.join(str(c) for c in w), ','.join(str(c) for c in w), ' '.join(str(c) for c in w),
           '%016x' % int.from_bytes(w, 'little')]
    for v in (int.from_bytes(w, 'little'), int.from_bytes(w, 'big')):
        if v >= 10 ** 11:
            out.append(str(v))
    return out


def test_leak(rt, pe, files):
    print('\n== leak: no bytes of the game in the generated files')
    text = {p: open(p).read().lower() for p in files}
    ranges = [('anchor ' + a['name'], a['rva'], a['len']) for a in rt['anchors']]
    ranges += [('callee %#x' % c['rva'], c['rva'] - c['offset'], c['len']) for c in rt['callees']]
    ranges += [('content %#x' % c['rva'], c['rva'], c['len']) for c in rt['contents']]
    # The search itself, seen to find what it looks for: one window of the first anchor, planted
    # in memory in each representation, must be found every time.
    a0 = rt['anchors'][0]
    w0 = pe.read(a0['rva'] + 8, 8)
    planted = sum(any(rep in ('x ' + r.upper() + ' y').lower() for rep in representations(w0)) for r in representations(w0))
    check('the leak search finds a planted window', planted == len(representations(w0)),
          '%d/%d representations of one window of %s found when planted' % (planted, len(representations(w0)), a0['name']))
    windows = hits = 0
    found = []
    for name, r, n in ranges:
        raw = pe.read(r, n)
        for i in range(0, n - 7):
            w = raw[i:i + 8]
            windows += 1
            for rep in representations(w):
                for p, t in text.items():
                    if rep in t:
                        hits += 1
                        found.append((name, i, os.path.basename(p)))
    check('no 8-byte window of a kept range in the generated files', hits == 0,
          '%d windows of %d ranges, %d representations each (hex, spaced, 0x.., \\x.., decimal bytes, LE/BE integers), %d files (%s): %d coincidences %s' % (
              windows, len(ranges), len(representations(b'\x01' * 8)), len(files), ', '.join(os.path.relpath(p, REPO) for p in files), hits, found[:5]))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('exe')
    ap.add_argument('--tmp', required=True)
    ap.add_argument('--recert13', default=os.path.join(REPO, 'docs', 'audit', '2026-10-13-recertification-build-208574.json'))
    ap.add_argument('--table', default=g.DEFAULT_TABLE)
    ap.add_argument('--certificate', default=g.DEFAULT_CERT)
    ap.add_argument('--only', default='identity,replay13,moved,negatives,leak')
    args = ap.parse_args()
    tmp = os.path.realpath(args.tmp)
    if tmp == REPO or tmp.startswith(REPO + os.sep):
        raise SystemExit('--tmp must be outside the repository')
    work = tempfile.mkdtemp(prefix='relocation-', dir=tmp)
    only = set(args.only.split(','))
    rt = g.load_table(args.table)
    cert = g.parse_certificate(open(args.certificate).read())
    pe = g.PE(args.exe)
    print('table %s (%d anchors, %d vtables), copy sha256 %s' % (os.path.relpath(args.table, REPO), len(rt['anchors']), len(rt['vtables']), g.sha(pe.f)))
    try:
        if 'identity' in only:
            test_identity(rt, cert, pe)
        if 'replay13' in only:
            test_replay13(rt, pe, args.recert13)
        moved = None
        if 'moved' in only or 'negatives' in only:
            moved = test_moved(rt, cert, pe, work)
        if 'negatives' in only:
            test_negatives(rt, cert, pe, work, *moved)
        if 'leak' in only:
            files = [args.table, g.DEFAULT_EVIDENCE, os.path.join(REPO, 'core', 'src', 'relocation.rs'), os.path.join(HERE, 'README.md')]
            test_leak(rt, pe, [p for p in files if os.path.exists(p)])
    finally:
        shutil.rmtree(work)
    print('\n%s' % ('all checks as expected' if not FAILED else 'NOT as expected: %s' % FAILED))
    return 1 if FAILED else 0


if __name__ == '__main__':
    sys.exit(main())
