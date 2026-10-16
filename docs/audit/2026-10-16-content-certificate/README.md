# Content certificate `code-208704-r1` (16 October 2026)

Where `core/src/certificate_data.rs` comes from. A change of that table is a recertification.

## Inputs

- `docs/audit/2026-10-15-recertification-build-208704.json`: the 37 ranges kept by digest
  (`guards`, file offset, size, `new_sha256`), the 18 vtables with the slots the readers
  check (`vtables[].evidence`, `+0xNN -> 0xRVA`), the sections and the image size.
- A copy of `Gw2-64.exe` of the game's update of 16 October 2026, 08:59 CEST, sha256
  `4a220846c8136dac90380ad0545304d6622a84cf6b0d45a46014212125c4a64d`, 43065736 bytes.
  Measured on it before anything was taken from it: the 37 ranges hash to the digests of
  15 October at the same file offsets, the 43 slots hold the same functions, the 7 sections
  and the image size are the same, and the TLS directory's AddressOfIndex is `0x28155c0`.

## The 14 functions compared by address only

They had no digest. `funcs.py` takes each one's bounds from `.pdata` (or, for a leaf without an
entry, up to its first `ret`), disassembles it with `objdump -M intel` and looks for the
instructions the recertification of 15 October quoted for it, in order. Nine match as text;
the other five (`final_stack_quantity_getter`, `stack_quantity_getter`,
`conditional_predicate_A`, `_B`, `_C`) were read one by one and hold the same instructions,
written differently by objdump (`0xffffffffffffff58` for `-0xa8`, `[rcx+0x8]` for
`[rcx + 8]`, `rex.W jmp`). `funcs.json` is its output: offset, size, bound and digest.

## Generation

```
python3 -I funcs.py <copy of Gw2-64.exe> <recertification json>        # writes funcs.json
python3 -I gen.py <copy of Gw2-64.exe> <recertification json> funcs.json > certificate_data.rs
```

`gen.py` also asserts that each slot holds image base plus the proven RVA in the file.

## Checked

- `cargo test --offline -p tyrian_companion_nexus_core`: 459 passed, 0 failed, 1 ignored.
- The ignored test over the copy above: `certified by content (code-208704-r1), sha256 4a220846…`.
- The same over a copy with one byte of `mf:modifier_sum` changed: `Err("mf:modifier_sum")`.
- The comparison of the span digests disabled on purpose: 2 tests red; restored: green.

## Not proven

- Any reading from a running game with this addon. Native Windows.
- That the rest of the executable is unchanged since 15 October: the file of 15 October is
  gone; only what the readers rely on was compared, which is what the certificate checks.
