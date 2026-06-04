use std::pin::Pin;
use std::task::{Context, Poll};

use crossterm::event::{Event, EventStream, KeyEvent};
use tokio::sync::broadcast;
use tokio_stream::Stream;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

#[derive(Debug)]
pub enum RatatuiEvent {
    Key(KeyEvent),
    Paste(String),
    Resize,
    Draw,
    FocusGained,
}

pub struct RatatuiEventStream {
    input: EventStream,
    draw: BroadcastStream<()>,
    poll_draw_first: bool,
}

impl RatatuiEventStream {
    pub(crate) fn new(draw_rx: broadcast::Receiver<()>) -> Self {
        Self {
            input: EventStream::new(),
            draw: BroadcastStream::new(draw_rx),
            poll_draw_first: false,
        }
    }

    fn poll_input(&mut self, cx: &mut Context<'_>) -> Poll<Option<RatatuiEvent>> {
        loop {
            let event = match Pin::new(&mut self.input).poll_next(cx) {
                Poll::Ready(Some(Ok(event))) => event,
                Poll::Ready(Some(Err(_))) => continue,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            };

            if let Some(mapped) = map_crossterm_event(event) {
                return Poll::Ready(Some(mapped));
            }
        }
    }

    fn poll_draw(&mut self, cx: &mut Context<'_>) -> Poll<Option<RatatuiEvent>> {
        match Pin::new(&mut self.draw).poll_next(cx) {
            Poll::Ready(Some(Ok(()) | Err(BroadcastStreamRecvError::Lagged(_)))) => {
                Poll::Ready(Some(RatatuiEvent::Draw))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Stream for RatatuiEventStream {
    type Item = RatatuiEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let draw_first = self.poll_draw_first;
        self.poll_draw_first = !self.poll_draw_first;

        if draw_first {
            if let Poll::Ready(event) = self.poll_draw(cx) {
                return Poll::Ready(event);
            }
            if let Poll::Ready(event) = self.poll_input(cx) {
                return Poll::Ready(event);
            }
        } else {
            if let Poll::Ready(event) = self.poll_input(cx) {
                return Poll::Ready(event);
            }
            if let Poll::Ready(event) = self.poll_draw(cx) {
                return Poll::Ready(event);
            }
        }

        Poll::Pending
    }
}

fn map_crossterm_event(event: Event) -> Option<RatatuiEvent> {
    match event {
        Event::Key(key) => Some(RatatuiEvent::Key(key)),
        Event::Resize(_, _) => Some(RatatuiEvent::Resize),
        Event::Paste(text) => Some(RatatuiEvent::Paste(text)),
        Event::FocusGained => Some(RatatuiEvent::FocusGained),
        Event::FocusLost | Event::Mouse(_) => None,
    }
}
