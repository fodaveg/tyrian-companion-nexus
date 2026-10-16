import json,struct,sys,re,hashlib
exe,receipt,funcs=sys.argv[1:4]
f=open(exe,'rb').read(); d=json.load(open(receipt)); fn=json.load(open(funcs))
IB=0x140000000
pe=struct.unpack_from('<I',f,0x3c)[0]; ns=struct.unpack_from('<H',f,pe+6)[0]; osz=struct.unpack_from('<H',f,pe+20)[0]
secs=[]
for i in range(ns):
    o=pe+24+osz+40*i; n=f[o:o+8]; vs,va,rs,ro=struct.unpack_from('<IIII',f,o+8); secs.append((n,va,vs,ro))
def r2o(r):
    for n,va,vs,ro in secs:
        if va<=r<va+vs: return r-va+ro
tls_rva=struct.unpack_from('<I',f,pe+24+112+9*8)[0]
L=[]
L.append('// Generated from docs/audit/2026-10-15-recertification-build-208704.json and the 14 functions')
L.append('// measured on 16 October 2026 (see the module docs). Do not edit by hand: a change here is a')
L.append('// recertification.')
L.append('pub const IMAGE_BASE: u64 = %s;' % hex(IB))
L.append('pub const IMAGE_SIZE: u32 = %s;' % d['to']['image_size'])
L.append('pub const TLS_DIRECTORY_RVA: u32 = %s;' % hex(tls_rva))
L.append('pub const TLS_DIRECTORY_OFFSET: u64 = %s;' % hex(r2o(tls_rva)))
L.append('pub const SECTIONS: [Section; %d] = [' % len(secs))
for n,va,vs,ro in secs:
    L.append('    Section { name: *b"%s", rva: %s, virtual_size: %s, file_offset: %s },' % (''.join(chr(c) if c else '\\0' for c in n), hex(va), hex(vs), hex(ro)))
L.append('];')
L.append('pub const SPANS: [Span; %d] = [' % (len(d['guards'])+len(fn)))
for g in d['guards']:
    L.append('    Span { name: "%s", offset: %s, len: %d, sha256: "%s" },' % (' + '.join(g['names']), g['file_offset'], g['size'], g['new_sha256']))
for x in fn:
    assert x['evidence_in_order'] or x['name'] in ('final_stack_quantity_getter','stack_quantity_getter','conditional_predicate_A','conditional_predicate_B','conditional_predicate_C')
    L.append('    Span { name: "item:%s", offset: %s, len: %d, sha256: "%s" },' % (x['name'], x['file_offset'], x['size'], x['sha256']))
L.append('];')
slots=[]
for v in d['vtables']:
    for off,tgt in re.findall(r'\+(0x[0-9a-f]+) -> (0x[0-9a-f]+)',v['evidence']):
        a=int(v['new_rva'],16)+int(off,16)
        q=struct.unpack_from('<Q',f,r2o(a))[0]
        assert q==IB+int(tgt,16), v['name']
        slots.append('    Slot { name: "vtable %s +%s", offset: %s, rva: %s },' % (v['name'], off, hex(r2o(a)), tgt))
L.append('pub const SLOTS: [Slot; %d] = [' % len(slots)); L+=slots; L.append('];')
print('\n'.join(L))
