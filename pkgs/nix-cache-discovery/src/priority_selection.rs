use std::{collections::BTreeSet, sync::Arc};

use pingora_load_balancing::{selection::{BackendIter, BackendSelection}, Backend};


pub struct Priority{
    backends: Box<[Backend]>
}

impl BackendSelection for Priority{
    type Iter = PriorityIterator;

    fn build(backends: &BTreeSet<Backend>) -> Self {
        let mut backends = Vec::from_iter(backends.iter().cloned());
        backends.sort_by_key(|b| b.weight);
        let backends = backends.into_boxed_slice();
        Priority{
            backends
        }
    }

    fn iter(self: &Arc<Self>, key: &[u8]) -> Self::Iter {
        PriorityIterator::new(key, self.clone())
    }
}

pub struct PriorityIterator{
    index: u64,
    backend: Arc<Priority>,
}

impl PriorityIterator{
    fn new(_input: &[u8], backend: Arc<Priority>) -> Self {
        Self {
            index: 0,
            backend
        }
    }
}

impl BackendIter for PriorityIterator{
    fn next(&mut self) -> Option<&Backend> {
        if self.backend.backends.is_empty(){
            return None;
        }

        let index = self.index as usize % self.backend.backends.len();
        self.index += 1;
        Some(&self.backend.backends[index])
    }
}