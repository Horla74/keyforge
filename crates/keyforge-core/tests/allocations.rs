//! Repeatable engine-only allocation benchmark; no native keyboard injection.
use keyforge_core::{CompiledConfig, Engine, KeyCode, KeyState};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    time::Instant,
};
thread_local! { static ALLOCS: Cell<usize> = const { Cell::new(0) }; }
struct Counter;
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.with(|n| n.set(n.get() + 1));
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCS.with(|n| n.set(n.get() + 1));
        System.realloc(ptr, layout, size)
    }
}
#[global_allocator]
static ALLOCATOR: Counter = Counter;

#[test]
fn repeated_remap_allocations() {
    let config = CompiledConfig::from_toml_str("[general]\nlog_enabled = false\n[profiles.default]\n[[profiles.default.maps]]\nkey = 'f13'\nremap = 'f14'").unwrap();
    let mut engine = Engine::new(config);
    let run = |engine: &mut Engine| {
        std::hint::black_box(engine.on_key(KeyCode::F13, KeyState::Down));
        std::hint::black_box(engine.on_key(KeyCode::F13, KeyState::Up));
    };
    run(&mut engine);
    ALLOCS.with(|n| n.set(0));
    let start = Instant::now();
    for _ in 0..100_000 {
        run(&mut engine);
    }
    let elapsed = start.elapsed();
    let count = ALLOCS.with(Cell::get);
    println!("100000 remap down/up pairs: {count} allocations/reallocations, {elapsed:?}");
    assert!(
        count <= 1_200_000,
        "disabled logging remap allocation budget exceeded"
    );
    assert!(engine.take_last_activation().is_none());
    engine.set_activation_logging(true);
    run(&mut engine);
    let description = engine.take_last_activation().unwrap();
    assert!(description.contains("f13"));
    assert!(description.contains("f14"));
    engine.set_activation_logging(false);
    run(&mut engine);
    assert!(engine.take_last_activation().is_none());
}
