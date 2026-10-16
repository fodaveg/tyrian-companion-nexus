#!/usr/bin/env python3
"""The relocation table of the readers, and the resolver that uses it.

Generate (from the certified copy of the game's executable):

    python3 -I gen_relocation.py <certified Gw2-64.exe> <recertification json> <certificate_data.rs>
        [-o core/src/relocation_data.rs] [--evidence scripts/relocation/evidence.json]

Resolve (locate every anchor of the table in another executable, the semiautomatic
recertification):

    python3 -I gen_relocation.py --resolve <other Gw2-64.exe> [--table relocation_data.rs]
        [--certificate certificate_data.rs] [--force-search] [--json out.json]

The table keeps no byte of the game: for each anchor its RVA, length, the positions of the fields
that move with the code (the mask), the SHA-256 of its bytes with those fields set to zero (the
normalized digest), how many places of the certified build held it, the field targets as
identities (another anchor, a fingerprinted callee, a .rdata digest, an offset inside itself or
its function, the image base, the TLS index) and, for the anchors that are searched, a 32-bit
rolling hash of one window of K >= 16 bytes of it. See README.md in this directory for why that
does not let anyone rebuild the bytes, and for the rules the resolver applies (study, section 4).

The generator disassembles with x86_64-w64-mingw32-objdump. The resolver does not: it reads the
table and the other file only, as the addon will.
"""
import argparse
import bisect
import hashlib
import json
import os
import re
import struct
import subprocess
import sys
import time

IB = 0x140000000
OBJDUMP = 'x86_64-w64-mingw32-objdump'
# Rolling hash (Rabin-Karp): h(w) = sum(w[j] * BASE^(K-1-j)) mod PRIME, over K bytes.
K = 16
PRIME = 4294967291  # the largest prime below 2^32
BASE = 16777619
# A constructor longer than this is not fingerprinted whole: the instructions within
# CTOR_WINDOW bytes of the lea are.
CTOR_MAX = 4096
CTOR_WINDOW = 48
LEAF_MAX = 512

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
DEFAULT_TABLE = os.path.join(REPO, 'core', 'src', 'relocation_data.rs')
DEFAULT_CERT = os.path.join(REPO, 'core', 'src', 'certificate_data.rs')
DEFAULT_EVIDENCE = os.path.join(HERE, 'evidence.json')


def sha(b):
    return hashlib.sha256(bytes(b)).hexdigest()


