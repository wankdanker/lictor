//! Lease deadlines.
//!
//! A lease (node membership, health record) is valid for `ttl` after the store
//! accepted the write. The store's timestamp is converted once, on receipt, into
//! a deadline on the local monotonic clock:
//!
//! ```text
//! age      = max(0, local_now - server_timestamp - clock_skew)
//! deadline = monotonic_now + ttl - age
//! ```
//!
//! Consequences:
//! - A replayed old heartbeat (after a reconnect) has a large age, so it cannot
//!   extend a lease that has already run out.
//! - Wall-clock jumps after receipt do not affect leases.
//! - Up to `clock_skew` of difference between this host and the server is tolerated.

use std::time::{Duration, SystemTime};
use tokio::time::Instant;

/// The monotonic deadline of a lease written at `written` (store time) with `ttl`.
pub fn deadline(written: SystemTime, ttl: Duration, skew: Duration) -> Instant {
    let age = SystemTime::now().duration_since(written).unwrap_or_default().saturating_sub(skew);
    Instant::now() + ttl.saturating_sub(age)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTL: Duration = Duration::from_secs(30);
    const SKEW: Duration = Duration::from_secs(2);

    #[tokio::test(start_paused = true)]
    async fn fresh_write_gets_full_ttl() {
        let d = deadline(SystemTime::now(), TTL, SKEW);
        assert!(d - Instant::now() >= TTL - Duration::from_millis(5));
    }

    #[tokio::test(start_paused = true)]
    async fn old_write_is_shortened() {
        let d = deadline(SystemTime::now() - Duration::from_secs(20), TTL, SKEW);
        let left = d - Instant::now();
        assert!(left > Duration::from_secs(11) && left <= Duration::from_secs(12), "{left:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn replayed_expired_write_stays_expired() {
        let d = deadline(SystemTime::now() - Duration::from_secs(120), TTL, SKEW);
        assert!(d <= Instant::now());
    }

    #[tokio::test(start_paused = true)]
    async fn future_timestamps_from_skewed_server_are_capped() {
        let d = deadline(SystemTime::now() + Duration::from_secs(60), TTL, SKEW);
        assert!(d - Instant::now() <= TTL);
    }

    #[tokio::test(start_paused = true)]
    async fn skew_is_tolerated() {
        let d = deadline(SystemTime::now() - Duration::from_secs(31), TTL, SKEW);
        assert!(d > Instant::now(), "31s old with 2s skew is still alive");
    }
}
