//! Spaces out database writes for sessions that change on every poll.
//!
//! While an agent works, its session row changes every few seconds. The live
//! view reads memory, so the analytics database only needs the row about once
//! a minute; the latest copy is kept and written when the interval passes or
//! when the session stops being live.
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// How often a live session's row is written while it keeps changing.
pub const LIVE_WRITE_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct LiveWrites<T> {
    last_written: HashMap<String, Instant>,
    held: HashMap<String, T>,
}

impl<T> Default for LiveWrites<T> {
    fn default() -> Self {
        Self {
            last_written: HashMap::new(),
            held: HashMap::new(),
        }
    }
}

impl<T> LiveWrites<T> {
    /// Returns the row when it must be written now: the session is not live,
    /// or its last write is at least [`LIVE_WRITE_INTERVAL`] old. Otherwise the
    /// row replaces any held copy and comes back from [`LiveWrites::due`].
    pub fn offer(&mut self, key: &str, row: T, live: bool, now: Instant) -> Option<T> {
        let due = !live
            || self
                .last_written
                .get(key)
                .is_none_or(|last| now.saturating_duration_since(*last) >= LIVE_WRITE_INTERVAL);
        if due {
            self.held.remove(key);
            self.last_written.insert(key.to_string(), now);
            Some(row)
        } else {
            self.held.insert(key.to_string(), row);
            None
        }
    }

    /// Keys of the rows waiting for their next write.
    pub fn held_keys(&self) -> impl Iterator<Item = &str> {
        self.held.keys().map(String::as_str)
    }

    /// Held rows to write now: their interval passed, or their session is no
    /// longer in `live`. Sessions that left `live` are forgotten.
    pub fn due(&mut self, live: &HashSet<String>, now: Instant) -> Vec<T> {
        let ready: Vec<String> = self
            .held
            .keys()
            .filter(|key| {
                !live.contains(*key)
                    || self.last_written.get(*key).is_none_or(|last| {
                        now.saturating_duration_since(*last) >= LIVE_WRITE_INTERVAL
                    })
            })
            .cloned()
            .collect();
        let rows = ready
            .iter()
            .filter_map(|key| {
                self.last_written.insert(key.clone(), now);
                self.held.remove(key)
            })
            .collect();
        self.last_written
            .retain(|key, _| live.contains(key) || self.held.contains_key(key));
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLL: Duration = Duration::from_secs(5);

    fn live(keys: &[&str]) -> HashSet<String> {
        keys.iter().map(|key| key.to_string()).collect()
    }

    #[test]
    fn a_session_changing_every_poll_is_written_once_a_minute() {
        let mut writes = LiveWrites::default();
        let start = Instant::now();
        let mut written = Vec::new();
        for tick in 0..24u32 {
            let now = start + POLL * tick;
            written.extend(writes.offer("s", tick, true, now));
            written.extend(writes.due(&live(&["s"]), now));
        }
        assert_eq!(written, [0, 12], "two minutes of polls write twice");
    }

    #[test]
    fn the_last_copy_is_written_when_the_session_stops() {
        let mut writes = LiveWrites::default();
        let start = Instant::now();
        assert_eq!(writes.offer("s", 1, true, start), Some(1));
        assert_eq!(writes.offer("s", 2, true, start + POLL), None);
        assert_eq!(writes.offer("s", 3, true, start + POLL * 2), None);

        assert_eq!(writes.due(&live(&[]), start + POLL * 3), [3]);
        assert!(writes.due(&live(&[]), start + POLL * 4).is_empty());
    }

    #[test]
    fn rows_that_are_not_live_are_written_at_once() {
        let mut writes = LiveWrites::default();
        let now = Instant::now();
        assert_eq!(writes.offer("old", 1, false, now), Some(1));
        assert_eq!(writes.offer("old", 2, false, now), Some(2));
    }
}