# ------------------------------------------------------------------------------------------
# PE
# ------------------------------------------------------------------------------------------
class PE:
    """A PE32+ file read whole: sections, header fields, .pdata and base relocations."""

    def __init__(self, path, data=None):
        self.path = path
        self.f = data if data is not None else open(path, 'rb').read()
        f = self.f
        if f[:2] != b'MZ':
            raise ValueError('not MZ')
        pe = struct.unpack_from('<I', f, 0x3c)[0]
        self.pe = pe
        if f[pe:pe + 4] != b'PE\0\0':
            raise ValueError('not PE')
        self.machine = struct.unpack_from('<H', f, pe + 4)[0]
        ns = struct.unpack_from('<H', f, pe + 6)[0]
        osz = struct.unpack_from('<H', f, pe + 20)[0]
        self.magic = struct.unpack_from('<H', f, pe + 24)[0]
        self.image_base = struct.unpack_from('<Q', f, pe + 24 + 24)[0]
        self.image_size = struct.unpack_from('<I', f, pe + 24 + 56)[0]
        self.dirs = [struct.unpack_from('<II', f, pe + 24 + 112 + 8 * i) for i in range(16)]
        self.secs = []
        for i in range(ns):
            o = pe + 24 + osz + 40 * i
            n = f[o:o + 8].rstrip(b'\0').decode('latin-1')
            vs, va, rs, ro = struct.unpack_from('<IIII', f, o + 8)
            self.secs.append((n, va, vs, ro, rs))
        self.sec = {s[0]: s for s in self.secs}
        self._pdata = None
        self._relocs = None

    def r2o(self, r):
        for n, va, vs, ro, rs in self.secs:
            if va <= r < va + vs:
                return r - va + ro if r - va < rs else None
        return None

    def o2r(self, o):
        for n, va, vs, ro, rs in self.secs:
            if ro <= o < ro + rs:
                return o - ro + va
        return None

    def secof(self, r):
        for n, va, vs, ro, rs in self.secs:
            if va <= r < va + vs:
                return n
        return None

    def read(self, r, n):
        """n bytes at RVA r, or None if any of them is outside the file's sections."""
        o = self.r2o(r)
        if o is None or self.r2o(r + n - 1) is None or self.secof(r) != self.secof(r + n - 1):
            return None
        return self.f[o:o + n]

    def u32(self, r):
        b = self.read(r, 4)
        return None if b is None else struct.unpack('<I', b)[0]

    def u64(self, r):
        b = self.read(r, 8)
        return None if b is None else struct.unpack('<Q', b)[0]

    def section_bytes(self, name):
        n, va, vs, ro, rs = self.sec[name]
        return self.f[ro:ro + min(vs, rs)], va

    def pdata(self):
        """(sorted begins, ends) of the RUNTIME_FUNCTION entries."""
        if self._pdata is None:
            n, va, vs, ro, rs = self.sec['.pdata']
            ent = []
            for i in range(vs // 12):
                b, e, u = struct.unpack_from('<III', self.f, ro + 12 * i)
                if b:
                    ent.append((b, e))
            ent.sort()
            self._pdata = ([b for b, _ in ent], [e for _, e in ent])
        return self._pdata

    def func_at(self, r):
        """The .pdata function that starts at r, as (begin, end), or None."""
        bs, es = self.pdata()
        i = bisect.bisect_left(bs, r)
        return (bs[i], es[i]) if i < len(bs) and bs[i] == r else None

    def func_of(self, r):
        """The .pdata function that holds r, as (begin, end), or None."""
        bs, es = self.pdata()
        i = bisect.bisect_right(bs, r) - 1
        return (bs[i], es[i]) if i >= 0 and bs[i] <= r < es[i] else None

    def relocs(self):
        """Sorted RVAs of the DIR64 base relocations."""
        if self._relocs is None:
            va, size = self.dirs[5]
            out = []
            o = self.r2o(va)
            end = o + size
            while o + 8 <= end:
                page, bs = struct.unpack_from('<II', self.f, o)
                if bs < 8:
                    break
                for i in range((bs - 8) // 2):
                    e = struct.unpack_from('<H', self.f, o + 8 + 2 * i)[0]
                    if e >> 12 == 10:
                        out.append(page + (e & 0xfff))
                o += bs
            out.sort()
            self._relocs = out
        return self._relocs

    def relocs_in(self, r0, r1):
        """Starts of the DIR64 relocations whose 8 bytes overlap [r0, r1)."""
        rl = self.relocs()
        i = bisect.bisect_left(rl, r0 - 7)
        out = []
        while i < len(rl) and rl[i] < r1:
            out.append(rl[i])
            i += 1
        return out

    def tls_index_rva(self):
        va, size = self.dirs[9]
        q = self.u64(va + 16)
        return None if q is None else q - self.image_base


# ------------------------------------------------------------------------------------------
# Disassembly and the fields that move with the code (generation only)
# ------------------------------------------------------------------------------------------
LINE = re.compile(r'\s+([0-9a-f]+):\t([0-9a-f ]+)\t(.*)$')
REG64 = {'eax': 'rax', 'ecx': 'rcx', 'edx': 'rdx', 'ebx': 'rbx', 'esp': 'rsp', 'ebp': 'rbp',
         'esi': 'rsi', 'edi': 'rdi'}
WIDTH = {'byte': 1, 'word': 2, 'dword': 4, 'fword': 6, 'qword': 8, 'tbyte': 10, 'xmmword': 16,
         'ymmword': 32, 'zmmword': 64}


def disasm(path, rva, size):
    out = subprocess.run([OBJDUMP, '-d', '-M', 'intel', '--insn-width=16',
                          '--start-address=%d' % (IB + rva), '--stop-address=%d' % (IB + rva + size), path],
                         capture_output=True, text=True, check=True).stdout
    ins = []
    for l in out.splitlines():
        m = LINE.match(l)
        if m:
            ins.append((int(m.group(1), 16) - IB, bytes.fromhex(m.group(2).replace(' ', '')), m.group(3).strip()))
    return ins


def fields(pe, rva, size, ins=None):
    """Every field of [rva, rva+size) that moves with the code, as dicts {at, kind, target, tail,
    width}: rel32 branches, rip-relative disp32, image-relative disp32 (base register loaded with
    the image base by a rip lea) and the entries of jump tables kept inside the range. The model of
    docs/audit/2026-10-16-relocation-study/normsig.py, which rebuilt the 37 old digests of
    13 October 2026."""
    res = []
    ibregs = set()
    for at, b, txt in (ins if ins is not None else disasm(pe.path, rva, size)):
        nxt = at + len(b)
        pos = at - rva
        if nxt > rva + size:
            break
        mn = txt.split()[0] if txt else ''
        m = re.search(r'#\s*0x([0-9a-f]+)', txt)
        if 'rip' in txt and m:
            tgt = int(m.group(1), 16) - IB
            le = struct.pack('<i', tgt - nxt)
            k = b.find(le)
            if k < 0:
                raise SystemExit('rip field not found at %x %s' % (at, txt))
            w = re.search(r'\b(%s) ptr' % '|'.join(WIDTH), txt, re.I)
            res.append(dict(at=pos + k, kind='rip', target=tgt, tail=len(b) - k - 4,
                            width=WIDTH[w.group(1).lower()] if w else None, mnemonic=mn))
            mm = re.match(r'lea\s+(\w+),', txt)
            if mm and tgt == 0:
                ibregs.add(mm.group(1))
            elif mm:
                ibregs.discard(mm.group(1))
            continue
        if (mn == 'call' or mn.startswith('j')) and len(b) >= 5 and (
                b[-5] in (0xe8, 0xe9) or (len(b) >= 6 and b[-6] == 0x0f and 0x80 <= b[-5] <= 0x8f)):
            tgt = (nxt + struct.unpack('<i', b[-4:])[0]) & 0xffffffff
            res.append(dict(at=pos + len(b) - 4, kind='rel32', target=tgt, tail=0, width=None, mnemonic=mn))
            continue
        mm = re.search(r'\[(\w+)(?:\+(\w+)\*\d)?\+0x([0-9a-f]+)\]', txt)
        if mm and (mm.group(1) in ibregs or (mm.group(2) or '') in ibregs):
            disp = int(mm.group(3), 16)
            k = b.find(struct.pack('<I', disp))
            if k >= 0:
                w = re.search(r'\b(%s) ptr' % '|'.join(WIDTH), txt, re.I)
                res.append(dict(at=pos + k, kind='imagerel', target=disp, tail=0,
                                width=WIDTH[w.group(1).lower()] if w else None, mnemonic=mn))
        mw = re.match(r'(?:mov|movzx|movsxd|add|sub|xor|and|or|lea|imul|shl|shr|sar)\s+(\w+),', txt)
        if mw:
            r = REG64.get(mw.group(1), mw.group(1))
            if mn != 'add':
                ibregs.discard(r)
                ibregs.discard(mw.group(1))
    raw = pe.read(rva, size)
    taken = {x['at'] for x in res}
    for x in list(res):
        if x['kind'] != 'imagerel' or not (rva <= x['target'] < rva + size):
            continue
        q = x['target'] - rva
        while q + 4 <= size and q not in taken:
            v = struct.unpack_from('<I', raw, q)[0]
            if not (rva - 0x1000 <= v < rva + size + 0x1000):
                break
            res.append(dict(at=q, kind='jumptable', target=v, tail=0, width=None, mnemonic=None))
            taken.add(q)
            q += 4
    res.sort(key=lambda x: x['at'])
    return res


def leaf_len(pe, rva):
    """Length of a leaf with no .pdata entry: up to and including its first ret or jmp."""
    for at, b, txt in disasm(pe.path, rva, LEAF_MAX):
        mn = txt.split()[0] if txt else ''
        if mn in ('ret', 'jmp') or mn.startswith('rep') and 'ret' in txt:
            return at + len(b) - rva
    raise SystemExit('no ret within %d bytes of %#x' % (LEAF_MAX, rva))


# ------------------------------------------------------------------------------------------
# Normalized digests and the rolling hash
# ------------------------------------------------------------------------------------------
def normalized(raw, mask):
    """The bytes with every relocatable field (4 bytes at each mask position) set to zero."""
    b = bytearray(raw)
    for p in mask:
        b[p:p + 4] = b'\0\0\0\0'
    return bytes(b)


def zero_pointers(raw, base, pointers):
    """Content bytes with every 8-byte absolute pointer (at offsets relative to base) zeroed."""
    b = bytearray(raw)
    for p in pointers:
        for q in range(max(0, p), min(len(b), p + 8)):
            b[q] = 0
    return bytes(b)


def rolling(w):
    h = 0
    for x in w:
        h = (h * BASE + x) % PRIME
    return h


def rolling_scan(buf, k, wanted):
    """Every position i of buf whose k-byte window hashes into wanted, as (i, hash)."""
    out = []
    if len(buf) < k:
        return out
    top = pow(BASE, k - 1, PRIME)
    h = 0
    for j in range(k):
        h = (h * BASE + buf[j]) % PRIME
    if h in wanted:
        out.append((0, h))
    p, bb = PRIME, BASE
    for i in range(k, len(buf)):
        h = ((h - buf[i - k] * top) * bb + buf[i]) % p
        if h in wanted:
            out.append((i - k + 1, h))
    return out


def unmasked_windows(size, mask, k):
    """Starts of the contiguous k-byte windows that hold no masked byte, in order."""
    bad = [False] * size
    for p in mask:
        for q in range(p, min(size, p + 4)):
            bad[q] = True
    out = []
    run = 0
    for i in range(size):
        run = 0 if bad[i] else run + 1
        if run >= k:
            out.append(i - k + 1)
    return out


def occurrences(hay, needle, cap):
    n = 0
    k = hay.find(needle)
    while k >= 0 and n < cap:
        n += 1
        k = hay.find(needle, k + 1)
    return n


def masked_matches(pe, section, raw, mask, align=1):
    """Every place of the section whose bytes equal raw outside the mask (exhaustive)."""
    buf, va = pe.section_bytes(section)
    n = len(raw)
    bad = [False] * n
    for p in mask:
        for q in range(p, min(n, p + 4)):
            bad[q] = True
    best = (0, 0)
    i = 0
    while i < n:
        if bad[i]:
            i += 1
            continue
        j = i
        while j < n and not bad[j]:
            j += 1
        if j - i > best[1] - best[0]:
            best = (i, j)
        i = j
    if best[1] == best[0]:
        return None
    needle = raw[best[0]:best[1]]
    want = normalized(raw, mask)
    hits = []
    s = 0
    while True:
        k = buf.find(needle, s)
        if k < 0:
            break
        s = k + 1
        start = k - best[0]
        if start < 0 or start + n > len(buf) or (va + start) % align:
            continue
        if normalized(buf[start:start + n], mask) == want:
            hits.append(va + start)
    return hits


# ------------------------------------------------------------------------------------------
# Generation
# ------------------------------------------------------------------------------------------
def parse_certificate(text):
    spans = [dict(name=m.group(1), offset=int(m.group(2), 16), len=int(m.group(3)), sha256=m.group(4))
             for m in re.finditer(r'Span \{ name: "([^"]+)", offset: (0x[0-9a-f]+), len: (\d+), sha256: "([0-9a-f]{64})"', text)]
    slots = [dict(name=m.group(1), offset=int(m.group(2), 16), rva=int(m.group(3), 16))
             for m in re.finditer(r'Slot \{ name: "([^"]+)", offset: (0x[0-9a-f]+), rva: (0x[0-9a-f]+)', text)]
    sections = [dict(name=m.group(1).replace('\\0', ''), rva=int(m.group(2), 16), virtual_size=int(m.group(3), 16), file_offset=int(m.group(4), 16))
                for m in re.finditer(r'Section \{ name: \*b"([^"]+)", rva: (0x[0-9a-f]+), virtual_size: (0x[0-9a-f]+), file_offset: (0x[0-9a-f]+)', text)]
    image_size = int(re.search(r'IMAGE_SIZE: u32 = (0x[0-9a-f]+)', text).group(1), 16)
    tls_dir = int(re.search(r'TLS_DIRECTORY_RVA: u32 = (0x[0-9a-f]+)', text).group(1), 16)
    return dict(spans=spans, slots=slots, sections=sections, image_size=image_size, tls_directory_rva=tls_dir)


# The slots the study (section 3) asks to add: the two functions that are only located through a
# slot no certificate check named until now.
NEW_SLOTS = [('bag_item (ItCliBag)', 0x260, 'item:null_stackable'),
             ('player_stats', 0x260, 'item:stack_quantity_getter'),
             ('conditional predicate C', 0x28, 'item:stack_quantity_getter')]


def generate(exe, recert_path, cert_path, log=print):
    pe = PE(exe)
    recert = json.load(open(recert_path))
    cert = parse_certificate(open(cert_path).read())
    exe_sha = sha(pe.f)
    if pe.image_base != IB:
        raise SystemExit('image base')
    # The copy must hold the certificate: generating from anything else is not a recertification.
    for s in cert['spans']:
        if sha(pe.f[s['offset']:s['offset'] + s['len']]) != s['sha256']:
            raise SystemExit('the copy does not hold the certificate: %s' % s['name'])
    for s in cert['slots']:
        r = pe.o2r(s['offset'])
        if pe.u64(r) != IB + s['rva']:
            raise SystemExit('the copy does not hold the certificate: %s' % s['name'])
    tls_index = pe.tls_index_rva()

    anchors = []  # dicts
    by_rva = {}

    def add_anchor(a):
        a['index'] = len(anchors)
        anchors.append(a)
        by_rva.setdefault(a['rva'], a['index'])
        return a

    for s in cert['spans']:
        r = pe.o2r(s['offset'])
        add_anchor(dict(name=s['name'], kind='span', section=pe.secof(r), rva=r, offset=s['offset'],
                        len=s['len'], raw_sha256=s['sha256']))

    # vtables: certified RVA from the recertification, slots from the certificate.
    vts = []
    vt_by_name = {}
    for v in recert['vtables']:
        x = dict(name=v['name'], rva=int(v['new_rva'], 16), slots=[], evidence_leas=[int(t, 16) for t in re.findall(r'0x[0-9a-f]+', v['evidence'].split('rip-relative lea:')[1].split(';')[0])] if 'rip-relative lea:' in v['evidence'] else [])
        x['index'] = len(vts)
        vts.append(x)
        vt_by_name[x['name']] = x
    span_by_rva = {a['rva']: a for a in anchors}
    for s in cert['slots']:
        m = re.match(r'vtable (.+) \+(0x[0-9a-f]+)$', s['name'])
        v = vt_by_name[m.group(1)]
        so = int(m.group(2), 16)
        if pe.o2r(s['offset']) - so != v['rva']:
            raise SystemExit('slot %s is not in its vtable' % s['name'])
        fn = span_by_rva.get(s['rva'])
        if fn is None:
            raise SystemExit('slot %s holds no anchor' % s['name'])
        v['slots'].append(dict(offset=so, anchor=fn['index'], added=False))
    for vname, so, fname in NEW_SLOTS:
        v = vt_by_name[vname]
        fn = next(a for a in anchors if a['name'] == fname)
        if pe.u64(v['rva'] + so) != IB + fn['rva']:
            raise SystemExit('new slot %s +%#x does not hold %s' % (vname, so, fname))
        v['slots'].append(dict(offset=so, anchor=fn['index'], added=True))
    vt_by_rva = {v['rva']: v for v in vts}

    # Constructor anchors: the functions that install each vtable with a rip-relative lea.
    text, tva = pe.section_bytes('.text')
    leas = {}
    for m in re.finditer(rb'[\x48\x4c]\x8d[\x05\x0d\x15\x1d\x25\x2d\x35\x3d]', text):
        p = m.start()
        t = tva + p + 7 + struct.unpack_from('<i', text, p + 3)[0]
        if t in vt_by_rva:
            leas.setdefault(t, []).append(tva + p)
    ctor_of_fn = {}
    for v in vts:
        sites = leas.get(v['rva'], [])
        if sorted(sites) != sorted(v['evidence_leas']):
            raise SystemExit('lea sites of %s: %s, recertification says %s' % (v['name'], [hex(x) for x in sites], [hex(x) for x in v['evidence_leas']]))
        v['lea_sites'] = sites
        for site in sites:
            fn = pe.func_of(site)
            if fn is None:
                raise SystemExit('lea %#x outside .pdata' % site)
            if fn[1] - fn[0] <= CTOR_MAX:
                r0, r1 = fn
            else:
                ins = disasm(exe, fn[0], fn[1] - fn[0])
                inside = [(at, b) for at, b, _ in ins if at + len(b) > site - CTOR_WINDOW and at < site + CTOR_WINDOW]
                r0, r1 = inside[0][0], inside[-1][0] + len(inside[-1][1])
            key = (r0, r1)
            span = next((a for a in anchors if a['kind'] == 'span' and (a['rva'], a['rva'] + a['len']) == key), None)
            if span is not None:
                continue  # a span already is that function (bag_item_constructor): it locates the vtable
            if key not in ctor_of_fn:
                a = add_anchor(dict(name='ctor:%s' % v['name'], kind='constructor', section='.text', rva=r0,
                                    offset=pe.r2o(r0), len=r1 - r0, raw_sha256=None, site=site))
                ctor_of_fn[key] = a
            else:
                ctor_of_fn[key]['name'] += ' + %s' % v['name']
    # Two lea sites of one vtable in two functions: the names say which is which.
    seen = {}
    for a in anchors:
        if a['kind'] == 'constructor':
            seen[a['name']] = seen.get(a['name'], 0) + 1
            if seen[a['name']] > 1:
                a['name'] += ' #%d' % seen[a['name']]

    # The context getter that reads the TLS index (study, section 3: checked against the header).
    tls_getter_rva = int(re.search(r'context getter at (0x[0-9a-f]+)', recert['image'][0]['evidence']).group(1), 16)

    # Fields of every anchor; the TLS getter is added when an anchor's field reaches it.
    def anchor_fields(a):
        if a['name'] == 'bag:item_factory_table_bag_entry':
            v = struct.unpack('<I', pe.read(a['rva'], 4))[0]
            return [dict(at=0, kind='jumptable', target=v, tail=0, width=None, mnemonic=None)]
        if a['section'] != '.text':
            return []
        return fields(pe, a['rva'], a['len'])

    for a in list(anchors):
        a['fields'] = anchor_fields(a)
    if tls_getter_rva not in by_rva:
        n = leaf_len(pe, tls_getter_rva) if pe.func_at(tls_getter_rva) is None else pe.func_at(tls_getter_rva)[1] - tls_getter_rva
        g = add_anchor(dict(name='tls:context_getter', kind='tls_getter', section='.text', rva=tls_getter_rva,
                            offset=pe.r2o(tls_getter_rva), len=n, raw_sha256=None))
        g['fields'] = anchor_fields(g)
    for a in anchors:
        if a['kind'] != 'span':
            a['raw_sha256'] = sha(pe.read(a['rva'], a['len']))

    # Relocations inside code anchors would be absolute addresses the mask does not cover.
    for a in anchors:
        if a['section'] == '.text' and pe.relocs_in(a['rva'], a['rva'] + a['len']):
            raise SystemExit('a base relocation inside %s' % a['name'])

    # Matches in the certified build and the search window.
    for a in anchors:
        a['mask'] = [x['at'] for x in a['fields']]
        align = 8 if a['section'] == '.rdata' else 1
        a['align'] = align
        hits = masked_matches(pe, a['section'], pe.read(a['rva'], a['len']), a['mask'], align)
        a['certified_matches'] = None if hits is None else len(hits)
        a['certified_hits'] = hits
        if hits is not None and a['rva'] not in hits:
            raise SystemExit('%s does not match itself' % a['name'])
        a['search'] = None
        wins = unmasked_windows(a['len'], a['mask'], K)
        if not wins or hits is None:
            continue
        buf, va = pe.section_bytes(a['section'])
        raw = pe.read(a['rva'], a['len'])
        first = occurrences(buf, raw[wins[0]:wins[0] + K], 1 << 20)
        # The first window that occurs no more often than the anchor itself; failing that, the one
        # that occurs least (a count that reaches the cap only says "at least that many").
        best = None
        for w in wins:
            cap = 1 << 20 if best is None else best[1]
            n = occurrences(buf, raw[w:w + K], cap)
            if best is None or n < best[1]:
                best = (w, n)
            if n <= len(hits):
                break
        a['search'] = dict(k=K, at=best[0], hash=rolling(raw[best[0]:best[0] + K]))
        a['window_occurrences'] = best[1]
        a['first_window_occurrences'] = first

    # A constructor is only a way to the vtable: one that cannot be searched for (no window of
    # K bytes free of fields) is left out, and kept in the evidence.
    dropped = [dict(name=a['name'], rva=hex(a['rva']), len=a['len'], certified_matches=a['certified_matches'],
                    reason='no window of %d bytes without a field' % K)
               for a in anchors if a['kind'] == 'constructor' and not a['search']]
    anchors[:] = [a for a in anchors if not (a['kind'] == 'constructor' and not a['search'])]
    for i, a in enumerate(anchors):
        a['index'] = i
    by_rva = {}
    for a in anchors:
        by_rva.setdefault(a['rva'], a['index'])

    # Classify every field's target.
    callees, callee_at = [], {}
    contents, content_at = [], {}
    starts = sorted((a['rva'], a['rva'] + a['len'], a['index']) for a in anchors)

    def anchor_holding(t, self_index):
        for r0, r1, i in starts:
            if i != self_index and r0 <= t < r1:
                return i
        return None

    def callee_of(t):
        if t in callee_at:
            return callee_at[t]
        fn = pe.func_at(t)
        if fn:
            bound, r0, n, off = 'function', t, fn[1] - t, 0
        else:
            g = pe.func_of(t)
            if g:
                bound, r0, n, off = 'inside', g[0], g[1] - g[0], t - g[0]
            else:
                bound, r0, n, off = 'leaf', t, leaf_len(pe, t), 0
        fl = fields(pe, r0, n)
        c = dict(index=len(callees), rva=t, bound=bound, offset=off, len=n, mask=[x['at'] for x in fl],
                 normalized_sha256=sha(normalized(pe.read(r0, n), [x['at'] for x in fl])))
        callees.append(c)
        callee_at[t] = c['index']
        return c['index']

    def content_of(t, width):
        w = width or 16
        n_, va, vs, ro, rs = pe.sec['.rdata']
        w = min(w, va + vs - t)
        key = (t, w)
        if key in content_at:
            return content_at[key]
        ptrs = [r - t for r in pe.relocs_in(t, t + w)]
        c = dict(index=len(contents), rva=t, len=w, pointers=ptrs,
                 sha256=sha(zero_pointers(pe.read(t, w), 0, ptrs)))
        contents.append(c)
        content_at[key] = c['index']
        return c['index']

    for a in anchors:
        fn = pe.func_of(a['rva']) if a['section'] == '.text' else None
        a['function'] = dict(offset=a['rva'] - fn[0], len=fn[1] - fn[0]) if fn else None
        for x in a['fields']:
            t = x['target']
            sec = pe.secof(t)
            if t in vt_by_rva:
                x['to'] = ('vtable', vt_by_rva[t]['index'])
            elif x['kind'] in ('rip', 'imagerel') and t == 0:
                x['to'] = ('image_base',)
            elif t == tls_index:
                x['to'] = ('tls_index',)
            elif a['rva'] <= t < a['rva'] + a['len']:
                x['to'] = ('internal', t - a['rva'])
            elif anchor_holding(t, a['index']) is not None:
                i = anchor_holding(t, a['index'])
                x['to'] = ('anchor', i, anchors[i]['rva'] - t)
            elif x['kind'] == 'imagerel' and any(b['len'] == 4 and 0 < b['rva'] - t < 0x400 for b in anchors):
                b = next(b for b in anchors if b['len'] == 4 and 0 < b['rva'] - t < 0x400)
                x['to'] = ('anchor', b['index'], b['rva'] - t)
            elif fn and fn[0] <= t < fn[1]:
                x['to'] = ('same_function', t - fn[0])
            elif a['kind'] == 'constructor' and sec in ('.text', '.rdata'):
                # A constructor's other calls and data are not what the readers rely on: masked,
                # and only required to stay in their section.
                x['to'] = ('code',) if sec == '.text' else ('data',)
            elif sec == '.text':
                x['to'] = ('callee', callee_of(t))
            elif sec == '.rdata':
                x['to'] = ('content', content_of(t, x['width']))
            else:
                raise SystemExit('field +%d of %s points into %s (%#x): no rule for it' % (x['at'], a['name'], sec, t))
        a['normalized_sha256'] = sha(normalized(pe.read(a['rva'], a['len']), a['mask']))

    # The TLS getter must name the TLS index.
    tg = next(a for a in anchors if a['rva'] == tls_getter_rva)
    if not any(x['to'] == ('tls_index',) for x in tg['fields']):
        raise SystemExit('the context getter does not read the TLS index')
    # Every vtable has a way to be located: a constructor anchor (or span) whose field names it.
    for v in vts:
        if not any(x['to'] == ('vtable', v['index']) for a in anchors for x in a['fields']):
            raise SystemExit('no anchor locates vtable %s' % v['name'])

    table = dict(dropped_constructors=dropped,certificate=re.search(r'CERTIFICATE: &str = "([^"]+)"', open(os.path.join(os.path.dirname(cert_path), 'certificate.rs')).read()).group(1)
                 if os.path.exists(os.path.join(os.path.dirname(cert_path), 'certificate.rs')) else 'unknown',
                 source_sha256=exe_sha, image_base=IB, tls_index_rva=tls_index, k=K, prime=PRIME, base=BASE,
                 sections=[s[0] for s in pe.secs], anchors=anchors, vtables=vts, callees=callees, contents=contents,
                 tls_getter=tg['index'])

    # Twins (certified_matches > 1 with a window): how many of the certified places pass every
    # field check, all anchors at their certified places. One means the field checks pick it.
    # More than one (a template repeated whole, as the array reserve of Array.h) means no search
    # can place it: it is not searched, and must be derived by the graph (below).
    located = {a['index']: a['rva'] for a in anchors}
    vloc = {v['index']: v['rva'] for v in vts}
    rt = runtime_table(table)
    ambiguous = {}
    for a in anchors:
        if a['search'] and a['certified_matches'] and a['certified_matches'] > 1:
            ok = [h for h in a['certified_hits'] if check_fields(rt, pe, a['index'], h, located, vloc)[0]]
            a['certified_survivors'] = len(ok)
            if len(ok) > 1 and a['rva'] in ok:
                ambiguous[a['index']] = ok
                a['search'] = None
            elif ok and ok != [a['rva']]:
                raise SystemExit('%s: the field checks pick %s' % (a['name'], ok))
        else:
            a['certified_survivors'] = None

    # Every anchor the readers rely on that no search places (no window, every byte a field, or
    # ambiguous) must be reached by the graph from the anchors a search places: a field of a
    # reached anchor that names it, or a slot of a vtable that a lea of a reached anchor names.
    # The resolver checks its signature where it is derived (rule 3). If nothing reaches it, no
    # rule locates it, and the table is not written.
    def referred_by(i):
        return [b['name'] + ' +%d' % x['at'] for b in anchors for x in b['fields'] if x['to'][:2] == ('anchor', i)] + \
               ['vtable %s +%#x' % (v['name'], s['offset']) for v in vts for s in v['slots'] if s['anchor'] == i]

    reached, vreached = graph_closure(table, searched_start(table))
    lost = []
    for a in anchors:
        if a['kind'] == 'constructor' or a['index'] in reached:
            continue
        if a['index'] in ambiguous:
            why = '%d places of the certified build match it and %d of them pass every field check (%s), so no search can place it' % (
                a['certified_matches'], len(ambiguous[a['index']]), ', '.join(hex(h) for h in ambiguous[a['index']]))
        else:
            why = 'it is not searched (certified matches: %s)' % a['certified_matches']
        lost.append('%s: %s, and the graph does not reach it from the anchors a search places (referred to by: %s)' % (
            a['name'], why, ', '.join(referred_by(a['index'])) or 'nothing'))
    lost += ['vtable %s: the graph does not reach it from the anchors a search places' % v['name'] for v in vts if v['index'] not in vreached]
    if lost:
        raise SystemExit('no rule locates %d of the anchors and vtables; the table is not written:\n  %s' % (len(lost), '\n  '.join(lost)))
    table['derived_not_searched'] = [
        dict(name=anchors[i]['name'], rva=hex(anchors[i]['rva']), certified_matches=anchors[i]['certified_matches'],
             certified_survivors=len(ok), passing=[hex(h) for h in ok], referred_by=referred_by(i))
        for i, ok in sorted(ambiguous.items())]
    return table, pe


def searched_start(table):
    """The anchors a search can place: one match when certified, or one that the field checks
    picked among several (twins)."""
    return {a['index'] for a in table['anchors']
            if a['search'] and (a['certified_matches'] == 1 or a['certified_survivors'] == 1)}


def graph_closure(table, start):
    """What the resolver's derivations (rules 3 and 6) reach from the anchors of start: the anchors
    a field of a reached anchor names, the vtables a lea of a reached anchor names, and the
    functions the slots of a reached vtable hold. (anchor indices, vtable indices)."""
    A = table['anchors']
    reached, vreached = set(start), set()
    changed = True
    while changed:
        changed = False
        for i in sorted(reached):
            for x in A[i]['fields']:
                if x['to'][0] == 'anchor' and x['to'][1] not in reached:
                    reached.add(x['to'][1])
                    changed = True
                elif x['to'][0] == 'vtable' and x['to'][1] not in vreached:
                    vreached.add(x['to'][1])
                    changed = True
        for j in sorted(vreached):
            for s in table['vtables'][j]['slots']:
                if s['anchor'] not in reached:
                    reached.add(s['anchor'])
                    changed = True
    return reached, vreached


# ------------------------------------------------------------------------------------------
# The Rust table: emit, and parse back (the resolver reads only this)
# ------------------------------------------------------------------------------------------
KIND_RS = {'rel32': 'Rel32', 'rip': 'Rip', 'imagerel': 'ImageRel', 'jumptable': 'JumpTable'}
ANCHOR_KIND_RS = {'span': 'Span', 'constructor': 'Constructor', 'tls_getter': 'TlsGetter'}
SECTION_RS = {'.text': 'Text', '.rdata': 'Rdata'}
BOUND_RS = {'function': 'Function', 'inside': 'Inside', 'leaf': 'Leaf'}


def target_rs(to):
    k = to[0]
    if k == 'vtable':
        return 'Target::Vtable(%d)' % to[1]
    if k == 'image_base':
        return 'Target::ImageBase'
    if k == 'tls_index':
        return 'Target::TlsIndex'
    if k == 'internal':
        return 'Target::Internal(%d)' % to[1]
    if k == 'anchor':
        return 'Target::Anchor { anchor: %d, add: %d }' % (to[1], to[2])
    if k == 'same_function':
        return 'Target::SameFunction(%d)' % to[1]
    if k == 'callee':
        return 'Target::Callee(%d)' % to[1]
    if k == 'content':
        return 'Target::Content(%d)' % to[1]
    if k == 'code':
        return 'Target::Code'
    if k == 'data':
        return 'Target::Data'
    raise ValueError(to)


def emit_rs(table):
    L = []
    L.append('// Generated by scripts/relocation/gen_relocation.py from the copy of Gw2-64.exe whose SHA-256 is')
    L.append('// SOURCE_SHA256, the certificate of certificate_data.rs and the recertification of 15 October')
    L.append('// 2026. Do not edit by hand: a change here is a recertification. It holds no byte of the game:')
    L.append('// RVAs, lengths, field positions, digests and one 32-bit rolling hash per searched window.')
    L.append('pub const CERTIFICATE: &str = "%s";' % table['certificate'])
    L.append('pub const SOURCE_SHA256: &str = "%s";' % table['source_sha256'])
    L.append('pub const IMAGE_BASE: u64 = %#x;' % table['image_base'])
    L.append('pub const TLS_INDEX_RVA: u32 = %#x;' % table['tls_index_rva'])
    L.append('pub const ROLLING_PRIME: u32 = %d;' % table['prime'])
    L.append('pub const ROLLING_BASE: u32 = %d;' % table['base'])
    L.append('pub const SECTION_NAMES: [&str; %d] = [%s];' % (len(table['sections']), ', '.join('"%s"' % s for s in table['sections'])))
    L.append('pub const TLS_GETTER: usize = %d;' % table['tls_getter'])
    L.append('pub const ANCHORS: [Anchor; %d] = [' % len(table['anchors']))
    for a in table['anchors']:
        s = a['search']
        fl = ', '.join('Field { at: %d, kind: FieldKind::%s, tail: %d, target: %s }' % (x['at'], KIND_RS[x['kind']], x['tail'], target_rs(x['to'])) for x in a['fields'])
        L.append('    Anchor { name: "%s", kind: AnchorKind::%s, section: Section::%s, rva: %#x, len: %d, align: %d, normalized_sha256: "%s", certified_matches: %s, certified_survivors: %s, search: %s, function: %s, fields: &[%s] },' % (
            a['name'], ANCHOR_KIND_RS[a['kind']], SECTION_RS[a['section']], a['rva'], a['len'], a['align'], a['normalized_sha256'],
            'None' if a['certified_matches'] is None else 'Some(%d)' % a['certified_matches'],
            'None' if a['certified_survivors'] is None else 'Some(%d)' % a['certified_survivors'],
            'None' if s is None else 'Some(Search { k: %d, at: %d, hash: %#010x })' % (s['k'], s['at'], s['hash']),
            'None' if a['function'] is None else 'Some(Within { offset: %d, len: %d })' % (a['function']['offset'], a['function']['len']),
            fl))
    L.append('];')
    L.append('pub const VTABLES: [Vtable; %d] = [' % len(table['vtables']))
    for v in table['vtables']:
        sl = ', '.join('VtableSlot { offset: %#x, anchor: %d, added: %s }' % (x['offset'], x['anchor'], 'true' if x['added'] else 'false') for x in v['slots'])
        L.append('    Vtable { name: "%s", rva: %#x, slots: &[%s] },' % (v['name'], v['rva'], sl))
    L.append('];')
    L.append('pub const CALLEES: [Callee; %d] = [' % len(table['callees']))
    for c in table['callees']:
        L.append('    Callee { rva: %#x, bound: Bound::%s, offset: %d, len: %d, normalized_sha256: "%s", mask: &[%s] },' % (
            c['rva'], BOUND_RS[c['bound']], c['offset'], c['len'], c['normalized_sha256'], ', '.join(str(p) for p in c['mask'])))
    L.append('];')
    L.append('pub const CONTENTS: [Content; %d] = [' % len(table['contents']))
    for c in table['contents']:
        L.append('    Content { rva: %#x, len: %d, sha256: "%s", pointers: &[%s] },' % (c['rva'], c['len'], c['sha256'], ', '.join(str(p) for p in c['pointers'])))
    L.append('];')
    return '\n'.join(L) + '\n'


class RsParser:
    """A parser of the subset of Rust the emitter writes: consts of literals, structs, enums,
    slices, Some/None. Enough to read the table back without trusting anything but the file."""
    TOK = re.compile(r'\s*(?:(//[^\n]*)|("(?:[^"\\]|\\.)*")|(0x[0-9a-fA-F_]+|-?\d+)|([A-Za-z_][A-Za-z_0-9]*(?:::[A-Za-z_][A-Za-z_0-9]*)*)|(&\[|[\[\]{}(),:;=&]))')

    def __init__(self, text):
        self.toks = []
        pos = 0
        while pos < len(text):
            m = self.TOK.match(text, pos)
            if not m or m.end() == pos:
                if text[pos:].strip() == '':
                    break
                raise ValueError('cannot parse at %d: %r' % (pos, text[pos:pos + 40]))
            pos = m.end()
            if m.group(1):
                continue
            if m.group(2):
                self.toks.append(('str', json.loads(m.group(2))))
            elif m.group(3):
                self.toks.append(('int', int(m.group(3).replace('_', ''), 0)))
            elif m.group(4):
                self.toks.append(('id', m.group(4)))
            elif m.group(5):
                self.toks.append(('p', m.group(5)))
        self.i = 0

    def peek(self):
        return self.toks[self.i] if self.i < len(self.toks) else (None, None)

    def take(self, kind=None, val=None):
        t = self.toks[self.i]
        if (kind and t[0] != kind) or (val and t[1] != val):
            raise ValueError('expected %s %s, got %s' % (kind, val, t))
        self.i += 1
        return t[1]

    def consts(self):
        out = {}
        while self.i < len(self.toks):
            self.take('id', 'pub')
            self.take('id', 'const')
            name = self.take('id')
            self.take('p', ':')
            self.typ()
            self.take('p', '=')
            out[name] = self.value()
            self.take('p', ';')
        return out

    def typ(self):
        depth = 0
        while True:
            k, v = self.peek()
            if k == 'p' and v == '=' and depth == 0:
                return
            if k == 'p' and v in ('[', '&['):
                depth += 1
            if k == 'p' and v == ']':
                depth -= 1
            self.i += 1

    def value(self):
        k, v = self.peek()
        if k in ('str', 'int'):
            self.i += 1
            return v
        if k == 'p' and v in ('[', '&['):
            self.i += 1
            out = []
            while self.peek() != ('p', ']'):
                out.append(self.value())
                if self.peek() == ('p', ','):
                    self.i += 1
            self.i += 1
            return out
        if k == 'id':
            self.i += 1
            if v in ('true', 'false'):
                return v == 'true'
            if v == 'None':
                return None
            if self.peek() == ('p', '{'):
                self.i += 1
                d = {'_': v}
                while self.peek() != ('p', '}'):
                    f = self.take('id')
                    self.take('p', ':')
                    d[f] = self.value()
                    if self.peek() == ('p', ','):
                        self.i += 1
                self.i += 1
                return d
            if self.peek() == ('p', '('):
                self.i += 1
                args = []
                while self.peek() != ('p', ')'):
                    args.append(self.value())
                    if self.peek() == ('p', ','):
                        self.i += 1
                self.i += 1
                return args[0] if v == 'Some' else {'_': v, 'args': args}
            return {'_': v}
        raise ValueError('unexpected %s %s' % (k, v))


def target_from_rs(t):
    n = t['_'].split('::')[-1]
    if n == 'Vtable':
        return ('vtable', t['args'][0])
    if n == 'ImageBase':
        return ('image_base',)
    if n == 'TlsIndex':
        return ('tls_index',)
    if n == 'Internal':
        return ('internal', t['args'][0])
    if n == 'Anchor':
        return ('anchor', t['anchor'], t['add'])
    if n == 'SameFunction':
        return ('same_function', t['args'][0])
    if n == 'Callee':
        return ('callee', t['args'][0])
    if n == 'Content':
        return ('content', t['args'][0])
    if n == 'Code':
        return ('code',)
    if n == 'Data':
        return ('data',)
    raise ValueError(t)


def load_table(path):
    """The table as the resolver uses it, read from the Rust file and nothing else."""
    c = RsParser(open(path).read()).consts()
    inv = lambda d: {v: k for k, v in d.items()}
    anchors = []
    for i, a in enumerate(c['ANCHORS']):
        s = a['search']
        anchors.append(dict(index=i, name=a['name'], kind=inv(ANCHOR_KIND_RS)[a['kind']['_'].split('::')[-1]],
                            section=inv(SECTION_RS)[a['section']['_'].split('::')[-1]], rva=a['rva'], len=a['len'], align=a['align'],
                            normalized_sha256=a['normalized_sha256'], certified_matches=a['certified_matches'],
                            certified_survivors=a['certified_survivors'],
                            search=None if s is None else dict(k=s['k'], at=s['at'], hash=s['hash']),
                            function=None if a['function'] is None else dict(offset=a['function']['offset'], len=a['function']['len']),
                            fields=[dict(at=x['at'], kind=inv(KIND_RS)[x['kind']['_'].split('::')[-1]], tail=x['tail'], to=target_from_rs(x['target'])) for x in a['fields']]))
        anchors[-1]['mask'] = [x['at'] for x in anchors[-1]['fields']]
    vts = [dict(index=i, name=v['name'], rva=v['rva'], slots=[dict(offset=x['offset'], anchor=x['anchor'], added=x['added']) for x in v['slots']]) for i, v in enumerate(c['VTABLES'])]
    callees = [dict(index=i, rva=x['rva'], bound=inv(BOUND_RS)[x['bound']['_'].split('::')[-1]], offset=x['offset'], len=x['len'],
                    normalized_sha256=x['normalized_sha256'], mask=x['mask']) for i, x in enumerate(c['CALLEES'])]
    contents = [dict(index=i, rva=x['rva'], len=x['len'], sha256=x['sha256'], pointers=x['pointers']) for i, x in enumerate(c['CONTENTS'])]
    return dict(certificate=c['CERTIFICATE'], source_sha256=c['SOURCE_SHA256'], image_base=c['IMAGE_BASE'], tls_index_rva=c['TLS_INDEX_RVA'],
                prime=c['ROLLING_PRIME'], base=c['ROLLING_BASE'], sections=c['SECTION_NAMES'], tls_getter=c['TLS_GETTER'],
                anchors=anchors, vtables=vts, callees=callees, contents=contents)


def runtime_table(table):
    """The part of a generated table the resolver may use: what the Rust file holds."""
    keep = ('index', 'name', 'kind', 'section', 'rva', 'len', 'align', 'normalized_sha256', 'certified_matches',
            'certified_survivors', 'search', 'function', 'mask')
    anchors = []
    for a in table['anchors']:
        d = {k: a.get(k) for k in keep}
        d['fields'] = [dict(at=x['at'], kind=x['kind'], tail=x['tail'], to=tuple(x['to'])) for x in a['fields']]
        anchors.append(d)
    return dict(certificate=table['certificate'], source_sha256=table['source_sha256'], image_base=table['image_base'],
                tls_index_rva=table['tls_index_rva'], prime=table['prime'], base=table['base'], sections=table['sections'],
                tls_getter=table['tls_getter'], anchors=anchors,
                vtables=[dict(index=v['index'], name=v['name'], rva=v['rva'], slots=[dict(s) for s in v['slots']]) for v in table['vtables']],
                callees=[dict(c) for c in table['callees']], contents=[dict(c) for c in table['contents']])


# ------------------------------------------------------------------------------------------
# Resolver (reads only the table and the other file)
# ------------------------------------------------------------------------------------------
class Fail(Exception):
    def __init__(self, anchor, rule, detail):
        super().__init__('%s: rule %s: %s' % (anchor, rule, detail))
        self.anchor, self.rule, self.detail = anchor, rule, detail


RULES = {
    '1': 'identity first',
    '2': 'search only locates what was unique when certified',
    '3': 'a derived anchor matches its normalized signature where it is derived',
    '4': 'every masked field points where it pointed',
    '5': 'redundant paths agree',
    '6': 'vtable = the constructor lea; every slot holds its located function',
    '7': 'field offsets and constants are never masked',
    '8': 'header',
    '9': 'all or nothing',
}


def field_target(pe, base, x):
    """Where field x of an anchor placed at base points in the file pe, as an RVA."""
    p = base + x['at']
    v = pe.read(p, 4)
    if v is None:
        return None
    if x['kind'] in ('rel32', 'rip'):
        return (p + 4 + x['tail'] + struct.unpack('<i', v)[0]) & 0xffffffff
    return struct.unpack('<I', v)[0]


def digest_at(pe, a, at):
    raw = pe.read(at, a['len'])
    if raw is None or pe.secof(at) != a['section'] or at % a['align']:
        return False
    return sha(normalized(raw, a['mask'])) == a['normalized_sha256']


def callee_ok(rt, pe, c, t):
    """Whether the function at t has the callee's fingerprint: bound, length, normalized digest."""
    if c['bound'] == 'function':
        fn = pe.func_at(t)
        if fn is None or fn[1] - fn[0] != c['len']:
            return False
        r0 = t
    elif c['bound'] == 'inside':
        fn = pe.func_of(t)
        if fn is None or t - fn[0] != c['offset'] or fn[1] - fn[0] != c['len']:
            return False
        r0 = fn[0]
    else:
        if pe.func_of(t) is not None:
            return False
        r0 = t
    raw = pe.read(r0, c['len'])
    return raw is not None and pe.secof(r0) == '.text' and sha(normalized(raw, c['mask'])) == c['normalized_sha256']


def content_ok(pe, c, t):
    raw = pe.read(t, c['len'])
    if raw is None or pe.secof(t) != '.rdata':
        return False
    if [r - t for r in pe.relocs_in(t, t + c['len'])] != c['pointers']:
        return False
    return sha(zero_pointers(raw, 0, c['pointers'])) == c['sha256']


def vtable_slots_ok(rt, pe, j, at, located, only_located=False):
    """Whether the vtable j, placed at at, holds each checked slot: the located function, or a
    place where the function's normalized signature matches. With only_located, the slots of
    functions not located yet are left to their own derivation (which names them if they fail)."""
    for s in rt['vtables'][j]['slots']:
        q = pe.u64(at + s['offset'])
        if q is None:
            return False, 'slot +%#x unreadable' % s['offset']
        f = q - rt['image_base']
        fa = rt['anchors'][s['anchor']]
        if s['anchor'] in located:
            if f != located[s['anchor']]:
                return False, 'slot +%#x holds %#x, %s is at %#x' % (s['offset'], f, fa['name'], located[s['anchor']])
        elif only_located:
            continue
        elif not digest_at(pe, fa, f):
            return False, 'slot +%#x holds %#x, where %s does not match' % (s['offset'], f, fa['name'])
    return True, None


def check_fields(rt, pe, i, base, located, vloc):
    """Rule 4 for anchor i placed at base: every masked field points at its target's new place.
    Targets not located yet are checked by their own signature there. (ok, reason)."""
    a = rt['anchors'][i]
    fn = None
    if a['function'] is not None:
        fn = pe.func_of(base)
        if fn is None or base - fn[0] != a['function']['offset'] or fn[1] - fn[0] != a['function']['len']:
            return False, 'rule 4: its function is not the certified one (offset %s, length %s)' % (
                None if fn is None else base - fn[0], None if fn is None else fn[1] - fn[0])
    for x in a['fields']:
        t = field_target(pe, base, x)
        to = x['to']
        k = to[0]
        where = 'field +%d (%s)' % (x['at'], x['kind'])
        if t is None:
            return False, '%s unreadable' % where
        if k == 'image_base':
            if t != 0:
                return False, '%s points at %#x, not the image base' % (where, t)
        elif k == 'tls_index':
            if t != pe.tls_index_rva():
                return False, '%s points at %#x, the TLS directory names %#x' % (where, t, pe.tls_index_rva() or 0)
        elif k == 'internal':
            if t - base != to[1]:
                return False, '%s points %d bytes into it, certified %d' % (where, t - base, to[1])
        elif k == 'same_function':
            if fn is None or t - fn[0] != to[1]:
                return False, '%s points %s bytes into its function, certified %d' % (where, None if fn is None else t - fn[0], to[1])
        elif k == 'anchor':
            j, add = to[1], to[2]
            b = rt['anchors'][j]
            if j in located:
                if t + add != located[j]:
                    return False, '%s names %s at %#x, it is at %#x' % (where, b['name'], t + add, located[j])
            elif not digest_at(pe, b, t + add):
                return False, '%s names %s at %#x, where its signature does not match' % (where, b['name'], t + add)
        elif k == 'vtable':
            j = to[1]
            if j in vloc:
                if t != vloc[j]:
                    return False, '%s names vtable %s at %#x, it is at %#x' % (where, rt['vtables'][j]['name'], t, vloc[j])
            else:
                ok, why = vtable_slots_ok(rt, pe, j, t, located)
                if not ok:
                    return False, '%s names vtable %s at %#x: %s' % (where, rt['vtables'][j]['name'], t, why)
        elif k == 'callee':
            c = rt['callees'][to[1]]
            if not callee_ok(rt, pe, c, t):
                return False, '%s calls %#x, which is not the certified callee (%s, %d bytes)' % (where, t, c['bound'], c['len'])
        elif k == 'content':
            c = rt['contents'][to[1]]
            if not content_ok(pe, c, t):
                return False, '%s reads %#x, whose %d bytes are not the certified ones' % (where, t, c['len'])
        elif k == 'code':
            if pe.secof(t) != '.text':
                return False, '%s points at %#x, outside .text' % (where, t)
        elif k == 'data':
            if pe.secof(t) != '.rdata':
                return False, '%s points at %#x, outside .rdata' % (where, t)
        else:
            return False, 'unknown target %s' % (to,)
    return True, None


def search(rt, pe, stats=None):
    """Rolling-hash search of every anchor with a window, in its section. Returns
    {anchor index: [confirmed places]} and fills stats with the cost."""
    found = {a['index']: [] for a in rt['anchors'] if a['search']}
    false = {a['index']: 0 for a in rt['anchors'] if a['search']}
    t0 = time.perf_counter()
    scanned = 0
    raw_hits = 0
    for section in ('.text', '.rdata'):
        group = [a for a in rt['anchors'] if a['search'] and a['section'] == section]
        if not group:
            continue
        by_hash = {}
        for a in group:
            by_hash.setdefault(a['search']['hash'], []).append(a)
        k = group[0]['search']['k']
        assert all(a['search']['k'] == k for a in group)
        buf, va = pe.section_bytes(section)
        scanned += len(buf)
        hits = rolling_scan(buf, k, by_hash)
        raw_hits += len(hits)
        for i, h in hits:
            for a in by_hash[h]:
                at = va + i - a['search']['at']
                if digest_at(pe, a, at):
                    found[a['index']].append(at)
                else:
                    false[a['index']] += 1
    if stats is not None:
        stats.update(seconds=time.perf_counter() - t0, bytes=scanned, hash_hits=raw_hits,
                     false_candidates={rt['anchors'][i]['name']: n for i, n in false.items()},
                     confirmed={rt['anchors'][i]['name']: len(v) for i, v in found.items()})
    return found


def header_check(rt, pe):
    if pe.machine != 0x8664 or pe.magic != 0x20b:
        raise Fail('header', '8', 'not AMD64 PE32+')
    if pe.image_base != rt['image_base']:
        raise Fail('header', '8', 'image base %#x' % pe.image_base)
    if [s[0] for s in pe.secs] != rt['sections']:
        raise Fail('header', '8', 'sections %s' % [s[0] for s in pe.secs])
    if pe.tls_index_rva() is None:
        raise Fail('header', '8', 'no TLS directory')


def identity(cert, pe):
    """Rule 1: the certificate of certificate_data.rs at its fixed offsets."""
    if pe.image_size != cert['image_size'] or pe.dirs[9][0] != cert['tls_directory_rva']:
        return False
    for s, c in zip(pe.secs, cert['sections']):
        if (s[0], s[1], s[2], s[3]) != (c['name'], c['rva'], c['virtual_size'], c['file_offset']):
            return False
    if len(pe.secs) != len(cert['sections']):
        return False
    for s in cert['spans']:
        if sha(pe.f[s['offset']:s['offset'] + s['len']]) != s['sha256']:
            return False
    for s in cert['slots']:
        if s['offset'] + 8 > len(pe.f) or struct.unpack_from('<Q', pe.f, s['offset'])[0] != IB + s['rva']:
            return False
    tls = pe.tls_index_rva()
    return tls is not None


def search_accepts(a, places):
    """Rule 2: a search places an anchor only if it matched once when certified and once now."""
    return a['certified_matches'] == 1 and len(places) == 1


def resolve(rt, pe, cert=None, force_search=False, stats=None):
    """Every anchor, vtable and the TLS index in pe, or Fail with the first rule that does not
    hold. Returns dict(how=..., anchors={i: rva}, vtables={j: rva}, ...)."""
    header_check(rt, pe)
    A = rt['anchors']
    if cert is not None and not force_search and identity(cert, pe):
        # Rule 1. The certificate holds at fixed offsets: every anchor is where it was certified.
        # The constructor and TLS anchors are not in the certificate: they must still match there.
        located = {a['index']: a['rva'] for a in A}
        vloc = {v['index']: v['rva'] for v in rt['vtables']}
        for a in A:
            if not digest_at(pe, a, a['rva']):
                raise Fail(a['name'], '1', 'the certificate holds but this anchor does not match at its RVA')
        return finish(rt, pe, located, vloc, {a['index']: 'identity' for a in A}, 'identity')
    found = search(rt, pe, stats)
    located, vloc, how = {}, {}, {}
    # Rule 2: by search, only what had one match when certified and has one now.
    for a in A:
        if a['search'] and search_accepts(a, found[a['index']]):
            located[a['index']] = found[a['index']][0]
            how[a['index']] = 'search (1 match)'
    changed = True
    while changed:
        changed = False
        # Rule 3: derivation from the fields of located anchors.
        for i in sorted(located):
            a = A[i]
            for x in a['fields']:
                to = x['to']
                t = field_target(pe, located[i], x)
                if to[0] == 'anchor':
                    j, add = to[1], to[2]
                    if j in located:
                        continue
                    if not digest_at(pe, A[j], t + add):
                        raise Fail(A[j].get('name'), '3', 'derived at %#x by field +%d of %s, its signature does not match there' % (t + add, x['at'], a['name']))
                    located[j] = t + add
                    how[j] = 'field +%d of %s' % (x['at'], a['name'])
                    changed = True
                elif to[0] == 'vtable':
                    j = to[1]
                    if j in vloc:
                        continue
                    ok, why = vtable_slots_ok(rt, pe, j, t, located, only_located=True)
                    if not ok:
                        raise Fail('vtable ' + rt['vtables'][j]['name'], '6', 'lea in %s names %#x: %s' % (a['name'], t, why))
                    vloc[j] = t
                    how['vt%d' % j] = 'lea in %s' % a['name']
                    changed = True
        # Rule 6 and 3: functions named by the slots of located vtables.
        for j in sorted(vloc):
            for s in rt['vtables'][j]['slots']:
                if s['anchor'] in located:
                    continue
                q = pe.u64(vloc[j] + s['offset'])
                f = None if q is None else q - rt['image_base']
                b = A[s['anchor']]
                if f is None or not digest_at(pe, b, f):
                    raise Fail(b['name'], '3', 'slot +%#x of vtable %s holds %s, where its signature does not match' % (s['offset'], rt['vtables'][j]['name'], None if f is None else hex(f)))
                located[s['anchor']] = f
                how[s['anchor']] = 'slot +%#x of vtable %s' % (s['offset'], rt['vtables'][j]['name'])
                changed = True
        if changed:
            continue  # the graph first: twins only once nothing more derives
        # Twins: an anchor that matched several places when certified, of which the field
        # checks picked one, is taken from the search when they pick one now.
        for a in A:
            i = a['index']
            if i in located or not a['search'] or a['certified_survivors'] != 1:
                continue
            ok = [h for h in found[i] if check_fields(rt, pe, i, h, located, vloc)[0]]
            if len(ok) == 1:
                located[i] = ok[0]
                how[i] = 'search (%d matches), the field checks pick one' % len(found[i])
                changed = True
            elif not ok:
                raise Fail(a['name'], '4', '%d matches, none passes the field checks' % len(found[i]))
    # Rule 9: all or nothing, over what the readers rely on. A constructor is only a way to its
    # vtable: one that is not found is no failure, as long as its vtable is located.
    for a in A:
        if a['index'] not in located and a['kind'] == 'constructor':
            how[a['index']] = 'not found (%d matches); optional' % len(found[a['index']])
            continue
        if a['index'] not in located:
            n = len(found.get(a['index'], [])) if a['search'] else None
            if a['search'] and a['certified_matches'] == 1 and n and n > 1:
                passing = [h for h in found[a['index']] if check_fields(rt, pe, a['index'], h, located, vloc)[0]]
                raise Fail(a['name'], '2', 'certified with 1 match, %d now (%d pass every field check): ambiguous, and nothing derives it' % (n, len(passing)))
            raise Fail(a['name'], '9', 'not located (search: %s, certified matches %s; no derivation reached it)' % (
                'not searched' if n is None else '%d matches' % n, a['certified_matches']))
    for v in rt['vtables']:
        if v['index'] not in vloc:
            raise Fail('vtable ' + v['name'], '9', 'not located')
    return finish(rt, pe, located, vloc, how, 'relocated')


def finish(rt, pe, located, vloc, how, mode):
    A = rt['anchors']
    # Rule 4 over everything, with every place known.
    for a in A:
        if a['index'] not in located:
            continue
        ok, why = check_fields(rt, pe, a['index'], located[a['index']], located, vloc)
        if not ok:
            raise Fail(a['name'], '4', why)
    # Rule 6: every slot of every vtable.
    for v in rt['vtables']:
        ok, why = vtable_slots_ok(rt, pe, v['index'], vloc[v['index']], located)
        if not ok:
            raise Fail('vtable ' + v['name'], '6', why)
    # Rule 5: a searched anchor located some other way must be its one match.
    tls = pe.tls_index_rva()
    n, va, vs, ro, rs = pe.sec['.rdata']
    return dict(mode=mode, image_size=pe.image_size, tls_index_rva=tls, rdata=(va, va + vs),
                anchors={i: located[i] for i in sorted(located)}, vtables={j: vloc[j] for j in sorted(vloc)},
                how={k: v for k, v in how.items()})


def report(rt, res):
    out = dict(mode=res['mode'], image_size=hex(res['image_size']), tls_index_rva=hex(res['tls_index_rva']),
               rdata=[hex(res['rdata'][0]), hex(res['rdata'][1])], anchors=[], vtables=[])
    for a in rt['anchors']:
        r = res['anchors'].get(a['index'])
        out['anchors'].append(dict(name=a['name'], kind=a['kind'], certified=hex(a['rva']), rva=None if r is None else hex(r),
                                   delta=None if r is None else hex(r - a['rva']), how=res['how'].get(a['index'])))
    for v in rt['vtables']:
        r = res['vtables'][v['index']]
        out['vtables'].append(dict(name=v['name'], certified=hex(v['rva']), rva=hex(r), delta=hex(r - v['rva']), how=res['how'].get('vt%d' % v['index'])))
    return out


# ------------------------------------------------------------------------------------------
# Evidence
# ------------------------------------------------------------------------------------------
def evidence(table, inputs):
    A = table['anchors']
    return dict(
        generated_by='scripts/relocation/gen_relocation.py',
        inputs=inputs,
        parameters=dict(k=K, prime=PRIME, base=BASE, ctor_max=CTOR_MAX, ctor_window=CTOR_WINDOW),
        counts=dict(anchors=len(A), spans=sum(a['kind'] == 'span' for a in A), constructors=sum(a['kind'] == 'constructor' for a in A),
                    tls_getters=sum(a['kind'] == 'tls_getter' for a in A), searched=sum(1 for a in A if a['search']),
                    unique_searched=sum(1 for a in A if a['search'] and a['certified_matches'] == 1),
                    vtables=len(table['vtables']), slots=sum(len(v['slots']) for v in table['vtables']),
                    added_slots=sum(s['added'] for v in table['vtables'] for s in v['slots']),
                    callees=len(table['callees']), contents=len(table['contents']),
                    fields=sum(len(a['fields']) for a in A),
                    edges=dict(field_to_anchor=sum(1 for a in A for x in a['fields'] if x['to'][0] == 'anchor'),
                               constructor_lea=sum(1 for a in A for x in a['fields'] if x['to'][0] == 'vtable'),
                               vtable_slot=sum(len(v['slots']) for v in table['vtables']))),
        anchors=[dict(name=a['name'], kind=a['kind'], section=a['section'], rva=hex(a['rva']), len=a['len'], fields=len(a['fields']),
                      masked_bytes=4 * len(a['fields']), certified_matches=a['certified_matches'],
                      certified_survivors=a['certified_survivors'],
                      search=None if not a['search'] else dict(at=a['search']['at'], window_occurrences=a['window_occurrences'],
                                                               first_window_occurrences=a['first_window_occurrences']),
                      targets=sorted({x['to'][0] for x in a['fields']})) for a in A],
        vtables=[dict(name=v['name'], rva=hex(v['rva']), lea_sites=[hex(s) for s in v['lea_sites']],
                      slots=[dict(offset=hex(s['offset']), anchor=A[s['anchor']]['name'], added=s['added']) for s in v['slots']]) for v in table['vtables']],
        callees=[dict(rva=hex(c['rva']), bound=c['bound'], offset=c['offset'], len=c['len'], masked=len(c['mask'])) for c in table['callees']],
        contents=[dict(rva=hex(c['rva']), len=c['len'], pointers=c['pointers']) for c in table['contents']],
        dropped_constructors=table['dropped_constructors'],
        derived_not_searched=table['derived_not_searched'],
    )


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('exe', nargs='?')
    ap.add_argument('recert', nargs='?')
    ap.add_argument('certificate_data', nargs='?')
    ap.add_argument('-o', '--out', default=DEFAULT_TABLE)
    ap.add_argument('--evidence', default=DEFAULT_EVIDENCE)
    ap.add_argument('--resolve', metavar='EXE')
    ap.add_argument('--table', default=DEFAULT_TABLE)
    ap.add_argument('--certificate', default=DEFAULT_CERT)
    ap.add_argument('--force-search', action='store_true', help='skip the identity path (rule 1)')
    ap.add_argument('--json', help='write the resolution here')
    args = ap.parse_args()
    if args.resolve:
        rt = load_table(args.table)
        cert = parse_certificate(open(args.certificate).read())
        pe = PE(args.resolve)
        stats = {}
        t0 = time.perf_counter()
        try:
            res = resolve(rt, pe, cert, args.force_search, stats)
        except Fail as e:
            print('not resolved: %s: rule %s (%s): %s' % (e.anchor, e.rule, RULES.get(e.rule, ''), e.detail))
            if args.json:
                json.dump(dict(failed=dict(anchor=e.anchor, rule=e.rule, detail=e.detail)), open(args.json, 'w'), indent=1)
            return 1
        out = report(rt, res)
        if stats:
            out['search'] = dict(seconds=round(stats['seconds'], 2), bytes=stats['bytes'], hash_hits=stats['hash_hits'],
                                 false_candidates=sum(stats['false_candidates'].values()))
        out['seconds'] = round(time.perf_counter() - t0, 2)
        for x in out['anchors']:
            print('%-70s %-10s -> %-10s %-8s %s' % (x['name'][:70], x['certified'], x['rva'], x['delta'], x['how']))
        for x in out['vtables']:
            print('vtable %-63s %-10s -> %-10s %-8s %s' % (x['name'][:63], x['certified'], x['rva'], x['delta'], x['how']))
        print('resolved (%s): %d anchors, %d vtables, image size %s, TLS index %s, .rdata %s..%s%s' % (
            out['mode'], len(out['anchors']), len(out['vtables']), out['image_size'], out['tls_index_rva'], out['rdata'][0], out['rdata'][1],
            '' if 'search' not in out else ', search %.2f s over %d bytes, %d hash hits, %d false candidates' % (
                out['search']['seconds'], out['search']['bytes'], out['search']['hash_hits'], out['search']['false_candidates'])))
        if args.json:
            json.dump(out, open(args.json, 'w'), indent=1)
        return 0
    if not (args.exe and args.recert and args.certificate_data):
        ap.error('generate needs <exe> <recertification json> <certificate_data.rs>')
    table, pe = generate(args.exe, args.recert, args.certificate_data)
    text = emit_rs(table)
    with open(args.out, 'w') as fh:
        fh.write(text)
    # Round trip: the resolver reads the Rust file; it must say what the generator meant.
    back = load_table(args.out)
    want = runtime_table(table)
    for k in want:
        if json.dumps(want[k], sort_keys=True, default=list) != json.dumps(back[k], sort_keys=True, default=list):
            raise SystemExit('round trip of %s differs' % k)
    # Self check: resolving the certified copy by search alone gives every certified RVA.
    stats = {}
    res = resolve(back, pe, None, True, stats)
    bad = [a['name'] for a in back['anchors'] if res['anchors'].get(a['index']) != a['rva']] + \
          [v['name'] for v in back['vtables'] if res['vtables'][v['index']] != v['rva']]
    if bad:
        raise SystemExit('the certified copy does not resolve to itself: %s' % bad)
    inputs = dict(exe_sha256=table['source_sha256'], recertification=os.path.basename(args.recert),
                  recertification_sha256=sha(open(args.recert, 'rb').read()),
                  certificate_data_sha256=sha(open(args.certificate_data, 'rb').read()),
                  table_sha256=sha(text.encode()))
    ev = evidence(table, inputs)
    ev['self_check'] = dict(mode=res['mode'], anchors_at_certified_rva=len(back['anchors']), vtables_at_certified_rva=len(back['vtables']),
                            how={back['anchors'][i]['name'] if isinstance(i, int) else back['vtables'][int(i[2:])]['name']: h for i, h in res['how'].items()},
                            false_candidates=stats['false_candidates'], hash_hits=stats['hash_hits'], bytes_scanned=stats['bytes'])
    with open(args.evidence, 'w') as fh:
        json.dump(ev, fh, indent=1)
        fh.write('\n')
    print('wrote %s (%d anchors, %d vtables, %d callees, %d contents) and %s; certified copy resolves to itself by search in %.2f s' % (
        args.out, len(table['anchors']), len(table['vtables']), len(table['callees']), len(table['contents']), args.evidence, stats['seconds']))
    return 0


if __name__ == '__main__':
    sys.exit(main())
