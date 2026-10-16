import json,struct,sys,hashlib,subprocess,re,bisect
# offsets.py <copy of Gw2-64.exe> offsets.json > offsets_measured.json
# Measures the ranges and slots of offsets.json on the copy: bounds from .pdata, the evidence in
# order in objdump's disassembly, the digest of each range and the target of each slot.
exe,spec=sys.argv[1],sys.argv[2]
f=open(exe,'rb').read(); d=json.load(open(spec))
IB=0x140000000
pe=struct.unpack_from('<I',f,0x3c)[0]; ns=struct.unpack_from('<H',f,pe+6)[0]; osz=struct.unpack_from('<H',f,pe+20)[0]
secs=[]
for i in range(ns):
    o=pe+24+osz+40*i; n=f[o:o+8].rstrip(b'\0').decode(); vs,va,rs,ro=struct.unpack_from('<IIII',f,o+8); secs.append((n,va,vs,ro))
def r2o(r):
    for n,va,vs,ro in secs:
        if va<=r<va+vs: return r-va+ro
pd=[s for s in secs if s[0]=='.pdata'][0]
pdata=sorted(struct.unpack_from('<II',f,pd[3]+12*i) for i in range(pd[2]//12))
pdata=[(b,e) for b,e in pdata if b]
starts=[b for b,e in pdata]
def entry(rva):
    i=bisect.bisect_right(starts,rva)-1
    return pdata[i] if i>=0 and pdata[i][0]<=rva<pdata[i][1] else None
def dis(start,end):
    out=subprocess.run(['x86_64-w64-mingw32-objdump','-d','-M','intel','--no-show-raw-insn','--start-address=%d'%(IB+start),'--stop-address=%d'%(IB+end),exe],capture_output=True,text=True).stdout
    res=[]
    for l in out.splitlines():
        m=re.match(r'\s+([0-9a-f]+):\t(.*)',l)
        if m: res.append((int(m.group(1),16)-IB,re.sub(r'\s+',' ',m.group(2)).strip().lower()))
    return res
def bounds(x):
    rva=int(x['rva'],16)
    if x['bound']=='pdata':
        b,e=entry(rva); assert b==rva, x['name']; return rva,e
    end=int(x['end'],16)
    if x['bound']=='pdata-run':
        # One function in contiguous .pdata entries, from its first to the end of its last.
        at=rva
        while at<end:
            b,e=entry(at); assert b==at, (x['name'],hex(at)); at=e
        assert at==end, x['name']; return rva,end
    if x['bound']=='block':
        # Straight code inside one function: both ends are instruction boundaries of it.
        fn=int(x['function'],16)
        assert entry(fn)[0]==fn and fn<=rva, x['name']
        at=[a for a,_ in dis(fn,end+16)]
        assert rva in at and end in at, x['name']; return rva,end
    raise ValueError(x['bound'])
def found(ins,want):
    # Each piece of evidence in order: a memory operand, an immediate operand, or a direct call.
    plain=[re.sub(r'\b(xmm|[dq]?)word ptr |byte ptr ','',s) for _,s in ins]
    pos=0
    for w in want:
        call='call 0x%x'%(IB+int(w[5:],16)) if w.startswith('call ') else None
        while pos<len(plain) and not (plain[pos]==call if call else w in plain[pos]): pos+=1
        if pos==len(plain): return False
        pos+=1
    return True
out=dict(spans=[],slots=[])
for x in d['spans']:
    rva,end=bounds(x)
    ins=dis(rva,end)
    assert ins and ins[0][0]==rva and found(ins,x['evidence']), x['name']
    off=r2o(rva); size=end-rva
    out['spans'].append(dict(name=x['name'],backs=x['backs'],rva=hex(rva),file_offset=hex(off),size=size,bound=x['bound'],sha256=hashlib.sha256(f[off:off+size]).hexdigest()))
    print(x['name'],hex(rva),x['bound'],size,'evidence-ok',file=sys.stderr)
for s in d['slots']:
    a=int(s['rva'],16)+int(s['slot'],16)
    q=struct.unpack_from('<Q',f,r2o(a))[0]
    assert q==IB+int(s['target'],16), s
    out['slots'].append(dict(name='vtable %s +%s'%(s['vtable'],s['slot']),file_offset=hex(r2o(a)),rva=s['target']))
print(json.dumps(out,indent=1))
