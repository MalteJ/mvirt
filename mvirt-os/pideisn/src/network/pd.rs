use crate::network::DelegatedPrefix;
use std::collections::HashMap;
use std::net::Ipv6Addr;
use std::sync::Mutex;

static PREFIX_POOL: Mutex<Option<PrefixPool>> = Mutex::new(None);

pub struct PrefixPool {
    delegated_prefix: Ipv6Addr,
    delegated_prefix_len: u8,
    valid_lifetime: u32,
    preferred_lifetime: u32,
    allocated: HashMap<String, Ipv6Addr>,
    next_index: u64,
}

impl PrefixPool {
    fn new(prefix: DelegatedPrefix) -> Self {
        Self {
            delegated_prefix: prefix.prefix,
            delegated_prefix_len: prefix.prefix_len,
            valid_lifetime: prefix.valid_lifetime,
            preferred_lifetime: prefix.preferred_lifetime,
            allocated: HashMap::new(),
            next_index: 1, // Start at 1, 0 is reserved for the host
        }
    }

    pub fn allocate_address(&mut self, vm_id: &str) -> Option<Ipv6Addr> {
        // Check if already allocated
        if let Some(addr) = self.allocated.get(vm_id) {
            return Some(*addr);
        }

        // For a /64 prefix, we can allocate individual addresses
        // The host uses ::1, VMs get ::2, ::3, etc.
        if self.delegated_prefix_len != 64 {
            // For other prefix lengths, we'd need different logic
            return None;
        }

        let segments = self.delegated_prefix.segments();
        let addr = Ipv6Addr::new(
            segments[0],
            segments[1],
            segments[2],
            segments[3],
            0,
            0,
            0,
            self.next_index as u16,
        );

        self.allocated.insert(vm_id.to_string(), addr);
        self.next_index += 1;

        Some(addr)
    }

    pub fn release_address(&mut self, vm_id: &str) {
        self.allocated.remove(vm_id);
    }

    pub fn get_prefix(&self) -> (Ipv6Addr, u8) {
        (self.delegated_prefix, self.delegated_prefix_len)
    }
}

pub fn store_delegated_prefix(prefix: DelegatedPrefix) {
    let mut pool = PREFIX_POOL.lock().unwrap();
    *pool = Some(PrefixPool::new(prefix));
}

pub fn allocate_address_for_vm(vm_id: &str) -> Option<Ipv6Addr> {
    let mut pool = PREFIX_POOL.lock().unwrap();
    pool.as_mut()?.allocate_address(vm_id)
}

pub fn release_address_for_vm(vm_id: &str) {
    let mut pool = PREFIX_POOL.lock().unwrap();
    if let Some(p) = pool.as_mut() {
        p.release_address(vm_id);
    }
}

pub fn get_delegated_prefix() -> Option<(Ipv6Addr, u8)> {
    let pool = PREFIX_POOL.lock().unwrap();
    pool.as_ref().map(|p| p.get_prefix())
}
