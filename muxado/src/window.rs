use std::cmp;

#[derive(Debug)]
pub struct Window {
    max_size: usize,
    size: usize,
}

impl Window {
    pub(crate) fn new(capacity: usize) -> Self {
        Window {
            max_size: capacity,
            size: capacity,
        }
    }
    pub fn inc(&mut self, by: usize) {
        if by == 0 {
            return;
        }

        self.size = cmp::min(self.size + by, self.max_size);
    }

    pub fn capacity(&self) -> usize {
        self.size
    }

    pub fn dec(&mut self, by: usize) -> usize {
        let actual = cmp::min(self.size, by);

        self.size -= actual;

        actual
    }
}
