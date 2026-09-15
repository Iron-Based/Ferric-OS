#![no_std]
// deny (not forbid) so Slint's macro-generated code — which carries its own
// `allow(unsafe_code)` overrides deep inside `include_modules!` — may compile.
// forbid is sticky and forbids those overrides; deny preserves the guarantee
// that our own hand-written code in this crate stays free of unsafe.
#![deny(unsafe_code)]

extern crate alloc;

// Each `.slint` entry point compiles to its own generated module (build.rs
// compiles both); `include_modules!` would only pull the last one, so include
// each file explicitly.
include!(concat!(env!("OUT_DIR"), "/main.rs"));
include!(concat!(env!("OUT_DIR"), "/monitor.rs"));
include!(concat!(env!("OUT_DIR"), "/keyboard.rs"));

/// Builds the top-level `MainWindow` component; panics only if Slint's
/// backend setup failed.
pub fn main_window() -> MainWindow {
    MainWindow::new().expect("Slint MainWindow creation failed")
}

/// Builds the live hardware-monitor component; panics only if Slint's
/// backend setup failed.
pub fn monitor_window() -> MonitorWindow {
    MonitorWindow::new().expect("Slint MonitorWindow creation failed")
}

/// Builds the on-screen keyboard component; panics only if Slint's backend
/// setup failed.
pub fn keyboard_window() -> KeyboardWindow {
    KeyboardWindow::new().expect("Slint KeyboardWindow creation failed")
}

#[cfg(test)]
mod host_churn {
    #![allow(unsafe_code)]
    extern crate std;

    use core::time::Duration;
    use std::boxed::Box;
    use std::rc::Rc;

    use slint::platform::software_renderer::{
        MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
    };
    use slint::platform::{self, Platform, PlatformError, WindowAdapter};

    /// Mirror of the kernel's first-fit free-list heap (heap.rs), used as the
    /// test-process allocator so freed Slint blocks are IMMEDIATELY reused the
    /// way the kernel's allocator does — the one behavior the host's OS
    /// allocator does not share. If the churn test then reproduces the kernel
    /// mem_move corruption, the allocator-reuse mechanism is proven.
    mod kernel_heap_mirror {
        extern crate std;
        use core::alloc::{GlobalAlloc, Layout};
        use std::sync::Mutex;

        const ARENA_SIZE: usize = 256 * 1024 * 1024;
        // SAFETY: only touched by alloc/dealloc, serialized under LOCK.
        static mut ARENA: [u8; ARENA_SIZE] = [0; ARENA_SIZE];

        #[repr(C)]
        struct Header {
            size: usize,
            next: *mut Header,
            prev: *mut Header,
        }
        const HEADER_SIZE: usize = core::mem::size_of::<Header>();
        const HEADER_GAP: usize = core::mem::size_of::<usize>();
        const MIN_BLOCK: usize = HEADER_SIZE * 2;

        fn align_up(value: usize, align: usize) -> usize {
            (value + (align - 1)) & !(align - 1)
        }

