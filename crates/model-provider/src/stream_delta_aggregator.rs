use crate::DsmlStreamDeltaFilter;
use std::time::{Duration, Instant};

const STREAM_DELTA_BATCH_INTERVAL: Duration = Duration::from_millis(40);
const STREAM_DELTA_MAX_PENDING_BYTES: usize = 4 * 1024;

pub(super) struct FilteredStreamDeltaEmitter<'a, F: FnMut(&str) + ?Sized> {
    filter: DsmlStreamDeltaFilter,
    aggregator: StreamingDeltaAggregator<'a, F>,
}

impl<'a, F: FnMut(&str) + ?Sized> FilteredStreamDeltaEmitter<'a, F> {
    pub(super) fn new(on_delta: &'a mut F) -> Self {
        Self {
            filter: DsmlStreamDeltaFilter::default(),
            aggregator: StreamingDeltaAggregator::new(on_delta),
        }
    }

    pub(super) fn push(&mut self, delta: &str, now: Instant) {
        self.filter
            .push(delta, &mut |visible| self.aggregator.push(visible, now));
    }

    pub(super) fn before_poll(&mut self, now: Instant, default: Duration) -> Duration {
        self.aggregator.before_poll(now, default)
    }

    pub(super) fn finish(&mut self, now: Instant) {
        self.filter
            .finish(&mut |visible| self.aggregator.push(visible, now));
        self.aggregator.finish();
    }
}

struct StreamingDeltaAggregator<'a, F: FnMut(&str) + ?Sized> {
    on_delta: &'a mut F,
    pending: String,
    pending_since: Option<Instant>,
    emitted_first: bool,
    batch_interval: Duration,
    max_pending_bytes: usize,
}

impl<'a, F: FnMut(&str) + ?Sized> StreamingDeltaAggregator<'a, F> {
    fn new(on_delta: &'a mut F) -> Self {
        Self::with_policy(
            on_delta,
            STREAM_DELTA_BATCH_INTERVAL,
            STREAM_DELTA_MAX_PENDING_BYTES,
        )
    }

    fn with_policy(
        on_delta: &'a mut F,
        batch_interval: Duration,
        max_pending_bytes: usize,
    ) -> Self {
        Self {
            on_delta,
            pending: String::new(),
            pending_since: None,
            emitted_first: false,
            batch_interval,
            max_pending_bytes: max_pending_bytes.max(1),
        }
    }

    fn push(&mut self, delta: &str, now: Instant) {
        if delta.is_empty() {
            return;
        }
        if !self.emitted_first {
            self.emitted_first = true;
            (self.on_delta)(delta);
            return;
        }

        self.flush_if_due(now);
        if delta.len() >= self.max_pending_bytes {
            self.flush();
            (self.on_delta)(delta);
            return;
        }
        if !self.pending.is_empty()
            && self.pending.len().saturating_add(delta.len()) > self.max_pending_bytes
        {
            self.flush();
        }
        if self.pending.is_empty() {
            self.pending_since = Some(now);
        }
        self.pending.push_str(delta);
        if self.pending.len() >= self.max_pending_bytes {
            self.flush();
        }
    }

    fn flush_if_due(&mut self, now: Instant) {
        if self
            .pending_since
            .is_some_and(|started| now.saturating_duration_since(started) >= self.batch_interval)
        {
            self.flush();
        }
    }

    fn before_poll(&mut self, now: Instant, default: Duration) -> Duration {
        self.flush_if_due(now);
        self.pending_since.map_or(default, |started| {
            default.min(
                self.batch_interval
                    .saturating_sub(now.saturating_duration_since(started)),
            )
        })
    }

    fn finish(&mut self) {
        self.flush();
    }

    fn flush(&mut self) {
        if self.pending.is_empty() {
            self.pending_since = None;
            return;
        }
        let mut pending = std::mem::take(&mut self.pending);
        self.pending_since = None;
        (self.on_delta)(&pending);
        pending.clear();
        self.pending = pending;
    }
}

impl<F: FnMut(&str) + ?Sized> Drop for StreamingDeltaAggregator<'_, F> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn first_delta_is_immediate_and_deadline_is_not_debounced() {
        let now = Instant::now();
        let emitted = RefCell::new(Vec::new());
        let mut callback = |delta: &str| emitted.borrow_mut().push(delta.to_string());
        let mut aggregator =
            StreamingDeltaAggregator::with_policy(&mut callback, Duration::from_millis(40), 4096);
        aggregator.push("first", now);
        aggregator.push("tail", now);

        assert_eq!(*emitted.borrow(), ["first"]);
        assert_eq!(
            aggregator.before_poll(now + Duration::from_millis(39), Duration::from_secs(1)),
            Duration::from_millis(1)
        );
        aggregator.before_poll(now + Duration::from_millis(40), Duration::from_secs(1));
        assert_eq!(*emitted.borrow(), ["first", "tail"]);
    }

    #[test]
    fn pending_bytes_stay_bounded_while_large_deltas_preserve_order() {
        let now = Instant::now();
        let emitted = RefCell::new(Vec::new());
        let mut callback = |delta: &str| emitted.borrow_mut().push(delta.to_string());
        let mut aggregator =
            StreamingDeltaAggregator::with_policy(&mut callback, Duration::from_secs(1), 4);
        aggregator.push("0", now);
        aggregator.push("ab", now);
        assert_eq!(aggregator.pending.len(), 2);
        aggregator.push("cd", now);
        assert!(aggregator.pending.is_empty());
        aggregator.push("xy", now);
        aggregator.push("oversized", now);

        assert_eq!(*emitted.borrow(), ["0", "abcd", "xy", "oversized"]);
        assert!(aggregator.pending.is_empty());
    }

    #[test]
    fn utf8_content_is_exact_and_callback_count_is_bounded() {
        let now = Instant::now();
        let emitted = RefCell::new(Vec::new());
        let mut callback = |delta: &str| emitted.borrow_mut().push(delta.to_string());
        let mut aggregator =
            StreamingDeltaAggregator::with_policy(&mut callback, Duration::from_secs(1), 64);
        let mut expected = String::new();
        for delta in std::iter::once("始").chain(std::iter::repeat_n("界", 10_000)) {
            expected.push_str(delta);
            aggregator.push(delta, now);
            assert!(aggregator.pending.len() <= 64);
        }
        aggregator.finish();

        let emitted = emitted.borrow();
        assert_eq!(emitted.concat(), expected);
        assert!(emitted.len() < 600);
    }
}
