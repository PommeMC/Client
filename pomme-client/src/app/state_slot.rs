use scopeguard::defer_on_unwind;

pub struct StateSlot<T>(Option<T>);

impl<T> StateSlot<T> {
    pub const fn new(state: T) -> Self {
        Self(Some(state))
    }

    /// Transitions the state by applying `f` to the current state, and aborts
    /// if `f` panics to avoid UB.
    pub fn transition(&mut self, f: impl FnOnce(T) -> T) {
        // SAFETY: `self.0` is `Some` on entry by the type invariant. The transient
        // `None` during `f` is unobservable since we hold exclusive access via
        // `&mut self`.
        let state = unsafe { self.0.take().unwrap_unchecked() };

        defer_on_unwind!(std::process::abort());

        self.0 = Some(f(state));
    }

    pub const fn get(&self) -> &T {
        // SAFETY: all write paths (`new`, `transition`) restore `Some` or abort
        // before returning, and there is no safe constructor for the `None` variant.
        unsafe { self.0.as_ref().unwrap_unchecked() }
    }

    pub const fn get_mut(&mut self) -> &mut T {
        // SAFETY: same invariant as `get`.
        unsafe { self.0.as_mut().unwrap_unchecked() }
    }
}
