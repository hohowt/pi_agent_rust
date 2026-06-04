use crate::Time;
use std::cell::RefCell;

#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub deadline: Option<Time>,
}

impl Budget {
    pub const INFINITE: Self = Self { deadline: None };

    #[must_use]
    pub const fn new() -> Self {
        Self::INFINITE
    }

    #[must_use]
    pub const fn with_deadline(mut self, deadline: Time) -> Self {
        self.deadline = Some(deadline);
        self
    }

    #[must_use]
    pub const fn with_poll_quota(self, _quota: u64) -> Self {
        self
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct Cx {
    budget: Budget,
}

thread_local! {
    static CURRENT_CX: RefCell<Option<Cx>> = const { RefCell::new(None) };
}

pub struct CurrentCxGuard {
    previous: Option<Cx>,
}

impl Drop for CurrentCxGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        CURRENT_CX.with(|slot| {
            *slot.borrow_mut() = previous;
        });
    }
}

impl Cx {
    #[must_use]
    pub fn current() -> Option<Self> {
        CURRENT_CX.with(|slot| slot.borrow().clone())
    }

    #[must_use]
    pub fn set_current(cx: Option<Self>) -> CurrentCxGuard {
        let previous = CURRENT_CX.with(|slot| slot.replace(cx));
        CurrentCxGuard { previous }
    }

    #[must_use]
    pub const fn for_request() -> Self {
        Self {
            budget: Budget::INFINITE,
        }
    }

    #[must_use]
    pub const fn for_request_with_budget(budget: Budget) -> Self {
        Self { budget }
    }

    #[must_use]
    pub const fn for_testing() -> Self {
        Self::for_request()
    }

    #[must_use]
    pub const fn for_testing_with_io() -> Self {
        Self::for_request()
    }

    #[must_use]
    pub const fn for_testing_with_budget(budget: Budget) -> Self {
        Self { budget }
    }

    #[must_use]
    pub const fn budget(&self) -> Budget {
        self.budget
    }

    #[must_use]
    pub const fn timer_driver(&self) -> Option<TimerDriver> {
        None
    }

    #[must_use]
    pub const fn checkpoint(&self) -> Result<(), std::convert::Infallible> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TimerDriver;

impl TimerDriver {
    #[must_use]
    pub fn now(&self) -> Time {
        crate::time::wall_now()
    }
}
