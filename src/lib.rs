//! Yet another lock-free object pool.
#![cfg_attr(not(any(feature = "std", test)), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, allow(unused_attributes))]
#![deny(missing_docs)]

#[cfg(not(feature = "std"))]
extern crate alloc as std;

#[cfg(feature = "std")]
extern crate std;

#[cfg(not(any(feature = "std", feature = "alloc")))]
compile_error!("`objectpool` requires either the 'std' or 'alloc' feature to be enabled.");

use core::{mem::ManuallyDrop, ptr::NonNull};

use crossbeam_queue::{ArrayQueue, SegQueue};

#[cfg(not(feature = "loom"))]
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

#[cfg(feature = "loom")]
use loom::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

#[cfg(not(feature = "std"))]
use std::boxed::Box;

/// A reusable `T`.
pub struct ReusableObject<T> {
  pool: Pool<T>,
  obj: ManuallyDrop<T>,
}

impl<T> AsRef<T> for ReusableObject<T> {
  fn as_ref(&self) -> &T {
    &self.obj
  }
}

impl<T> AsMut<T> for ReusableObject<T> {
  fn as_mut(&mut self) -> &mut T {
    &mut self.obj
  }
}

impl<T> core::ops::Deref for ReusableObject<T> {
  type Target = T;

  fn deref(&self) -> &Self::Target {
    &self.obj
  }
}

impl<T> core::ops::DerefMut for ReusableObject<T> {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.obj
  }
}

impl<T> Drop for ReusableObject<T> {
  fn drop(&mut self) {
    // SAFETY: The object is dropped, we never reuse the ManuallyDrop again.
    unsafe {
      self.pool.attach(ManuallyDrop::take(&mut self.obj));
    }
  }
}

// It is ok to have a large enum variant here because the enum will always be `Box::into_raw(Box::new(_))`
#[allow(clippy::large_enum_variant)]
enum Backed<T> {
  Bounded(ArrayQueue<T>),
  Unbounded(SegQueue<T>),
}

struct Queue<T> {
  refs: AtomicUsize,
  queue: Backed<T>,
}

impl<T> Queue<T> {
  #[inline]
  fn bounded(queue: ArrayQueue<T>) -> Self {
    Self {
      refs: AtomicUsize::new(1),
      queue: Backed::Bounded(queue),
    }
  }

  #[inline]
  fn unbounded(queue: SegQueue<T>) -> Self {
    Self {
      refs: AtomicUsize::new(1),
      queue: Backed::Unbounded(queue),
    }
  }

  #[inline]
  fn push(&self, obj: T) {
    match &self.queue {
      Backed::Bounded(queue) => {
        let _ = queue.push(obj);
      }
      Backed::Unbounded(queue) => queue.push(obj),
    }
  }

  #[inline]
  fn pop(&self) -> Option<T> {
    match &self.queue {
      Backed::Bounded(queue) => queue.pop(),
      Backed::Unbounded(queue) => queue.pop(),
    }
  }
}

/// Lock-free object pool.
pub struct Pool<T> {
  refs: AtomicPtr<()>,
  queue: *mut Queue<T>,
  new: NonNull<dyn Fn() -> T + Send + Sync + 'static>,
  reset: NonNull<dyn Fn(&mut T) + Send + Sync + 'static>,
}

unsafe impl<T: Send> Send for Pool<T> {}
unsafe impl<T: Sync> Sync for Pool<T> {}

impl<T> Pool<T> {
  /// Create a new pool with the given capacity.
  ///
  /// # Example
  ///
  /// ```rust
  /// use objectpool::Pool;
  ///
  /// let pool = Pool::<u32>::bounded(10, Default::default, |_v| {});
  /// ```
  #[inline]
  pub fn bounded(
    capacity: usize,
    new: impl Fn() -> T + Send + Sync + 'static,
    reset: impl Fn(&mut T) + Send + Sync + 'static,
  ) -> Self {
    let queue = Queue::bounded(ArrayQueue::<T>::new(capacity));
    Self::new(queue, new, reset)
  }

