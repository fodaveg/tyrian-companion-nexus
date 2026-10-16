# Relocation table (lot 1 of the relocation study)

`gen_relocation.py` makes `core/src/relocation_data.rs` from the certified copy of `Gw2-64.exe`,
and resolves another executable with it: the semiautomatic recertification of
`docs/audit/2026-10-16-relocation-study/README.md`. `test_relocation.py` holds it to the figures
of that study and to its own guarantees. Nothing in the addon uses the table yet (lots 2-4).

It lives here and not in `docs/audit/` because it is a tool that is run again on every update of
the game; the dated directories of `docs/audit/` are receipts of one recertification each.
`evidence.json` is regenerated with the table.

## Use

```
# generate (the copy must hold the certificate in force)
python3 -I scripts/relocation/gen_relocation.py <certified Gw2-64.exe> \
    docs/audit/2026-10-15-recertification-build-208704.json core/src/certificate_data.rs

# resolve another executable: new RVAs, or the first rule that does not hold
python3 -I scripts/relocation/gen_relocation.py --resolve <other Gw2-64.exe> [--force-search] [--json out.json]

# checks (variants are written under --tmp only, outside the repository, and deleted)
python3 -I scripts/relocation/test_relocation.py <certified Gw2-64.exe> --tmp <dir>
```

Use `systemd-run --user --scope -q -p MemoryMax=2G` around them. Generation needs
`x86_64-w64-mingw32-objdump`; the resolver does not disassemble: it reads only the Rust table
(parsed back, and checked against what the generator meant) and the other file, as the addon
will.

## What the table holds, and why it holds no byte of the game

Per anchor (the 51 spans of the certificate, 19 constructors that install the 18 vtables, and
the context getter that reads the TLS index): RVA, length, alignment, the positions and kinds of
the fields that move with the code (the mask), where each field pointed as an identity, the
SHA-256 of the anchor with those fields set to zero (normalized digest), how many places of the
certified build matched it, and for the anchors that are searched, `k` and the offset of one
window and its rolling hash. Per callee that is not an anchor: bound, length, field positions
and normalized digest. Per `.rdata` content a span reads: length, positions of its 8-byte
absolute pointers (base relocations, zeroed) and digest. Per vtable: RVA and the slots it must
hold.

The rolling hash is `sum(w[j] * 16777619^(15-j)) mod 4294967291` over a window of 16 bytes with
no field in it: 32 bits kept of a 128-bit window, so each kept value is shared by about 2^96
windows. It only proposes candidates; the normalized digest of the whole anchor confirms each.
Besides digests, positions, lengths, RVAs and counts, nothing derived from the bytes is kept.
`test_relocation.py` (leak) checks that no 8-byte window of any range the table keeps a digest
of appears in the generated files, in hex or in decimal.

Limit: 13 anchors (the small getters, 3 to 14 bytes, whose raw digests the certificate already
publishes) and 5 callees (4 to 17 bytes) have fewer than 16 bytes outside their fields; their
digests are of the same kind as those of the certificate.

## Rules (study, section 4), as the resolver applies them

1. Identity first: if the certificate holds at its fixed offsets, every anchor is where it was
   certified (and the constructors and the TLS getter must match there).
2. A search places an anchor only if it matched once when certified and once now.
3. A derived anchor (field target, jump-table entry, vtable slot) must match its normalized
   digest where it is derived.
4. Every masked field must point at the new place of what it pointed at: another anchor, a
   vtable (checked by its slots), the image base, the TLS index of the TLS directory, the same
   offset inside the anchor or inside its `.pdata` function, a callee with the same bound,
   length and normalized digest, or `.rdata` content with the same digest and pointers.
5. Redundant paths must agree (the hash table by search and by six rip fields, a vtable by two
   constructors, `stack_quantity_getter` by two slots).
6. A vtable is the target of its constructor's lea, and each slot holds image base plus the
   located function.
7. Field offsets and constants are never masked.
8. Header: AMD64, PE32+, the image base, the same 7 sections in order; sizes from the file.
9. All or nothing: one anchor or vtable not located is `unsupported_build`, named with its rule.

## Decisions taken in this lot

- The window is the first field-free 16-byte window that occurs only once in the certified
  section, not the first one: first windows are often prologues (up to 2810 occurrences
  measured); the chosen ones gave 63 hash hits in all over `.text` and `.rdata`.
- Small anchors without a 16-byte field-free window (5 getters that are unique, and
  `bag_item_constructor`, whose longest field-free run is 13 bytes) are not searched: they are
  derived from vtable slots or from `item_factory_bag_case`, as in the study.
- Twins: an anchor that matched several places when certified, of which exactly one passed
  every field check (the constructors of conditional items A and B, which are each other's
  twin), is taken from the search when exactly one passes now. The graph is tried first.
- Constructors are only locators of vtables: their other fields must only stay in `.text` or
  `.rdata` (fingerprinting them would add about 160 callees and 70 contents the readers do not
  use), and one that is not found does not fail the resolution if its vtable is located. A
  constructor with no 16-byte field-free window is left out (the second lea site of
  `buff_manager`, 18 bytes; in `evidence.json`).
- Three slots are added, not two: `bag_item +0x260` for `null_stackable`, and both
  `player_stats +0x260` and `conditional predicate C +0x28` for `stack_quantity_getter`.
- `.rdata` content read through a `lea` is taken as 16 bytes (its size is not in the
  instruction).

## Measured (16 October 2026, copy `4a220846…a64d`)

- Search in Python: 8.3 s over the 39,130,412 bytes of `.text` and `.rdata`, 63 hash hits,
  1 false candidate before the digest (`item_factory_bag_case`).
- The same scan in Rust (`rustc -O`, this machine, file in memory, 57 hashes): 210 ms
  (5.4 ns/byte); with a 256-entry table for the outgoing byte, 116 ms (3.0 ns/byte), same hits.
  Supposition, not measured: inside the hash pass of `executable.rs`, in 1 s slices on the
  player's machine, that is a fraction of a second of CPU spread over the slices.
- After the scan the resolver reads less than 1 MiB at points, plus the `.pdata` and `.reloc`
  sections (about 1.9 MiB), which come after `.text` and `.rdata` in the file.

## Not proven

- Any real update of the game: the moved copy of `test_relocation.py` moves a block of `.text`
  and a block of `.rdata` coherently for what the resolver reads, not 32-bit RVAs in `.rdata`
  (RTTI, unwind data), which it does not read.
- The fields of callees (only their own digest is checked) and the resolver in Rust.
