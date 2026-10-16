# Content certificate `code-208704-r2` (16 October 2026)

Where `core/src/certificate_data.rs` comes from. A change of that table is a recertification.

Revision 1 (`code-208704-r1`, addon 0.8.14) checked 51 ranges and 43 slots. Revision 2 adds 6
ranges and 5 slots for the four field offsets the readers used and no range named:
item `+0x58`, inventory `+0xd0`, item context `+0x38` and definition `+0x28`. Nothing of
revision 1 changed: its 51 ranges and 43 slots come out of `gen.py` byte for byte the same.

## Inputs

- `docs/audit/2026-10-15-recertification-build-208704.json`: the 37 ranges kept by digest
  (`guards`, file offset, size, `new_sha256`), the 18 vtables with the slots the readers
  check (`vtables[].evidence`, `+0xNN -> 0xRVA`), the sections and the image size.
- A copy of `Gw2-64.exe` of the game's update of 16 October 2026, 08:59 CEST, sha256
  `4a220846c8136dac90380ad0545304d6622a84cf6b0d45a46014212125c4a64d`, 43065736 bytes.
  Measured on it before anything was taken from it: the 37 ranges hash to the digests of
  15 October at the same file offsets, the 43 slots hold the same functions, the 7 sections
  and the image size are the same, and the TLS directory's AddressOfIndex is `0x28155c0`.
- `offsets.json` (revision 2): the ranges and slots that back the four field offsets, as RVAs
  of that copy, with the operands each range must hold. The reasons are below.

## The 14 functions compared by address only

They had no digest. `funcs.py` takes each one's bounds from `.pdata` (or, for a leaf without an
entry, up to its first `ret`), disassembles it with `objdump -M intel` and looks for the
instructions the recertification of 15 October quoted for it, in order. Nine match as text;
the other five (`final_stack_quantity_getter`, `stack_quantity_getter`,
`conditional_predicate_A`, `_B`, `_C`) were read one by one and hold the same instructions,
written differently by objdump (`0xffffffffffffff58` for `-0xa8`, `[rcx+0x8]` for
`[rcx + 8]`, `rex.W jmp`). `funcs.json` is its output: offset, size, bound and digest.

## The four field offsets (revision 2)

Until revision 1 these four were assumptions carried over from the proof of 6 October: the
recertification of 15 October says that no range it checked names them. Each now has code of
the game that reads that field on the readers' own route, chosen so that a patch that moved the
field would have to change that code. Only file offsets, lengths and digests of it are in the
repository; what it does is described here in words.

**Item `+0x58`, the inventory that owns an item** (read by the inventory reader, which compares
it with the inventory it came from).

- `item:inventory_location_getter`, RVA `0x13c4d80`, 115 bytes, one `.pdata` entry. The item's
  method for its place in the inventory (`ItCliItem.cpp`): it calls the item's own slot `+0x70`
  (the location getter the certificate already checks) and asserts that the item is in the
  inventory location, asserts that the pointer at `+0x58` is not null, and returns that pointer
  together with the slot index kept in the word at `+0x48`. It reads `+0x58` twice and nothing
  else of the item besides `+0x48`.
- Slots `+0x170` of the four item vtables the certificate already names (`bag_item (ItCliBag)`,
  `conditional item A`, `B`, `C`) select it. Measured on the copy, not in the table: all 15 item
  vtables the reader accepts (slot `+0x8` the definition getter and `+0x68`, `+0x70`, `+0xa0` the
  route) have that same function at `+0x170`.

**Inventory `+0xd0`, the capacity of the slot array** (read by the inventory reader, which holds
the count at `+0xd4` to it, and by the bag reader).

- `item:inventory_slots_message + bag:inventory_slots_message`, RVA `0x1251410`, 140 bytes. A
  message handler (one entry of a table of handlers in `.rdata`, each paired with a descriptor
  in `.data`) that walks the readers' own route: it takes the game context
  (`0x9b1fb0`), the character context at `+0x98`, the character by an id of the message, the
  agent at character `+0x8` and its slot `+0xc8` (`character_inventory_getter`, already
  certified), and passes that inventory and a count of the message to the next function.
- `item:inventory_slots_resize + bag:inventory_slots_resize`, RVA `0x128d680`, 276 bytes
  (`ChCliItemStorageInventory.cpp`). It sets the number of inventory slots: when it grows, it
  hands the array object at inventory `+0xc0` to the array's reserve function, clears the new
  entries from the pointer at `+0xc8` and stores the new count at `+0xd4`; when it shrinks, it
  asserts that the slots it drops are empty.
