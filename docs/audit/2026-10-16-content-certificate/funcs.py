import json,struct,sys,hashlib,subprocess,re
exe,receipt=sys.argv[1],sys.argv[2]
f=open(exe,'rb').read(); d=json.load(open(receipt))
pe=struct.unpack_from('<I',f,0x3c)[0]; ns=struct.unpack_from('<H',f,pe+6)[0]; osz=struct.unpack_from('<H',f,pe+20)[0]
secs=[]
for i in range(ns):
    o=pe+24+osz+40*i; n=f[o:o+8].rstrip(b'\0').decode(); vs,va,rs,ro=struct.unpack_from('<IIII',f,o+8); secs.append((n,va,vs,ro))
def r2o(r):
    for n,va,vs,ro in secs:
        if va<=r<va+vs: return r-va+ro
pd=[s for s in secs if s[0]=='.pdata'][0]
pdata={}
for i in range(pd[2]//12):
    b,e,u=struct.unpack_from('<III',f,pd[3]+12*i); pdata[b]=e
out=[]
for x in d['compared_by_address_only']:
    rva=int(x['new_rva'],16)
    end=pdata.get(rva)
    src='pdata'
    if end is None:
        # leaf: up to first ret found by disassembly
        src='leaf'
    dis=subprocess.run(['objdump','-d','-M','intel','--no-show-raw-insn','--start-address=%d'%(0x140000000+rva),'--stop-address=%d'%(0x140000000+(end or rva+64)),exe],capture_output=True,text=True).stdout
    ins=[re.sub(r'\s+',' ',l.split('\t',1)[1]).strip().lower() for l in dis.splitlines() if re.match(r'\s+[0-9a-f]+:\t',l)]
    if end is None:
        n=0; 
        for l in dis.splitlines():
            m=re.match(r'\s+([0-9a-f]+):\t(.*)',l)
            if m and m.group(2).strip().startswith('ret'):
                end=int(m.group(1),16)-0x140000000+1; break
    want=[re.sub(r'\s+',' ',w).strip().lower().replace(' ptr ',' ptr ') for w in x['evidence'].split('new file: ')[1].split('; ')]
    norm=lambda s: s.replace(' ','').replace('ptr','')
    seq=[norm(i) for i in ins]; pos=0; ok=True
    for w in want:
        try: pos=seq.index(norm(w),pos)+1
        except ValueError: ok=False; break
    off=r2o(rva); size=end-rva
    out.append(dict(name=x['name'],rva=hex(rva),file_offset=hex(off),size=size,bound=src,evidence_in_order=ok,sha256=hashlib.sha256(f[off:off+size]).hexdigest()))
    print(x['name'],hex(rva),src,size,'evidence-ok' if ok else 'EVIDENCE-MISSING')
json.dump(out,open('funcs.json','w'),indent=1)
