//! Preallocated side metadata for the Lisp-2 collector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Relocation {
    pub old_offset: usize,
    pub new_offset: usize,
    pub size: usize,
}

/// Reserve at initialization, never during collection. Entries are sorted by
/// old offset; descriptors remain intact. Mark before enqueueing an object.
pub(crate) struct Scratch {
    pub worklist: Vec<usize>,
    pub relocations: Vec<Relocation>,
}

impl Scratch {
    pub fn new(heap_bytes: usize) -> Result<Self, std::collections::TryReserveError> {
        let count = heap_bytes / core::mem::size_of::<crate::object::Object>();
        let mut worklist = Vec::new();
        let mut relocations = Vec::new();
        worklist.try_reserve_exact(count)?;
        relocations.try_reserve_exact(count)?;
        Ok(Self {
            worklist,
            relocations,
        })
    }

    pub fn destination(&self, old_offset: usize) -> Option<usize> {
        self.relocations
            .binary_search_by_key(&old_offset, |r| r.old_offset)
            .ok()
            .map(|i| self.relocations[i].new_offset)
    }

    pub fn clear(&mut self) {
        self.worklist.clear();
        self.relocations.clear();
    }

    pub fn push_work(&mut self, offset: usize) {
        if self.worklist.len() == self.worklist.capacity() {
            crate::heap_layout::oom();
        }
        self.worklist.push(offset);
    }

    pub fn push_relocation(&mut self, relocation: Relocation) {
        if self.relocations.len() == self.relocations.capacity() {
            crate::heap_layout::oom();
        }
        self.relocations.push(relocation);
    }
}

static mut SCRATCH: Option<Scratch> = None;

pub(crate) fn init(heap_bytes: usize) {
    shutdown();
    let scratch = Scratch::new(heap_bytes).unwrap_or_else(|_| crate::heap_layout::oom());
    unsafe { SCRATCH = Some(scratch) };
}

pub(crate) fn shutdown() {
    unsafe {
        SCRATCH = None;
    }
}

/// # Safety
/// The runtime is single-threaded and collection is guarded against re-entry.
pub(crate) unsafe fn scratch() -> &'static mut Scratch {
    // Collection has exclusive access; initialization/shutdown cannot overlap.
    (&mut *core::ptr::addr_of_mut!(SCRATCH))
        .as_mut()
        .expect("collector scratch not initialized")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forwarding_requires_an_object_start() {
        let mut scratch = Scratch::new(128).unwrap();
        scratch.relocations.push(Relocation {
            old_offset: 32,
            new_offset: 0,
            size: 16,
        });
        assert_eq!(scratch.destination(32), Some(0));
        assert_eq!(scratch.destination(33), None);
        assert!(scratch.worklist.capacity() >= 128 / core::mem::size_of::<crate::object::Object>());
    }
}
