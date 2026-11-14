//! Per-site budgets: token buckets that refill at a rate per minute and hold at most one
//! minute's worth, so that a page can load in a burst but cannot keep up more than its rate.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

/// A token bucket. `take` never waits; `wait_for` sleeps until the tokens are there.
#[derive(Debug)]
pub struct Bucket {
    state: Mutex<State>,
    capacity: f64,
    per_second: f64,
}

#[derive(Debug)]
struct State {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    /// Full at the start: `per_minute` tokens, refilled at `per_minute / 60` per second.
    pub fn per_minute(per_minute: u64) -> Bucket {
        let capacity = per_minute.max(1) as f64;
        Bucket {
            state: Mutex::new(State { tokens: capacity, at: Instant::now() }),
            capacity,
            per_second: capacity / 60.0,
        }
    }

    fn refill(&self, state: &mut State, now: Instant) {
        let elapsed = now.saturating_duration_since(state.at).as_secs_f64();
        state.tokens = (state.tokens + elapsed * self.per_second).min(self.capacity);
        state.at = now;
    }

    /// Take `n` tokens if they are there.
    pub fn take(&self, n: u64) -> bool {
        self.take_at(n, Instant::now())
    }

    fn take_at(&self, n: u64, now: Instant) -> bool {
        let mut state = self.state.lock().expect("bucket lock");
        self.refill(&mut state, now);
        if state.tokens >= n as f64 {
            state.tokens -= n as f64;
            true
        } else {
            false
        }
    }

    /// How long until `n` tokens could be taken (zero if they can now). More than the
    /// capacity is never there at once; such a request is measured against the capacity.
    pub fn wait_time(&self, n: u64) -> Duration {
        let mut state = self.state.lock().expect("bucket lock");
        self.refill(&mut state, Instant::now());
        let missing = (n as f64).min(self.capacity) - state.tokens;
        if missing <= 0.0 { Duration::ZERO } else { Duration::from_secs_f64(missing / self.per_second) }
    }

    /// Take `n` tokens, sleeping until they are there. A request for more than the bucket
    /// holds is taken in parts.
    pub async fn wait_for(&self, n: u64) {
        let mut left = n;
        while left > 0 {
            let part = left.min(self.capacity as u64).max(1);
            loop {
                if self.take(part) {
                    break;
                }
                // Short sleeps: the bucket is shared, and a long one would overshoot.
                tokio::time::sleep(self.wait_time(part).clamp(Duration::from_millis(5), Duration::from_millis(250)))
                    .await;
            }
            left -= part;
        }
    }

    /// Tokens there now, rounded down.
    pub fn available(&self) -> u64 {
        let mut state = self.state.lock().expect("bucket lock");
        self.refill(&mut state, Instant::now());
        state.tokens as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bucket_allows_a_minute_at_once_then_its_rate() {
        let bucket = Bucket::per_minute(60);
        let start = Instant::now();
        assert!(bucket.take_at(60, start), "a full minute's worth at once");
        assert!(!bucket.take_at(1, start), "then nothing");
        assert!(!bucket.take_at(1, start + Duration::from_millis(900)));
        assert!(bucket.take_at(1, start + Duration::from_millis(1100)), "one per second");
        assert!(bucket.take_at(30, start + Duration::from_secs(32)));
        // Never more than the capacity, however long it rests.
        assert!(!bucket.take_at(61, start + Duration::from_secs(3600)));
        assert!(bucket.take_at(60, start + Duration::from_secs(3600)));
    }

    #[tokio::test]
    async fn waiting_takes_the_time_the_rate_says() {
        let bucket = Bucket::per_minute(600); // ten per second
        assert!(bucket.take(600));
        let start = Instant::now();
        bucket.wait_for(3).await;
        let waited = start.elapsed();
        assert!(waited >= Duration::from_millis(250) && waited < Duration::from_secs(2), "{waited:?}");
        // More than the capacity is taken in parts instead of never.
        let small = Bucket::per_minute(6000);
        small.wait_for(6100).await;
    }
}
