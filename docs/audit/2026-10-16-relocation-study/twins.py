# twins.py <exe> <recert-13oct.json>: for the spans whose masked signature matches more than once,
# where each match's relocatable fields point; a twin that points elsewhere is rejected by the
# cross-check "every field must point at its located anchor or its fingerprinted target".
import sys, runpy, struct
g = runpy.run_path(sys.argv[0].replace('twins.py', 'normsig.py'), run_name='lib')
fields, masked_search, f, r2o = g['fields'], g['masked_search'], g['f'], g['r2o']
def targets_at(h, rva, fl):
    out = []
    for pos, kind, tgt in fl:
        if kind in ('rel32', 'rip'):
            orig = struct.unpack_from('<i', f, r2o(rva)+pos)[0]; tail = tgt-(rva+pos+4)-orig  # imm bytes after the field
            out.append(hex(h+pos+4+tail+struct.unpack_from('<i', f, r2o(h)+pos)[0]))
        else:
            out.append(hex(struct.unpack_from('<I', f, r2o(h)+pos)[0]))
    return out
for name, rva, size in [('mf:buff_lookup', 0x251e20, 296), ('bag:item_factory_bag_case', 0x13c7a6a, 86), ('bag:bag_item_constructor', 0x13c9730, 44)]:
    fl = fields(rva, size); mask = [False]*size
    for pos, kind, tgt in fl:
        for q in range(4): mask[pos+q] = True
    hits = masked_search(f[r2o(rva):r2o(rva)+size], mask)
    true_t = targets_at(rva, rva, fl)
    print(name, 'hits', len(hits))
    for h in hits:
        t = targets_at(h, rva, fl)
        print('   ', hex(h), 'TRUE' if h == rva else 'twin', 'fields differing from the true one:', sum(a != b for a, b in zip(t, true_t)), 'of', len(t), t if h != rva else '')