  /// Create a new pool with the unbounded capacity.
  ///
  /// # Example
  ///
  /// ```rust
  /// use objectpool::Pool;
  ///
  /// let pool = Pool::<u32>::unbounded(Default::default, |_v| {});
  /// ```
  #[inline]
  pub fn unbounded(
    new: impl Fn() -> T + Send + Sync + 'static,
    reset: impl Fn(&mut T) + Send + Sync + 'static,
  ) -> Self {
    let queue = Queue::unbounded(SegQueue::<T>::new());
    Self::new(queue, new, reset)
  }

  /// Get an object from the pool.
  ///
  /// # Example
  ///
  /// ```rust
  /// use objectpool::Pool;
  ///
  /// let pool = Pool::<u32>::bounded(10, Default::default, |_v| {});
  ///
  /// let mut obj = pool.get();
  ///
  /// assert_eq!(*obj, 0);
  ///
  /// *obj = 42;
  /// drop(obj);
  /// ```
  #[inline]
  pub fn get(&self) -> ReusableObject<T> {
    ReusableObject {
      pool: self.clone(),
      obj: ManuallyDrop::new(self.queue().pop().unwrap_or_else(|| self.new_object())),
    }
  }

  /// Get an object from the pool with a fallback.
  ///
  /// # Example
  ///
  /// ```rust
  /// use objectpool::Pool;
  ///
  /// let pool = Pool::<u32>::bounded(10, Default::default, |_| {});
  ///
  /// let mut obj = pool.get_or_else(|| 42);
  ///
  /// assert_eq!(*obj, 42);
  /// ```
  #[inline]
  pub fn get_or_else(&self, fallback: impl Fn() -> T) -> ReusableObject<T> {
    ReusableObject {
      pool: self.clone(),
      obj: ManuallyDrop::new(self.queue().pop().unwrap_or_else(fallback)),
    }
  }

  /// Clear the pool.
  ///
  /// # Example
  ///
  /// ```rust
  /// use objectpool::Pool;
  ///
  /// let pool = Pool::<u32>::bounded(10, Default::default, |v| {});
  ///
  /// let mut obj = pool.get();
  /// *obj = 42;
  /// drop(obj);
  ///
  /// pool.clear();
  /// ```
  #[inline]
  pub fn clear(&self) {
    while self.queue().pop().is_some() {}
  }

  #[inline]
  fn new(
    queue: Queue<T>,
    new: impl Fn() -> T + Send + Sync + 'static,
    reset: impl Fn(&mut T) + Send + Sync + 'static,
  ) -> Self {
    let ptr = Box::into_raw(Box::new(queue));

    unsafe {
      Self {
        queue: ptr,
        refs: AtomicPtr::new(ptr as *mut ()),
        // SAFETY: Box::new is safe because the closure is 'static.
        new: NonNull::new_unchecked(Box::into_raw(Box::new(new))),
        // SAFETY: Box::new is safe because the closure is 'static.
        reset: NonNull::new_unchecked(Box::into_raw(Box::new(reset))),
      }
    }
  }

  /// Return an object to the pool.
  #[inline]
  fn attach(&self, mut obj: T) {
    self.reset_object(&mut obj);
    self.queue().push(obj);
  }

  #[inline]
  fn new_object(&self) -> T {
    // SAFETY: The new closure is 'static and the pointer is valid until the last Pool instance is droped.
    let constructor = unsafe { &*(self.new.as_ptr()) };
    constructor()
  }

  #[inline]
  fn reset_object(&self, obj: &mut T) {
    // SAFETY: The reset closure is 'static and the pointer is valid until the last Pool instance is droped.
    let resetter = unsafe { &*(self.reset.as_ptr()) };
    resetter(obj);
  }

  #[inline]
  fn queue(&self) -> &Queue<T> {
    // SAFETY: The pointer is valid until the last Pool instance is droped.
    unsafe { &*self.queue }
  }
}

impl<T> Clone for Pool<T> {
  fn clone(&self) -> Self {
    unsafe {
      let shared: *mut Queue<T> = self.refs.load(Ordering::Relaxed).cast();

      let old_size = (*shared).refs.fetch_add(1, Ordering::Release);
      if old_size > usize::MAX >> 1 {
        abort();
      }

      // SAFETY: The ptr is always non-null, and the data is only deallocated when the
      // last Pool is dropped.
      Self {
        refs: AtomicPtr::new(shared as *mut ()),
        queue: self.queue,
        new: self.new,
        reset: self.reset,
      }
    }
  }
}

