use std::{
    collections::VecDeque,
    sync::{Condvar, Mutex},
    time::{Duration, Instant},
};

use vmi_driver_api::{EventAccess, VmiEvent};
use vmi_types::{Result, VmiError};

#[derive(Debug)]
struct State {
    events: VecDeque<VmiEvent>,
    closed: bool,
}

#[derive(Debug)]
pub struct EventQueue {
    capacity: usize,
    state: Mutex<State>,
    available: Condvar,
}

impl EventQueue {
    pub fn new(capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(VmiError::Backend(
                "event queue capacity must be non-zero".into(),
            ));
        }
        let mut events = VecDeque::new();
        events.try_reserve_exact(capacity).map_err(|error| {
            VmiError::Backend(format!(
                "failed to allocate event queue capacity {capacity}: {error}"
            ))
        })?;
        Ok(Self {
            capacity,
            state: Mutex::new(State {
                events,
                closed: false,
            }),
            available: Condvar::new(),
        })
    }

    pub fn push(&self, event: VmiEvent) -> Result<()> {
        let mut state = self.state.lock().map_err(poisoned)?;
        if state.closed {
            return Err(VmiError::Backend("event queue is closed".into()));
        }
        if state.events.len() == self.capacity {
            return Err(VmiError::Backend(format!(
                "event queue capacity {} exceeded",
                self.capacity
            )));
        }
        state.events.push_back(event);
        self.available.notify_one();
        Ok(())
    }

    pub fn close(&self) -> Result<()> {
        let mut state = self.state.lock().map_err(poisoned)?;
        state.closed = true;
        self.available.notify_all();
        Ok(())
    }

    pub fn len(&self) -> Result<usize> {
        Ok(self.state.lock().map_err(poisoned)?.events.len())
    }
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }
}

impl EventAccess for EventQueue {
    fn next_event(&self, timeout: Duration) -> Result<Option<VmiEvent>> {
        const MAX_WAIT_SLICE: Duration = Duration::from_secs(24 * 60 * 60);
        let deadline = Instant::now().checked_add(timeout);
        let mut state = self.state.lock().map_err(poisoned)?;
        loop {
            if let Some(event) = state.events.pop_front() {
                return Ok(Some(event));
            }
            if state.closed {
                return Ok(None);
            }
            let now = Instant::now();
            let remaining = match deadline {
                Some(deadline) if now >= deadline => return Ok(None),
                Some(deadline) => deadline.saturating_duration_since(now),
                None => MAX_WAIT_SLICE,
            };
            let (next, wait) = self
                .available
                .wait_timeout(state, remaining)
                .map_err(poisoned)?;
            state = next;
            if wait.timed_out() && state.events.is_empty() && deadline.is_some() {
                return Ok(None);
            }
        }
    }
}

fn poisoned(error: impl std::fmt::Display) -> VmiError {
    VmiError::Backend(format!("event queue synchronization failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, thread};
    use vmi_types::Gpa;

    fn event(kind: &str) -> VmiEvent {
        VmiEvent {
            kind: kind.into(),
            vcpu: Some(0),
            address: Some(Gpa::new(0x1000)),
        }
    }

    #[test]
    fn preserves_order_and_enforces_capacity() {
        let queue = EventQueue::new(2).unwrap();
        queue.push(event("first")).unwrap();
        queue.push(event("second")).unwrap();
        assert!(queue.push(event("overflow")).is_err());
        assert_eq!(
            queue.next_event(Duration::ZERO).unwrap().unwrap().kind,
            "first"
        );
        assert_eq!(
            queue.next_event(Duration::ZERO).unwrap().unwrap().kind,
            "second"
        );
    }

    #[test]
    fn waits_for_producer_and_close_unblocks_consumers() {
        let queue = Arc::new(EventQueue::new(4).unwrap());
        let producer = Arc::clone(&queue);
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            producer.push(event("ready")).unwrap();
        });
        assert_eq!(
            queue
                .next_event(Duration::from_secs(1))
                .unwrap()
                .unwrap()
                .kind,
            "ready"
        );
        handle.join().unwrap();
        queue.close().unwrap();
        assert_eq!(queue.next_event(Duration::from_secs(1)).unwrap(), None);
        assert!(queue.push(event("late")).is_err());
    }

    #[test]
    fn timeout_and_invalid_capacity_are_fail_closed() {
        assert!(EventQueue::new(0).is_err());
        assert!(EventQueue::new(usize::MAX).is_err());
        let queue = EventQueue::new(1).unwrap();
        assert_eq!(queue.next_event(Duration::from_millis(1)).unwrap(), None);
    }

    #[test]
    fn overflowing_timeout_waits_instead_of_expiring_immediately() {
        let queue = Arc::new(EventQueue::new(1).unwrap());
        let producer = Arc::clone(&queue);
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            producer.push(event("large-timeout")).unwrap();
        });
        assert_eq!(
            queue.next_event(Duration::MAX).unwrap().unwrap().kind,
            "large-timeout"
        );
        handle.join().unwrap();
    }
}
