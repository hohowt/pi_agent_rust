use crate::{Cx, Time};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

#[derive(Debug)]
pub struct Mutex<T> {
    inner: Arc<tokio::sync::Mutex<T>>,
}

pub type MutexGuard<'a, T> = tokio::sync::MutexGuard<'a, T>;
pub type TryLockError = tokio::sync::TryLockError;
pub type Notify = tokio::sync::Notify;

#[derive(Debug)]
pub enum LockError {
    Cancelled,
    Poisoned,
    PolledAfterCompletion,
    TimedOut(Time),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "lock cancelled"),
            Self::Poisoned => write!(f, "lock poisoned"),
            Self::PolledAfterCompletion => write!(f, "lock polled after completion"),
            Self::TimedOut(_) => write!(f, "lock timed out"),
        }
    }
}

impl std::error::Error for LockError {}

impl<T> Mutex<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(value)),
        }
    }

    pub async fn lock(&self, _cx: &Cx) -> Result<MutexGuard<'_, T>, LockError> {
        Ok(self.inner.lock().await)
    }

    pub fn try_lock(&self) -> Result<MutexGuard<'_, T>, TryLockError> {
        self.inner.try_lock()
    }
}

pub async fn lock_owned<T>(mutex: Arc<Mutex<T>>, _cx: &Cx) -> Result<OwnedMutexGuard<T>, LockError>
where
    T: Send + 'static,
{
    Ok(OwnedMutexGuard {
        inner: mutex.inner.clone().lock_owned().await,
    })
}

pub struct OwnedMutexGuard<T> {
    inner: tokio::sync::OwnedMutexGuard<T>,
}

impl<T> OwnedMutexGuard<T> {
    pub async fn lock(mutex: Arc<Mutex<T>>, cx: &Cx) -> Result<Self, LockError>
    where
        T: Send + 'static,
    {
        lock_owned(mutex, cx).await
    }
}

impl<T> Deref for OwnedMutexGuard<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for OwnedMutexGuard<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
