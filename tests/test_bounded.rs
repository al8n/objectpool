use objectpool::Pool;

#[macro_use]
mod test_generic;

fn make_pool() -> Pool<u32> {
  Pool::<u32>::bounded(10, Default::default, |v| {
    *v = 0;
  })
}

#[cfg(not(feature = "loom"))]
fn make_recycle_pool() -> Pool<u32> {
  Pool::<u32>::bounded(10, Default::default, |_v| {})
}

test_generic_01!(test_bounded_01, make_pool());
test_generic_02!(test_bounded_02, make_pool());
#[cfg(not(feature = "loom"))]
test_recycle_generic_01!(test_bounded_recycle_01, make_recycle_pool());
