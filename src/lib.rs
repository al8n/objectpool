//! Yet another lock-free object pool. 
#![cfg_attr(not(any(feature = "std", test)), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, allow(unused_attributes))]
#![deny(missing_docs)]

extern crate alloc as std;

use crossbeam_queue::ArrayQueue;

/// Lock-free object pool.
pub struct Pool<T> {
  queue: ArrayQueue<T>,
  new: Box<dyn Fn() -> T + 'static>,
  reset: Box<dyn Fn(&mut T) + 'static>,
}

impl<T> Pool<T> {
  /// Create a new pool with the given capacity.
  #[inline]
  pub fn new(capacity: usize, new: impl Fn() -> T + 'static, reset: impl Fn(&mut T) + 'static) -> Self {
    Self {
      queue: ArrayQueue::new(capacity),
      new: Box::new(new),
      reset: Box::new(reset),
    }
  }

  /// Get an object from the pool.
  #[inline]
  pub fn get(&self) -> Option<T> {
    self.queue.pop().or_else(|| Some((self.new)()))
  }

  /// Get an object from the pool with a fallback.
  #[inline]
  pub fn get_or_else(&self, fallback: impl Fn() -> T) -> T {
    self.queue.pop().unwrap_or_else(fallback)
  }

  /// Return an object to the pool.
  #[inline]
  pub fn put(&self, mut obj: T) {
    (self.reset)(&mut obj);
    let _ = self.queue.push(obj);
  }
}
