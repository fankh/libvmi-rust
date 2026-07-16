use std::sync::Mutex;

use vmi_driver_api::ViewAccess;
use vmi_types::{Result, VmiError};

const VIEW_WORD_BITS: usize = 64;
const VIEW_WORDS: usize = 1024;

#[derive(Debug)]
struct State {
    views: [u64; VIEW_WORDS],
    view_count: usize,
    active: u16,
    next: u16,
}

impl State {
    fn location(view: u16) -> Result<(usize, u64)> {
        let raw = usize::from(view);
        let word = raw / VIEW_WORD_BITS;
        let shift = u32::try_from(raw % VIEW_WORD_BITS)
            .map_err(|_| VmiError::Backend("memory view bitmap shift overflow".into()))?;
        let mask = 1u64
            .checked_shl(shift)
            .ok_or_else(|| VmiError::Backend("memory view bitmap mask overflow".into()))?;
        Ok((word, mask))
    }

    fn contains(&self, view: u16) -> Result<bool> {
        if view == 0 {
            return Ok(true);
        }
        let (word, mask) = Self::location(view)?;
        Ok(self.views.get(word).copied().unwrap_or_default() & mask != 0)
    }

    fn insert(&mut self, view: u16) -> Result<bool> {
        let (word, mask) = Self::location(view)?;
        if self.views.get(word).copied().unwrap_or_default() & mask != 0 {
            return Ok(false);
        }
        let next_count = self
            .view_count
            .checked_add(1)
            .ok_or_else(|| VmiError::Backend("memory view count overflow".into()))?;
        let entry = self
            .views
            .get_mut(word)
            .ok_or_else(|| VmiError::Backend("memory view bitmap index overflow".into()))?;
        *entry |= mask;
        self.view_count = next_count;
        Ok(true)
    }

    fn remove(&mut self, view: u16) -> Result<bool> {
        let (word, mask) = Self::location(view)?;
        if self.views.get(word).copied().unwrap_or_default() & mask == 0 {
            return Ok(false);
        }
        let next_count = self
            .view_count
            .checked_sub(1)
            .ok_or_else(|| VmiError::Backend("memory view count underflow".into()))?;
        let entry = self
            .views
            .get_mut(word)
            .ok_or_else(|| VmiError::Backend("memory view bitmap index overflow".into()))?;
        *entry &= !mask;
        self.view_count = next_count;
        Ok(true)
    }
}

#[derive(Debug)]
pub struct MemoryViewManager {
    state: Mutex<State>,
}

impl MemoryViewManager {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                views: [0; VIEW_WORDS],
                view_count: 1,
                active: 0,
                next: 1,
            }),
        }
    }

    pub fn create(&self) -> Result<u16> {
        let mut state = self.state.lock().map_err(poisoned)?;
        let start = state.next;
        loop {
            let candidate = state.next;
            state.next = state.next.wrapping_add(1);
            if candidate != 0 && state.insert(candidate)? {
                return Ok(candidate);
            }
            if state.next == start {
                return Err(VmiError::Backend(
                    "memory view identifiers exhausted".into(),
                ));
            }
        }
    }

    pub fn destroy(&self, view: u16) -> Result<()> {
        let mut state = self.state.lock().map_err(poisoned)?;
        if view == 0 {
            return Err(VmiError::Backend(
                "default memory view cannot be destroyed".into(),
            ));
        }
        if view == state.active {
            return Err(VmiError::Backend(format!(
                "active memory view {view} cannot be destroyed"
            )));
        }
        if !state.remove(view)? {
            return Err(VmiError::Backend(format!(
                "memory view {view} does not exist"
            )));
        }
        Ok(())
    }

    pub fn views(&self) -> Result<Vec<u16>> {
        let state = self.state.lock().map_err(poisoned)?;
        let mut views = Vec::new();
        views.try_reserve_exact(state.view_count).map_err(|error| {
            VmiError::Backend(format!("failed to allocate memory view list: {error}"))
        })?;
        for view in u16::MIN..=u16::MAX {
            if state.contains(view)? {
                views.push(view);
            }
        }
        Ok(views)
    }
}

impl Default for MemoryViewManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewAccess for MemoryViewManager {
    fn active_view(&self) -> Result<u16> {
        Ok(self.state.lock().map_err(poisoned)?.active)
    }
    fn switch_view(&self, view: u16) -> Result<()> {
        let mut state = self.state.lock().map_err(poisoned)?;
        if !state.contains(view)? {
            return Err(VmiError::Backend(format!(
                "memory view {view} does not exist"
            )));
        }
        state.active = view;
        Ok(())
    }
}

fn poisoned(error: impl std::fmt::Display) -> VmiError {
    VmiError::Backend(format!("memory view synchronization failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeSet, sync::Arc, thread};

    #[test]
    fn enforces_view_lifecycle() {
        let manager = MemoryViewManager::new();
        let first = manager.create().unwrap();
        let second = manager.create().unwrap();
        assert_eq!(manager.views().unwrap(), vec![0, first, second]);
        manager.switch_view(first).unwrap();
        assert_eq!(manager.active_view().unwrap(), first);
        assert!(manager.destroy(first).is_err());
        assert!(manager.destroy(0).is_err());
        manager.switch_view(0).unwrap();
        manager.destroy(first).unwrap();
        assert!(manager.switch_view(first).is_err());
    }

    #[test]
    fn concurrent_creates_return_unique_views() {
        let manager = Arc::new(MemoryViewManager::new());
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let manager = Arc::clone(&manager);
                thread::spawn(move || manager.create().unwrap())
            })
            .collect();
        let created: BTreeSet<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(created.len(), 8);
        assert_eq!(manager.views().unwrap().len(), 9);
    }

    #[test]
    fn exhausts_full_identifier_space_and_reuses_destroyed_view() {
        let manager = MemoryViewManager::new();
        for expected in 1..=u16::MAX {
            assert_eq!(manager.create().unwrap(), expected);
        }
        assert!(manager.create().is_err());
        manager.destroy(32_768).unwrap();
        assert_eq!(manager.create().unwrap(), 32_768);
        assert_eq!(manager.views().unwrap().len(), usize::from(u16::MAX) + 1);
    }
}