- `item:array_reserve + bag:array_reserve`, RVA `0x244840` to `0x2449b8`, 376 bytes, the 11
  contiguous `.pdata` entries of one function (the reserve of the engine's array, `Array.h`).
  It compares the requested count with the dword at array `+0x10` and returns when it fits;
  otherwise it allocates 8 bytes per element, copies the count at `+0x14` of them from the
  pointer at `+0x8`, frees the old block and stores the new pointer at `+0x8` and the new
  capacity at `+0x10`. With the array at inventory `+0xc0`, that is the pointer at `+0xc8` and
  the count at `+0xd4` the certified getters read, and the capacity at `+0xd0`.

**Item context `+0x38`, the capacity of the item array** (read by the inventory reader, which
holds the length at `+0x3c` to it).

- `item:item_array_growth`, RVA `0x13c78ee` to `0x13c7923`, 53 bytes: a straight block of the
  item factory (`ItCliContext.cpp`, function at `0x13c7720`) that ends where the certified
  `bag:item_factory_type_switch` begins. The factory's `this` is the item context: at its
  entry it calls its own slot `+0x10` (the `item_resolver` the certificate checks) with the
  index. The block compares the index plus one with the length at `+0x3c`, hands the array
  object at `+0x28` to the same reserve function, clears the new entries from the pointer at
  `+0x30` and stores the new length at `+0x3c`. With `item:array_reserve`, the capacity is at
  `+0x28 + 0x10 = +0x38`.

**Definition `+0x28`, an item's ID** (read by the inventory reader as the ID it counts).

- `item:definition_id_match`, RVA `0x13c6a50`, 146 bytes, one `.pdata` entry
  (`ItCliContext.cpp`): whether two items stack. When the first item has a stack (its slot
  `+0x260`, the quantity getter slot the reader follows), it asks each item for its definition
  (slot `+0x8`, the certified `item_definition_getter`) and compares the dword at `+0x28` of the
  two: equal means they stack.
- Slot `+0x0` of the `item_context` vtable selects it.

That `+0x28` is the ID, and not only the field the game compares to stack items, still rests on
the live reading of 6 October, which matched the IDs with the API; the range makes sure the
game still treats that dword as what tells definitions apart.

## Offsets the readers use with no range, also after revision 2

Checked for the inventory, bag and wallet readers field by field against the disassembly of
the certified ranges; for the Magic Find reader only its route and its buff table.

- TLS block `+0x10`, where `context_from_teb` finds the game context. The game's own context
  getter (`0x9b1fb0`, a 29-byte leaf) reads it; no range holds it. The character context
  read through that context is checked by its vtable on every pass, so a wrong context fails
  closed there.
- Context `+0x178`, the item context. The recertification of 15 October names the accessor that
  returns it (`0x13c71f0`), but not as a range. The reader checks the item context's vtable on
  every pass.
- Context `+0x198`, which the reader only requires to be non-null before it reads. There is an
  accessor that returns it (`0x13e7ba0`), but what the field is was not established, so it was
  not certified. A wrong field there would only let a pass start whose next checks are
  vtables.
- Magic Find: a buff node's key at `+0x18`, which the reader compares with the bucket's key; no
  range read here holds it. A moved field would fail that comparison unless it happened to hold
  the key.
- The two pointer rules of the bag and Magic Find readers (heap objects on 8 bytes, game content
  4 past), which are observations and not offsets of code.

Each of the first three could be a range of the same kind as the six above; they are left as a
proposal because the readers already check by vtable what those offsets lead to.

## Generation

```
python3 -I funcs.py <copy of Gw2-64.exe> <recertification json>          # writes funcs.json
python3 -I offsets.py <copy of Gw2-64.exe> offsets.json > offsets_measured.json
python3 -I gen.py <copy of Gw2-64.exe> <recertification json> funcs.json offsets_measured.json > certificate_data.rs
```

`gen.py` asserts that each slot holds image base plus the proven RVA in the file, and that each
range of `offsets_measured.json` still hashes to its digest. `offsets.py` takes each range's
bounds from `.pdata` (`pdata`: one entry starting at the RVA; `pdata-run`: contiguous entries
from the first to the last; `block`: two instruction boundaries of the named function), asserts
that the operands of `offsets.json` appear in it in order, and that each slot selects its
function. It needs `x86_64-w64-mingw32-objdump`.

## Checked

Revision 1:

- `cargo test --offline -p tyrian_companion_nexus_core`: 459 passed, 0 failed, 1 ignored.
- The ignored test over the copy above: `certified by content (code-208704-r1), sha256 4a220846…`.
- The same over a copy with one byte of `mf:modifier_sum` changed: `Err("mf:modifier_sum")`.
- The comparison of the span digests disabled on purpose: 2 tests red; restored: green.

Revision 2:

- `cargo test --offline -p tyrian_companion_nexus_core`: 459 passed, 0 failed, 1 ignored.
- The ignored test over the copy above: `certified by content (code-208704-r2), sha256 4a220846…`.
- The same over a copy with one byte changed in each new range, the byte of the field offset
  where there is one (`+0x58` to `+0x60`; `+0x98` to `+0xa0`; the array at `+0xc0` to `+0xc8`;
  the capacity at `+0x10` to `+0x18`; the array at `+0x28` to `+0x30`; `+0x28` to `+0x2c`), and
  in byte 1 of each new slot: 11 runs, each `Err` with the name of that range or slot.
- `offsets.py` with one operand of `offsets.json` changed (`+0x58` to `+0x60`) and with one slot
  moved (`+0x170` to `+0x168`): both stop on an assertion naming the range or the slot.

## Not proven

- Any reading from a running game with this addon. Native Windows.
- That the rest of the executable is unchanged since 15 October: the file of 15 October is
  gone; only what the readers rely on was compared, which is what the certificate checks.
- That the game keeps using these six ranges for those objects. A patch that moved one of the
  four fields has to change them, as long as they are still the code that reads it; one that
  moved the field, gave the objects other code for it and left these functions in place,
  unchanged and unused, would not be caught. The slots tie two of them to the classes the
  readers check, and the handler ties the resize to the readers' route; that none of them is
  dead code was not measured.
