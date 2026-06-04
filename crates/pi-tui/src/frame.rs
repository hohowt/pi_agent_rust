use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc};

pub const MIN_FRAME_INTERVAL: Duration = Duration::from_nanos(8_333_334);

#[derive(Debug, Default)]
pub struct FrameRateLimiter {
    last_emitted_at: Option<Instant>,
}

impl FrameRateLimiter {
    pub(crate) fn clamp_deadline(&self, requested: Instant) -> Instant {
        let Some(last_emitted_at) = self.last_emitted_at else {
            return requested;
        };
        requested.max(
            last_emitted_at
                .checked_add(MIN_FRAME_INTERVAL)
                .unwrap_or(last_emitted_at),
        )
    }

    pub(crate) const fn mark_emitted(&mut self, emitted_at: Instant) {
        self.last_emitted_at = Some(emitted_at);
    }
}

#[derive(Clone, Debug)]
pub struct FrameRequester {
    tx: mpsc::UnboundedSender<Instant>,
}

impl FrameRequester {
    pub(crate) fn new(draw_tx: broadcast::Sender<()>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let scheduler = FrameScheduler::new(rx, draw_tx);
        tokio::spawn(scheduler.run());
        Self { tx }
    }

    pub(crate) fn schedule_frame(&self) {
        let _ = self.tx.send(Instant::now());
    }
}

struct FrameScheduler {
    rx: mpsc::UnboundedReceiver<Instant>,
    draw_tx: broadcast::Sender<()>,
    limiter: FrameRateLimiter,
}

impl FrameScheduler {
    fn new(rx: mpsc::UnboundedReceiver<Instant>, draw_tx: broadcast::Sender<()>) -> Self {
        Self {
            rx,
            draw_tx,
            limiter: FrameRateLimiter::default(),
        }
    }

    async fn run(mut self) {
        const PARK_DURATION: Duration = Duration::from_secs(60 * 60 * 24 * 365);
        let mut next_deadline: Option<Instant> = None;
        loop {
            let target = next_deadline.unwrap_or_else(|| Instant::now() + PARK_DURATION);
            let sleep = tokio::time::sleep_until(target.into());
            tokio::pin!(sleep);

            tokio::select! {
                requested = self.rx.recv() => {
                    let Some(requested) = requested else {
                        break;
                    };
                    let requested = self.limiter.clamp_deadline(requested);
                    next_deadline = Some(next_deadline.map_or(requested, |current| current.min(requested)));
                }
                () = &mut sleep => {
                    if next_deadline.is_some() {
                        next_deadline = None;
                        self.limiter.mark_emitted(target);
                        let _ = self.draw_tx.send(());
                    }
                }
            }
        }
    }
}
