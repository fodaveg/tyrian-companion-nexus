//! What building and serializing one `live1` sample costs in allocations. The sample is the one
//! the 8 Oct 2026 audit measured: 150 objects and 55 currencies, 28 frames (6 898 bytes with this
//! data). While the frames were `serde_json::json!` trees it took 762 allocations; the bound
//! below keeps them from growing back into a tree of `Value`s.
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use tyrian_companion_nexus_core::inventory::InventorySnapshot;
use tyrian_companion_nexus_core::live::*;
use tyrian_companion_nexus_core::wallet::WalletSnapshot;

struct Counting;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}
#[global_allocator]
static COUNTING: Counting = Counting;

const NONCE: &str = "AQEBAQEBAQEBAQEBAQEBAQ";
const EPOCH: &str = "AgICAgICAgICAgICAgICAg";

fn sample() -> InventorySnapshot {
    let balances: BTreeMap<u32, u32> = (0..55).map(|n| (1 + n, n * 1013)).collect();
    InventorySnapshot {
        owner: (0x10a000, 0x10b000),
        quantities: (0..150).map(|n| (12000 + n * 7, 1 + n * 3)).collect(),
        unknown: 0,
        positions: 570,
        free_slots: Some(37),
        wallet: Some(WalletSnapshot::checked((0x210000, 0x220000), balances).unwrap()),
    }
}

#[test]
fn a_150_object_55_currency_sample_costs_few_allocations() {
    let sample = sample();
    // One warm-up, so lazily initialised statics are not counted.
    let _ = snapshot_frames(EPOCH, 7, 12, 12_345, &sample);
    let started = std::time::Instant::now();
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let frames = snapshot_frames(EPOCH, 7, 12, 12_345, &sample).unwrap();
    let lines: Vec<String> = frames
        .into_iter()
        .enumerate()
        .map(|(n, f)| frame_line(f, NONCE, n as u64).unwrap())
        .collect();
    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
    let micros = started.elapsed().as_micros();
    let bytes: usize = lines.iter().map(String::len).sum();
    eprintln!(
        "live1 sample: {} frames, {bytes} B, {micros} us, {allocations} allocations",
        lines.len()
    );
    assert_eq!(lines.len(), 28);
    assert_eq!(bytes, 6898);
    assert!(allocations <= 100, "{allocations} allocations");
}
