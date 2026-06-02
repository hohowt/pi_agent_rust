use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Time(Instant);

impl Time {
    pub fn duration_since(self, earlier: Self) -> u64 {
        u64::try_from(self.0.duration_since(earlier.0).as_nanos()).unwrap_or(u64::MAX)
    }
}

impl std::ops::Add<Duration> for Time {
    type Output = Self;

    fn add(self, rhs: Duration) -> Self::Output {
        Self(self.0 + rhs)
    }
}

pub fn wall_now() -> Time {
    Time(Instant::now())
}

pub async fn sleep(_start: Time, duration: Duration) {
    tokio::time::sleep(duration).await;
}

pub async fn timeout<T>(
    _start: Time,
    duration: Duration,
    future: impl std::future::Future<Output = T>,
) -> Result<T, tokio::time::error::Elapsed> {
    tokio::time::timeout(duration, future).await
}
