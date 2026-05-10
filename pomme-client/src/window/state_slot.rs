pub struct StateSlot<T>(Option<T>);

impl<T> StateSlot<T> {
    pub const fn new(state: T) -> Self {
        Self(Some(state))
    }

    pub fn transition(&mut self, f: impl FnOnce(T) -> T) {
        // SAFETY: invariant guarantees this is always `Some`.
        let state = unsafe { self.0.take().unwrap_unchecked() };
        self.0 = Some(f(state));
    }

    pub const fn get(&self) -> &T {
        // SAFETY: invariant guarantees this is always `Some`.
        unsafe { self.0.as_ref().unwrap_unchecked() }
    }

    pub const fn get_mut(&mut self) -> &mut T {
        // SAFETY: invariant guarantees this is always `Some`.
        unsafe { self.0.as_mut().unwrap_unchecked() }
    }

    pub fn set(&mut self, state: T) {
        self.0 = Some(state);
    }
}