        struct Heap {
            free: *mut Header,
            start: usize,
            end: usize,
            initialized: bool,
        }
        impl Heap {
            fn ensure_init(&mut self) {
                if self.initialized {
                    return;
                }
                self.initialized = true;
                let start = core::ptr::addr_of!(ARENA) as usize;
                let start = align_up(start, 8);
                let end = (start + ARENA_SIZE) & !7;
                let size = end - start;
                // SAFETY: single init under LOCK; writes only the head header.
                unsafe {
                    let head = start as *mut Header;
                    (*head).size = size;
                    (*head).next = core::ptr::null_mut();
                    (*head).prev = core::ptr::null_mut();
                    self.free = head;
                }
                self.start = start;
                self.end = end;
            }
            fn alloc(&mut self, layout: Layout) -> *mut u8 {
                self.ensure_init();
                let size = core::cmp::max(layout.size(), 1);
                let align = layout.align();
                let mut current = self.free;
                while !current.is_null() {
                    // SAFETY: free-list walk over nodes this allocator created.
                    let node = unsafe { &*current };
                    let block_addr = current as usize;
                    let block_size = node.size;
                    let payload = align_up(block_addr + HEADER_SIZE + HEADER_GAP, align);
                    let Some(need_end) = payload.checked_add(size) else {
                        current = node.next;
                        continue;
                    };
                    if need_end > block_addr + block_size {
                        current = node.next;
                        continue;
                    }
                    let head_pad = payload - (block_addr + HEADER_SIZE);
                    let block_end = block_addr + block_size;
                    let tail = block_end - need_end;
                    let split = tail >= MIN_BLOCK && block_end - align_up(need_end, 8) >= MIN_BLOCK;
                    if split {
                        let tail_addr = align_up(need_end, 8);
                        let tail_size = block_end - tail_addr;
                        // SAFETY: rewires the free list in-region.
                        unsafe {
                            let tail_hdr = tail_addr as *mut Header;
                            (*tail_hdr).size = tail_size;
                            (*tail_hdr).next = (*current).next;
                            (*tail_hdr).prev = (*current).prev;
                            if !(*current).prev.is_null() {
                                (*(*current).prev).next = tail_hdr;
                            } else {
                                self.free = tail_hdr;
                            }
                            if !(*current).next.is_null() {
                                (*(*current).next).prev = tail_hdr;
                            }
                            (*current).size = tail_addr - block_addr;
                        }
                    } else {
                        // SAFETY: unlinks `current`.
                        unsafe {
                            if !(*current).prev.is_null() {
                                (*(*current).prev).next = (*current).next;
                            } else {
                                self.free = (*current).next;
                            }
                            if !(*current).next.is_null() {
                                (*(*current).next).prev = (*current).prev;
                            }
                        }
                    }
                    // SAFETY: stores pad in the reserved gap slot below payload.
                    unsafe {
                        *((payload - HEADER_GAP) as *mut usize) = head_pad;
                    }
                    return payload as *mut u8;
                }
                core::ptr::null_mut()
            }
            fn dealloc(&mut self, ptr: *mut u8, _layout: Layout) {
                if ptr.is_null() {
                    return;
                }
                let payload = ptr as usize;
                if !self.initialized || payload < self.start + HEADER_SIZE || payload >= self.end {
                    return;
                }
                // SAFETY: payload came from alloc(); the gap slot holds the pad.
                let offset = unsafe { *((payload - HEADER_GAP) as *const usize) };
                let block_addr = payload - HEADER_SIZE - offset;
                if block_addr < self.start || block_addr >= self.end {
                    return;
                }
                let block = block_addr as *mut Header;
                // SAFETY: block is a live allocation header.
                let block_end = unsafe { block_addr + (*block).size };
                let mut left = core::ptr::null_mut();
                let mut right = core::ptr::null_mut();
                let mut current = self.free;
                while !current.is_null() {
                    // SAFETY: free-list walk.
                    let node = unsafe { &*current };
                    let node_addr = current as usize;
                    if node_addr + node.size == block_addr {
                        left = current;
                    }
                    if node_addr == block_end {
                        right = current;
                    }
                    current = node.next;
                }
                if !right.is_null() {
                    // SAFETY: merges right into block.
                    unsafe {
                        (*block).size += (*right).size;
                        if !(*right).prev.is_null() {
                            (*(*right).prev).next = (*right).next;
                        } else {
                            self.free = (*right).next;
                        }
                        if !(*right).next.is_null() {
                            (*(*right).next).prev = (*right).prev;
                        }
                    }
                }
                if !left.is_null() {
                    // SAFETY: folds block into left.
                    unsafe {
                        (*left).size += (*block).size;
                    }
                    return;
                }
                // SAFETY: prepends the freed block.
                unsafe {
                    (*block).next = self.free;
                    (*block).prev = core::ptr::null_mut();
                    if !self.free.is_null() {
                        (*self.free).prev = block;
                    }
                    self.free = block;
                }
            }
        }

        // SAFETY: every call serialized under LOCK; arena is a never-moved static.
        unsafe impl Sync for Heap {}
        // SAFETY: see Sync; only alloc/dealloc under LOCK.
        unsafe impl Send for Heap {}

        static LOCK: Mutex<()> = Mutex::new(());
        // SAFETY: only alloc/dealloc (under LOCK) write this; never moved.
        static mut HEAP: Heap = Heap {
            free: core::ptr::null_mut(),
            start: 0,
            end: 0,
            initialized: false,
        };

