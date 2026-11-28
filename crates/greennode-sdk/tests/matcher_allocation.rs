#![cfg(feature = "test-support")]

use greennode_sdk::Secret;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static PHASE: AtomicU8 = AtomicU8::new(0);
static PHASE_PEAK: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];

fn record(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
    PHASE_PEAK[usize::from(PHASE.load(Ordering::Relaxed))].fetch_max(live, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let Ok(next) = Layout::from_size_align(size, layout.align()) else {
            return std::ptr::null_mut();
        };
        let replacement = unsafe { System.alloc(next) };
        if !replacement.is_null() {
            record(size);
            unsafe {
                std::ptr::copy_nonoverlapping(pointer, replacement, layout.size().min(size));
            }
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            unsafe {
                System.dealloc(pointer, layout);
            }
        }
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn phase(value: u8) {
    PHASE.store(value, Ordering::Relaxed);
    PHASE_PEAK[usize::from(value)].fetch_max(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
}

#[test]
fn allocation_probe_baseline() {
    assert_eq!(std::hint::black_box(1), 1);
}

#[test]
fn worst_case_credential_allocation_phases() {
    let mut body = vec![b'a'; 8 << 20];
    body[0] = b'"';
    let end = body.len() - 1;
    body[end] = b'"';
    for kind in ["diverse", "parts", "single"] {
        let baseline = LIVE.load(Ordering::Relaxed);
        PEAK.store(baseline, Ordering::Relaxed);
        for peak in &PHASE_PEAK {
            peak.store(0, Ordering::Relaxed);
        }
        let mut state = 0x5eed_u64;
        let count = if kind == "single" { 1 } else { 256 };
        let length = if kind == "single" { 256 << 10 } else { 1024 };
        let secrets: Vec<_> = (0..count)
            .map(|index| {
                let token = if kind != "parts" {
                    (0..length)
                        .map(|_| {
                            state ^= state << 13;
                            state ^= state >> 7;
                            state ^= state << 17;
                            char::from(33 + (state % 94) as u8)
                        })
                        .collect()
                } else {
                    let mut value = String::with_capacity(1024);
                    for part in 0..113 {
                        value.push_str(&format!("{:08x}\n", index * 113 + part));
                    }
                    value.push_str("zzzzzzz");
                    value
                };
                Secret::new(token)
            })
            .collect();
        phase(6);
        let (result, compile, large_screen, accounted) =
            greennode_sdk::testing::allocation_probe(&secrets, phase, &body);
        println!(
            "{kind} result={result} compile_small={compile:?} large_screen={large_screen:?} accounted={accounted}"
        );
        let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
        let phases: Vec<_> = PHASE_PEAK
            .iter()
            .map(|value| value.load(Ordering::Relaxed).saturating_sub(baseline))
            .collect();
        println!("{kind} allocation peak={peak} phases={phases:?}");
        assert_eq!(result, 0);
        assert!(peak <= accounted);
        assert!(accounted <= 64 << 20);
    }
}
