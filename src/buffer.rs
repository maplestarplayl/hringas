//! Implementation of provided buffer support for `hringas`.

use std::{
    cell::Cell,
    ffi::c_void,
    io,
    sync::atomic::{AtomicU16, Ordering},
};

use rustix::io_uring::{io_uring_buf, io_uring_buf_reg, io_uring_ptr};

mod alloc {
    use std::{
        ffi::c_void,
        io,
        ptr::{self, NonNull},
    };

    use rustix::mm;

    pub struct AlignedMem {
        ptr: NonNull<c_void>,
        len: usize,
    }

    impl AlignedMem {
        pub fn new(len: usize) -> io::Result<Self> {
            let prot_flags = mm::ProtFlags::READ | mm::ProtFlags::WRITE;
            let mmap_flags = mm::MapFlags::POPULATE | mm::MapFlags::SHARED;
            let ptr = unsafe {
                mm::mmap_anonymous(
                    ptr::null_mut(),
                    len,
                    prot_flags,
                    mmap_flags,
                )?
            };

            Ok(Self { ptr: NonNull::new(ptr).unwrap(), len })
        }

        pub fn as_mut_ptr(&self) -> *mut u8 {
            self.ptr.as_ptr() as *mut u8
        }
    }

    impl Drop for AlignedMem {
        fn drop(&mut self) {
            unsafe {
                let _ = mm::munmap(self.ptr.as_ptr(), self.len);
            }
        }
    }
}

/// A provided buffer ring for io_uring.
///
/// Documentation: https://man7.org/linux/man-pages/man3/io_uring_register_buf_ring.3.html
/// struct io_uring_buf_ring {
///                union {
///                    struct {
///                        __u64 resv1;
///                        __u32 resv2;
///                        __u16 resv3;
///                        __u16 tail;
///                    };
///                    struct io_uring_buf bufs[0];
///                };
///            };
/// struct io_uring_buf {
///               __u64 addr;
///               __u32 len;
///               __u16 bid;
///               __u16 resv;    // reserved for tail
///           };
pub struct BufRing {
    bgid: u16,
    ring_entries_mask: u16,

    mem: alloc::AlignedMem,

    shared_tail: *const AtomicU16,
    local_tail: Cell<u16>,
}

impl BufRing {
    pub fn new(bgid: u16, ring_entries: u16) -> io::Result<Self> {
        assert!(ring_entries.is_power_of_two());

        let entry_size = std::mem::size_of::<io_uring_buf>();
        let mem = alloc::AlignedMem::new(entry_size * ring_entries as usize)?;

        #[repr(C)]
        struct BufRingHeader {
            _resv1: u64,
            _resv2: u32,
            _resv3: u16,
            tail: u16,
        }
        let tail_offset = core::mem::offset_of!(BufRingHeader, tail);
        // SAFETY: `mem` is at least `header_size` bytes, page-aligned, and we
        // only read from the `tail` field.
        let shared_tail = unsafe {
            AtomicU16::from_ptr(
                mem.as_mut_ptr().byte_add(tail_offset).cast::<u16>(),
            )
        };

        Ok(Self {
            bgid,
            ring_entries_mask: ring_entries - 1,
            mem,
            shared_tail,
            local_tail: Cell::new(0),
        })
    }

    /// # Safety
    ///
    /// App must ensure the memory `addr` must be valid within kernel usage.
    pub unsafe fn push(&self, addr: u64, len: u32, bid: u16) {
        let tail = self.local_tail.get();
        let index = (tail & self.ring_entries_mask) as usize;

        let ptr = self.mem.as_mut_ptr() as *mut io_uring_buf;
        let entry = &mut *ptr.add(index);

        entry.addr = io_uring_ptr::from(addr as *mut c_void);
        entry.len = len;
        entry.bid = bid;

        self.local_tail.set(tail.wrapping_add(1));
    }

    pub fn sync(&self) {
        unsafe {
            (*self.shared_tail).store(self.local_tail.get(), Ordering::Release);
        }
    }

    pub fn args(&self) -> io_uring_buf_reg {
        let mut reg = io_uring_buf_reg::default();
        reg.bgid = self.bgid;
        reg.ring_addr =
            io_uring_ptr::from(self.mem.as_mut_ptr() as *mut c_void);
        reg.ring_entries = (self.ring_entries_mask + 1) as u32;
        reg
    }
}