impl<T> Drop for Pool<T> {
  fn drop(&mut self) {
    unsafe {
      self.refs.with_mut(|shared| {
        let shared: *mut Queue<T> = shared.cast();
        // `Shared` storage... follow the drop steps from Arc.
        if (*shared).refs.fetch_sub(1, Ordering::Release) != 1 {
          return;
        }

        // This fence is needed to prevent reordering of use of the data and
        // deletion of the data.  Because it is marked `Release`, the decreasing
        // of the reference count synchronizes with this `Acquire` fence. This
        // means that use of the data happens before decreasing the reference
        // count, which happens before this fence, which happens before the
        // deletion of the data.
        //
        // As explained in the [Boost documentation][1],
        //
        // > It is important to enforce any possible access to the object in one
        // > thread (through an existing reference) to *happen before* deleting
        // > the object in a different thread. This is achieved by a "release"
        // > operation after dropping a reference (any access to the object
        // > through this reference must obviously happened before), and an
        // > "acquire" operation before deleting the object.
        //
        // [1]: (www.boost.org/doc/libs/1_55_0/doc/html/atomic/usage_examples.html)
        //
        // Thread sanitizer does not support atomic fences. Use an atomic load
        // instead.
        (*shared).refs.load(Ordering::Acquire);

        // Drop the data
        let _ = Box::from_raw(shared);
        let _ = Box::from_raw(self.new.as_ptr());
        let _ = Box::from_raw(self.reset.as_ptr());
      });
    }
  }
}

#[cfg(not(feature = "loom"))]
trait AtomicMut<T> {
  fn with_mut<F, R>(&mut self, f: F) -> R
  where
    F: FnOnce(&mut *mut T) -> R;
}

#[cfg(not(feature = "loom"))]
impl<T> AtomicMut<T> for AtomicPtr<T> {
  fn with_mut<F, R>(&mut self, f: F) -> R
  where
    F: FnOnce(&mut *mut T) -> R,
  {
    f(self.get_mut())
  }
}

