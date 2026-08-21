use std::mem::ManuallyDrop;

/// Completes storage release if user code unwinds across an endpoint cleanup boundary.
///
/// Event state transitions assign cleanup to one endpoint before payload or waker destructors run.
/// The owning endpoint creates this guard before user-controlled destruction. The guard performs
/// the release when its scope ends, whether normally or by unwinding.
pub(crate) struct ReleaseGuard<F>
where
    F: FnOnce(),
{
    release: ManuallyDrop<F>,
}

impl<F> ReleaseGuard<F>
where
    F: FnOnce(),
{
    pub(crate) fn new(release: F) -> Self {
        Self {
            release: ManuallyDrop::new(release),
        }
    }
}

impl<F> Drop for ReleaseGuard<F>
where
    F: FnOnce(),
{
    fn drop(&mut self) {
        // SAFETY: `new()` initializes the field, and `Drop::drop()` runs at most once. Taking it
        // here also prevents `ManuallyDrop` from running the closure destructor a second time.
        let release = unsafe { ManuallyDrop::take(&mut self.release) };
        release();
    }
}
