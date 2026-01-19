use nix::libc;
use std::ptr;

pub struct HugePagePool {
    ptr: *mut u8,
    size: usize,
}

// Safety: The pointer is only accessed through controlled methods
unsafe impl Send for HugePagePool {}

impl HugePagePool {
    pub fn new(size: usize) -> Option<Self> {
        let ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_HUGETLB,
                -1,
                0,
            )
        };

        if ptr == libc::MAP_FAILED {
            return None;
        }

        Some(Self {
            ptr: ptr as *mut u8,
            size,
        })
    }

    pub fn ptr(&self) -> *mut u8 {
        self.ptr
    }

    pub fn size(&self) -> usize {
        self.size
    }
}

impl Drop for HugePagePool {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.size);
        }
    }
}
