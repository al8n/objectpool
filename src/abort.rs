#[inline(never)]
#[cold]
pub(super) fn abort() -> ! {
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
