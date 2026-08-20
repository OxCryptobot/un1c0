use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crossbeam_queue::ArrayQueue;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufferPoolConfigError {
    ZeroSlots,
    ZeroCapacity,
}

impl Display for BufferPoolConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroSlots => formatter.write_str("buffer pool slots must be greater than zero"),
            Self::ZeroCapacity => {
                formatter.write_str("buffer pool capacity must be greater than zero")
            }
        }
    }
}

impl std::error::Error for BufferPoolConfigError {}

#[derive(Debug, Default)]
struct BufferPoolCounters {
    checkouts: AtomicU64,
    reused: AtomicU64,
    fresh_allocations: AtomicU64,
    returns: AtomicU64,
    dropped_full: AtomicU64,
    dropped_oversize: AtomicU64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BufferPoolMetrics {
    pub capacity: usize,
    pub slots: usize,
    pub available: usize,
    pub checkouts: u64,
    pub reused: u64,
    pub fresh_allocations: u64,
    pub returns: u64,
    pub dropped_full: u64,
    pub dropped_oversize: u64,
}

#[derive(Debug, Clone)]
pub struct LockFreeBufferPool {
    queue: Arc<ArrayQueue<Vec<u8>>>,
    capacity: usize,
    slots: usize,
    counters: Arc<BufferPoolCounters>,
}

impl LockFreeBufferPool {
    pub fn new(capacity: usize, slots: usize) -> Result<Self, BufferPoolConfigError> {
        if capacity == 0 {
            return Err(BufferPoolConfigError::ZeroCapacity);
        }
        if slots == 0 {
            return Err(BufferPoolConfigError::ZeroSlots);
        }
        Ok(Self {
            queue: Arc::new(ArrayQueue::new(slots)),
            capacity,
            slots,
            counters: Arc::new(BufferPoolCounters::default()),
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn slots(&self) -> usize {
        self.slots
    }

    pub fn checkout(&self) -> PooledBuffer {
        self.counters.checkouts.fetch_add(1, Ordering::Relaxed);
        match self.queue.pop() {
            Some(mut buffer) => {
                buffer.clear();
                self.counters.reused.fetch_add(1, Ordering::Relaxed);
                PooledBuffer {
                    pool: Some(self.clone()),
                    buffer,
                }
            }
            None => {
                self.counters
                    .fresh_allocations
                    .fetch_add(1, Ordering::Relaxed);
                PooledBuffer {
                    pool: Some(self.clone()),
                    buffer: Vec::with_capacity(self.capacity),
                }
            }
        }
    }

    pub fn metrics(&self) -> BufferPoolMetrics {
        BufferPoolMetrics {
            capacity: self.capacity,
            slots: self.slots,
            available: self.queue.len(),
            checkouts: self.counters.checkouts.load(Ordering::Relaxed),
            reused: self.counters.reused.load(Ordering::Relaxed),
            fresh_allocations: self.counters.fresh_allocations.load(Ordering::Relaxed),
            returns: self.counters.returns.load(Ordering::Relaxed),
            dropped_full: self.counters.dropped_full.load(Ordering::Relaxed),
            dropped_oversize: self.counters.dropped_oversize.load(Ordering::Relaxed),
        }
    }

    fn recycle(&self, mut buffer: Vec<u8>) {
        if buffer.capacity() > self.capacity {
            self.counters
                .dropped_oversize
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        buffer.clear();
        match self.queue.push(buffer) {
            Ok(()) => {
                self.counters.returns.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.counters.dropped_full.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

pub struct PooledBuffer {
    pool: Option<LockFreeBufferPool>,
    buffer: Vec<u8>,
}

impl PooledBuffer {
    pub fn as_slice(&self) -> &[u8] {
        &self.buffer
    }

    pub fn as_mut_vec(&mut self) -> &mut Vec<u8> {
        &mut self.buffer
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn extend_from_slice(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    pub fn into_vec(mut self) -> Vec<u8> {
        self.pool = None;
        std::mem::take(&mut self.buffer)
    }
}

impl std::fmt::Debug for PooledBuffer {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PooledBuffer")
            .field("len", &self.buffer.len())
            .field("capacity", &self.buffer.capacity())
            .finish()
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.take() {
            pool.recycle(std::mem::take(&mut self.buffer));
        }
    }
}