#[inline(never)]
#[cold]
fn abort() -> ! {
  #[cfg(feature = "std")]
  {
    std::process::abort()
  }

  #[cfg(not(feature = "std"))]
  {
    struct Abort;
    impl Drop for Abort {
      fn drop(&mut self) {
        panic!();
      }
    }
    let _a = Abort;
    panic!("abort");
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[cfg(not(feature = "std"))]
  use std::{vec, vec::Vec};

  #[cfg(all(feature = "std", not(feature = "loom")))]
  use std::thread;

  #[cfg(all(feature = "std", feature = "loom", not(miri)))]
  use loom::thread;

  fn create_pool(cap: usize) -> Pool<Vec<u8>> {
    Pool::bounded(cap, Vec::new, |val| {
      val.clear();
    })
  }

  fn basic_get_and_put_in() {
    let pool = create_pool(10);

    // Get a new object from the pool
    let mut obj = pool.get();
    assert_eq!(*obj, Vec::new());

    // Modify and return the object
    obj.push(42);
    drop(obj);

    // Get the object back from the pool
    let obj = pool.get();
    assert_eq!(*obj, Vec::new());
  }

  #[test]
  fn basic_get_and_put() {
    #[cfg(feature = "loom")]
    loom::model(basic_get_and_put_in);

    #[cfg(not(feature = "loom"))]
    basic_get_and_put_in();
  }

  fn get_or_else_in() {
    let pool = create_pool(10);

    // Get an object from the pool with a fallback
    let mut obj = pool.get_or_else(|| vec![42]);
    assert_eq!(*obj, [42]);

    // Modify and return the object
    obj.push(43);
    drop(obj);

    // Get the object back from the pool with a fallback
    let obj = pool.get_or_else(|| vec![42]);
    assert_eq!(*obj, []);

    let _objs = (0..10)
      .map(|_| pool.get_or_else(|| vec![42]))
      .collect::<Vec<_>>();

    let obj = pool.get_or_else(|| vec![42]);
    assert_eq!(*obj, [42]);
  }

  #[test]
  fn get_or_else() {
    #[cfg(feature = "loom")]
    loom::model(get_or_else_in);

    #[cfg(not(feature = "loom"))]
    get_or_else_in();
  }

  fn pool_clone_in() {
    let pool = create_pool(10);
    let pool_clone = pool.clone();

    // Get an object from the cloned pool
    let mut obj = pool_clone.get();
    assert_eq!(*obj, []);

    // Modify and return the object
    obj.push(42);
    drop(obj);

    // Get the object back from the original pool
    let obj = pool.get();
    assert_eq!(*obj, []);
  }

  #[test]
  fn pool_clone() {
    #[cfg(feature = "loom")]
    loom::model(pool_clone_in);

    #[cfg(not(feature = "loom"))]
    pool_clone_in();
  }

  #[cfg(feature = "std")]
  fn multi_threaded_access_in() {
    #[cfg(not(feature = "loom"))]
    const OUTER: usize = 10;
    #[cfg(feature = "loom")]
    const OUTER: usize = 1;

    #[cfg(not(feature = "loom"))]
    const INNER: usize = 100;
    #[cfg(feature = "loom")]
    const INNER: usize = 10;

    let pool = create_pool(10);

    let mut handles = vec![];

    for _ in 0..OUTER {
      let pool = pool.clone();
      let handle = thread::spawn(move || {
        for i in 0..INNER {
          let mut obj = pool.get();
          obj.push(i as u8);
          drop(obj);
        }
      });
      handles.push(handle);
    }

    for handle in handles {
      handle.join().expect("Thread panicked");
    }

    // Check that the pool is still functional after multi-threaded access
    let obj = pool.get();
    assert_eq!(*obj, []);
  }

  #[test]
  #[cfg(feature = "std")]
  fn multi_threaded_access() {
    #[cfg(all(feature = "std", not(feature = "loom")))]
    multi_threaded_access_in();

    #[cfg(all(feature = "std", feature = "loom"))]
    loom::model(multi_threaded_access_in);
  }

  fn custom_new_and_reset_in() {
    let pool = Pool::bounded(
      10,
      || 100, // new closure that creates an i32 with value 100
      |val: &mut i32| {
        *val = 200;
      }, // reset closure that resets the value to 200
    );

    // Get a new object from the pool
    let mut obj = pool.get();
    assert_eq!(*obj, 100);

    // Modify and return the object
    *obj = 42;
    drop(obj);

    // Get the object back from the pool
    let obj = pool.get();
    assert_eq!(*obj, 200);
  }

  #[test]
  fn custom_new_and_reset() {
    #[cfg(feature = "loom")]
    loom::model(custom_new_and_reset_in);

    #[cfg(not(feature = "loom"))]
    custom_new_and_reset_in();
  }

  #[cfg(not(feature = "loom"))]
  fn stress_test_in() {
    let pool = create_pool(10);

    for _ in 0..1_000_000 {
      let mut obj = pool.get();
      obj.push(42);
    }

    // Check that the pool is still functional after stress test
    let obj = pool.get();
    assert_eq!(*obj, []);
  }

  #[test]
  #[cfg(not(feature = "loom"))]
  fn stress_test() {
    stress_test_in();
  }

  fn test_reusable_object_in() {
    let pool = create_pool(10);

    {
      let mut obj = pool.get();
      obj.push(42);
      assert_eq!(*obj, [42]);
      // obj goes out of scope and is returned to the pool
    }

    // Get the object back from the pool
    let obj = pool.get();
    assert_eq!(*obj, []);
  }

  #[test]
  fn test_reusable_object() {
    #[cfg(feature = "loom")]
    loom::model(test_reusable_object_in);

    #[cfg(not(feature = "loom"))]
    test_reusable_object_in();
  }

  fn test_reset_on_put_in() {
    let pool = create_pool(10);

    let mut obj = pool.get();
    obj.push(123);
    drop(obj); // Object is returned to the pool and reset

    // Get the object back from the pool
    let obj = pool.get();
    assert_eq!(*obj, []); // Ensure that the object was reset
  }

  #[test]
  fn test_reset_on_put() {
    #[cfg(feature = "loom")]
    loom::model(test_reset_on_put_in);

    #[cfg(not(feature = "loom"))]
    test_reset_on_put_in();
  }
}
