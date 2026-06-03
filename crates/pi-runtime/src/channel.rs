pub mod oneshot {
    use crate::Cx;

    pub struct Sender<T>(Option<tokio::sync::oneshot::Sender<T>>);
    pub struct Receiver<T>(tokio::sync::oneshot::Receiver<T>);

    #[must_use]
    pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (Sender(Some(tx)), Receiver(rx))
    }

    impl<T> Sender<T> {
        pub fn send(mut self, _cx: &Cx, value: T) -> Result<(), T> {
            self.0.take().expect("oneshot sender present").send(value)
        }
    }

    impl<T> Receiver<T> {
        pub async fn recv(
            &mut self,
            _cx: &Cx,
        ) -> Result<T, tokio::sync::oneshot::error::RecvError> {
            (&mut self.0).await
        }

        pub fn try_recv(&mut self) -> Result<T, tokio::sync::oneshot::error::TryRecvError> {
            self.0.try_recv()
        }
    }
}

pub mod mpsc {
    use crate::Cx;

    pub struct Sender<T> {
        inner: tokio::sync::mpsc::Sender<T>,
    }

    pub struct Receiver<T> {
        inner: tokio::sync::mpsc::Receiver<T>,
    }

    pub enum SendError<T> {
        Disconnected(T),
        Cancelled(T),
        Full(T),
    }

    #[must_use]
    pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
        let (tx, rx) = tokio::sync::mpsc::channel(capacity);
        (Sender { inner: tx }, Receiver { inner: rx })
    }

    impl<T> Clone for Sender<T> {
        fn clone(&self) -> Self {
            Self {
                inner: self.inner.clone(),
            }
        }
    }

    impl<T> Sender<T> {
        pub async fn send(&self, _cx: &Cx, value: T) -> Result<(), SendError<T>> {
            self.inner
                .send(value)
                .await
                .map_err(|err| SendError::Disconnected(err.0))
        }

        pub fn try_send(&self, value: T) -> Result<(), SendError<T>> {
            self.inner.try_send(value).map_err(|err| match err {
                tokio::sync::mpsc::error::TrySendError::Full(value) => SendError::Full(value),
                tokio::sync::mpsc::error::TrySendError::Closed(value) => {
                    SendError::Disconnected(value)
                }
            })
        }
    }

    impl<T> Receiver<T> {
        pub async fn recv(&mut self, _cx: &Cx) -> Result<T, ()> {
            self.inner.recv().await.ok_or(())
        }

        pub fn try_recv(&mut self) -> Result<T, tokio::sync::mpsc::error::TryRecvError> {
            self.inner.try_recv()
        }
    }
}
