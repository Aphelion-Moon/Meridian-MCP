//! Bounded accounting for observed, procedure-attributed VM-thread allocations.
// SPDX-License-Identifier: MIT

use std::collections::HashMap;

pub struct Allocation {
    pub proc_id: u32,
    pub size: usize,
}

pub struct Accounting {
    pub live: HashMap<usize, Allocation>,
    pub live_bytes: u64,
    pub peak_bytes: u64,
    pub attributed_calls: u64,
    pub unattributed_calls: u64,
    pub capacity_exceeded: bool,
    limit: usize,
}

impl Accounting {
    pub fn new(limit: usize) -> Self {
        Self {
            live: HashMap::with_capacity(limit),
            live_bytes: 0,
            peak_bytes: 0,
            attributed_calls: 0,
            unattributed_calls: 0,
            capacity_exceeded: false,
            limit,
        }
    }

    pub fn allocate(&mut self, pointer: usize, size: usize, proc_id: Option<u32>) {
        if self.capacity_exceeded || pointer == 0 {
            return;
        }
        let Some(proc_id) = proc_id else {
            self.unattributed_calls = self.unattributed_calls.saturating_add(1);
            return;
        };
        if self.live.len() >= self.limit && !self.live.contains_key(&pointer) {
            self.capacity_exceeded = true;
            return;
        }
        if let Some(old) = self.live.insert(pointer, Allocation { proc_id, size }) {
            self.live_bytes -= old.size as u64;
        }
        self.live_bytes += size as u64;
        self.peak_bytes = self.peak_bytes.max(self.live_bytes);
        self.attributed_calls = self.attributed_calls.saturating_add(1);
    }

    pub fn free(&mut self, pointer: usize) {
        if self.capacity_exceeded {
            return;
        }
        if let Some(old) = self.live.remove(&pointer) {
            self.live_bytes -= old.size as u64;
        }
    }

    pub fn reallocate(&mut self, old: usize, new: usize, size: usize, proc_id: Option<u32>) {
        // UCRT realloc(p, 0) frees p. Failure for a nonzero size preserves p.
        if new == 0 && size != 0 {
            return;
        }
        let proc_id = proc_id.or_else(|| self.live.get(&old).map(|allocation| allocation.proc_id));
        self.free(old);
        self.allocate(new, size, proc_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frees_and_failed_realloc_preserve_allocation_truth() {
        let mut a = Accounting::new(4);
        a.allocate(10, 40, Some(7));
        a.reallocate(10, 0, 80, Some(7));
        assert_eq!(a.live_bytes, 40);
        a.reallocate(10, 10, 60, Some(7));
        assert_eq!(a.live_bytes, 60);
        a.reallocate(10, 20, 90, Some(8));
        assert_eq!(a.live_bytes, 90);
        assert_eq!(a.live.get(&20).unwrap().proc_id, 8);
        a.free(10);
        assert_eq!(a.live_bytes, 90);
        a.free(20);
        assert_eq!(a.live_bytes, 0);
        assert_eq!(a.peak_bytes, 90);
    }

    #[test]
    fn excludes_null_preexisting_and_unattributed_allocations() {
        let mut a = Accounting::new(4);
        a.allocate(0, 40, Some(1));
        a.allocate(10, 50, None);
        a.free(100);
        assert!(a.live.is_empty());
        a.reallocate(100, 101, 60, Some(1));
        assert_eq!(a.live_bytes, 60);
        a.reallocate(101, 0, 0, Some(1));
        assert_eq!(a.live_bytes, 0);
        a.reallocate(0, 22, 20, Some(1));
        assert_eq!(a.live_bytes, 20);
    }

    #[test]
    fn capacity_freezes_without_silently_dropping_records() {
        let mut a = Accounting::new(1);
        a.allocate(10, 40, Some(7));
        a.allocate(20, 50, Some(7));
        assert!(a.capacity_exceeded);
        assert_eq!(a.live.len(), 1);
        assert_eq!(a.live_bytes, 40);
        a.free(10);
        assert_eq!(a.live_bytes, 40);
    }
}
