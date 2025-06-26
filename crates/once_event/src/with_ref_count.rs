/// Just combines a value and a reference count, for use in custom reference counting logic.
#[derive(Debug)]
pub(crate) struct WithRefCount<T> {
    value: T,
    ref_count: usize,
}

impl<T> WithRefCount<T> {
    pub(crate) fn new(value: T) -> Self {
        Self {
            value,
            ref_count: 0,
        }
    }

    pub(crate) fn get(&self) -> &T {
        &self.value
    }

    #[expect(dead_code, reason = "May be useful for future extensions")]
    pub(crate) fn get_mut(&mut self) -> &mut T {
        &mut self.value
    }

    pub(crate) fn inc_ref(&mut self) {
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "Reference counting increment cannot overflow in practice"
        )]
        {
            self.ref_count += 1;
        }
    }

    pub(crate) fn dec_ref(&mut self) {
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "Reference counting decrement cannot underflow as it is only called when references exist"
        )]
        {
            self.ref_count -= 1;
        }
    }

    pub(crate) fn ref_count(&self) -> usize {
        self.ref_count
    }

    pub(crate) fn is_referenced(&self) -> bool {
        self.ref_count > 0
    }
}

impl<T> Default for WithRefCount<T>
where
    T: Default,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}