        struct KernelAlloc;
        // SAFETY: forwards to Heap under LOCK; contract of GlobalAlloc.
        unsafe impl GlobalAlloc for KernelAlloc {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
                let _guard = LOCK.lock().unwrap();
                // SAFETY: HEAP only touched while LOCK held.
                let heap = unsafe { &mut *core::ptr::addr_of_mut!(HEAP) };
                heap.alloc(layout)
            }
            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
                let _guard = LOCK.lock().unwrap();
                // SAFETY: as above.
                let heap = unsafe { &mut *core::ptr::addr_of_mut!(HEAP) };
                heap.dealloc(ptr, layout);
            }
        }

        #[global_allocator]
        static GLOBAL: KernelAlloc = KernelAlloc;
    }

    /// Host-side twin of the kernel's `FerricPlatform`. The window is NOT kept
    /// forever: like the kernel's `WINDOW.set`, each `create_window_adapter`
    /// builds a fresh `MinimalSoftwareWindow` and drops the previous one, so
    /// the full window-teardown churn is exercised on the host too.
    struct TestPlatform(Rc<std::cell::RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
    impl Clone for TestPlatform {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }
    }
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
            let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
            self.0.replace(Some(window.clone()));
            window.set_size(slint::PhysicalSize::new(1024, 768));
            window.show()?;
            Ok(window)
        }
        fn duration_since_start(&self) -> Duration {
            Duration::ZERO
        }
    }

    /// Full setup mirror of the kernel's `run_keyboard`: construct, register
    /// every no-arg/arg callback handler the kernel registers, focus-init, then
    /// poke the two-way props and render. Targets the `Callback<(),()>`
    /// handler-cell set/replace path that faults on the kernel during
    /// construction.
    #[test]
    fn callback_setup_churn() {
        const W: u32 = 1024;
        const H: u32 = 768;
        const ROUNDS: usize = 200;
        let window_slot = Rc::new(std::cell::RefCell::new(None::<Rc<MinimalSoftwareWindow>>));
        let platform = TestPlatform(window_slot.clone());
        platform::set_platform(Box::new(platform.clone())).unwrap();
        let mut buf = std::vec![PremultipliedRgbaColor::default(); (W * H) as usize];
        for _round in 0..ROUNDS {
            let win = super::keyboard_window();
            let window = window_slot
                .borrow()
                .as_ref()
                .expect("window created by keyboard_window()")
                .clone();
            win.on_char_pressed(|_low: slint::SharedString, _high: slint::SharedString| {});
            win.on_space_pressed(|| {});
            win.on_backspace_pressed(|| {});
            win.on_enter_pressed(|| {});
            win.on_shift_pressed(|| {});
            win.on_caps_pressed(|| {});
            win.invoke_init_focus();
            for _ in 0..10 {
                win.set_typed_text(slint::SharedString::from(std::string::String::from(
                    "abcde12345",
                )));
                window.draw_if_needed(|renderer| {
                    renderer.render(&mut buf, W as usize);
                });
                win.set_typed_text(slint::SharedString::default());
                let _ = win.get_typed_text();
                window.draw_if_needed(|renderer| {
                    renderer.render(&mut buf, W as usize);
                });
            }
        }
    }

    /// Mirror of the kernel's keyboard churn: construct the component, poke the
    /// two-way text binding and selection props, render a frame (text layout
    /// path), then drop. Exercises Slint's dependency tracking end to end,
    /// including window recreation per construction.
    #[test]
    fn construction_churn() {
        const W: u32 = 1024;
        const H: u32 = 768;
        const ROUNDS: usize = 200;
        let window_slot = Rc::new(std::cell::RefCell::new(None::<Rc<MinimalSoftwareWindow>>));
        let platform = TestPlatform(window_slot.clone());
        platform::set_platform(Box::new(platform.clone())).unwrap();
        let mut buf = std::vec![PremultipliedRgbaColor::default(); (W * H) as usize];
        for _round in 0..ROUNDS {
            let win = super::keyboard_window();
            let window = window_slot
                .borrow()
                .as_ref()
                .expect("window created by keyboard_window()")
                .clone();
            for sel_col in 0..4 {
                win.set_sel_col(sel_col);
            }
            win.set_sel_row(1);
            win.set_shifted(true);
            win.set_caps_on(false);
            for _ in 0..10 {
                win.set_typed_text(slint::SharedString::from(std::string::String::from(
                    "abcde12345",
                )));
                window.draw_if_needed(|renderer| {
                    renderer.render(&mut buf, W as usize);
                });
                win.set_typed_text(slint::SharedString::default());
                let _ = win.get_typed_text();
                window.draw_if_needed(|renderer| {
                    renderer.render(&mut buf, W as usize);
                });
            }
            window.draw_if_needed(|renderer| {
                renderer.render(&mut buf, W as usize);
            });
        }
    }
}
