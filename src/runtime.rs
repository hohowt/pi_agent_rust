use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

pub mod reactor {
    pub const fn create_reactor() -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct RuntimeHandle {
    inner: tokio::runtime::Handle,
}

pub struct Runtime {
    inner: Option<tokio::runtime::Runtime>,
}

pub struct RuntimeBuilder {
    kind: RuntimeKind,
    worker_threads: Option<usize>,
    thread_name: Option<String>,
}

enum RuntimeKind {
    CurrentThread,
    MultiThread,
}

pub struct JoinHandle<T> {
    inner: tokio::task::JoinHandle<T>,
}

impl RuntimeBuilder {
    pub const fn new() -> Self {
        Self {
            kind: RuntimeKind::MultiThread,
            worker_threads: None,
            thread_name: None,
        }
    }

    pub const fn current_thread() -> Self {
        Self {
            kind: RuntimeKind::CurrentThread,
            worker_threads: None,
            thread_name: None,
        }
    }

    pub const fn multi_thread() -> Self {
        Self::new()
    }

    #[must_use]
    pub const fn worker_threads(mut self, threads: usize) -> Self {
        self.worker_threads = Some(threads);
        self
    }

    #[must_use]
    pub const fn blocking_threads(self, _min: usize, _max: usize) -> Self {
        self
    }

    #[must_use]
    pub const fn enable_parking(self, _enabled: bool) -> Self {
        self
    }

    #[must_use]
    pub fn thread_name(mut self, name: impl Into<String>) -> Self {
        self.thread_name = Some(name.into());
        self
    }

    #[must_use]
    pub const fn with_reactor(self, _reactor: ()) -> Self {
        self
    }

    pub fn build(self) -> std::io::Result<Runtime> {
        let mut builder = match self.kind {
            RuntimeKind::CurrentThread => tokio::runtime::Builder::new_current_thread(),
            RuntimeKind::MultiThread => {
                let mut builder = tokio::runtime::Builder::new_multi_thread();
                if let Some(threads) = self.worker_threads {
                    builder.worker_threads(threads);
                }
                builder
            }
        };
        builder.enable_all();
        if let Some(name) = self.thread_name {
            builder.thread_name(name);
        }
        builder.build().map(|inner| Runtime { inner: Some(inner) })
    }
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    pub fn handle(&self) -> RuntimeHandle {
        let runtime = self.inner.as_ref().expect("runtime already shut down");
        RuntimeHandle {
            inner: runtime.handle().clone(),
        }
    }

    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.inner
            .as_ref()
            .expect("runtime already shut down")
            .block_on(future)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(runtime) = self.inner.take() {
            runtime.shutdown_background();
        }
    }
}

impl RuntimeHandle {
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        JoinHandle {
            inner: self.inner.spawn(future),
        }
    }
}

impl<T> JoinHandle<T> {
    pub fn abort(&self) {
        self.inner.abort();
    }

    pub fn is_finished(&self) -> bool {
        self.inner.is_finished()
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.inner).poll(cx) {
            Poll::Ready(Ok(value)) => Poll::Ready(value),
            Poll::Ready(Err(err)) => {
                if err.is_panic() {
                    std::panic::resume_unwind(err.into_panic());
                }
                panic!("spawned task was cancelled");
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

pub async fn spawn_blocking<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|err| std::panic::resume_unwind(err.into_panic()))
}

pub async fn spawn_blocking_io<F, R>(f: F) -> std::io::Result<R>
where
    F: FnOnce() -> std::io::Result<R> + Send + 'static,
    R: Send + 'static,
{
    spawn_blocking(f).await
}
