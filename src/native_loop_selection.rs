//! Bounded ownership of the loop-selection sets used by the native runtime.
//!
//! C++ keeps ten independent `LoopList`s.  This type owns the equivalent state
//! and deliberately stores only loop ids: loop playback state remains owned by
//! the loop manager.

pub const NUM_SELECTION_SETS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionError {
    InvalidSet,
    CapacityExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeLoopSelection {
    sets: [Vec<usize>; NUM_SELECTION_SETS],
    capacity: usize,
}

impl NativeLoopSelection {
    pub fn new(capacity: usize) -> Self {
        Self {
            sets: std::array::from_fn(|_| Vec::with_capacity(capacity)),
            capacity,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn selected(&self, set: usize, loop_id: usize) -> Result<bool, SelectionError> {
        Ok(self.set(set)?.contains(&loop_id))
    }

    pub fn selected_ids(&self, set: usize) -> Result<&[usize], SelectionError> {
        Ok(self.set(set)?.as_slice())
    }

    pub fn count(&self, set: usize) -> Result<usize, SelectionError> {
        Ok(self.set(set)?.len())
    }

    pub fn toggle(&mut self, set: usize, loop_id: usize) -> Result<bool, SelectionError> {
        let capacity = self.capacity;
        let selected = self.set_mut(set)?;
        if let Some(pos) = selected.iter().position(|&id| id == loop_id) {
            selected.swap_remove(pos);
            Ok(false)
        } else {
            if selected.len() == capacity {
                return Err(SelectionError::CapacityExceeded);
            }
            selected.push(loop_id);
            Ok(true)
        }
    }

    pub fn clear(&mut self, set: usize) -> Result<(), SelectionError> {
        self.set_mut(set)?.clear();
        Ok(())
    }

    pub fn select_all(&mut self, set: usize, loop_ids: &[usize]) -> Result<(), SelectionError> {
        let capacity = self.capacity;
        let selected = self.set_mut(set)?;
        selected.clear();
        for &id in loop_ids {
            if !selected.contains(&id) {
                if selected.len() == capacity {
                    selected.clear();
                    return Err(SelectionError::CapacityExceeded);
                }
                selected.push(id);
            }
        }
        Ok(())
    }

    pub fn select_only_playing(
        &mut self,
        set: usize,
        loop_ids: &[usize],
        playing: impl Fn(usize) -> bool,
    ) -> Result<(), SelectionError> {
        let ids: Vec<_> = loop_ids.iter().copied().filter(|&id| playing(id)).collect();
        self.select_all(set, &ids)
    }

    pub fn invert(&mut self, set: usize, loop_ids: &[usize]) -> Result<(), SelectionError> {
        let old = self.set(set)?.clone();
        let capacity = self.capacity;
        let selected = self.set_mut(set)?;
        selected.clear();
        for &id in loop_ids {
            if !old.contains(&id) {
                if selected.len() == capacity {
                    selected.clear();
                    return Err(SelectionError::CapacityExceeded);
                }
                selected.push(id);
            }
        }
        Ok(())
    }

    /// Remove an erased loop from every set, matching `UpdateLoopLists_ItemRemoved`.
    pub fn update_after_erase(&mut self, loop_id: usize) {
        for set in &mut self.sets {
            set.retain(|&id| id != loop_id);
        }
    }

    /// Keep imported/runtime loop ids valid in every set.  This also removes
    /// stale selections after a library import or reload.
    pub fn update_after_import(&mut self, available_ids: &[usize]) {
        for set in &mut self.sets {
            set.retain(|id| available_ids.contains(id));
        }
    }

    /// Apply the C++ loop-index update after a move/import renumbering.
    pub fn update_after_move(&mut self, old_id: usize, new_id: usize) {
        for set in &mut self.sets {
            if let Some(id) = set.iter_mut().find(|id| **id == old_id) {
                *id = new_id;
            }
        }
    }

    /// Return ids to erase, then remove them from all sets.  The caller owns
    /// deleting the actual loop objects.
    pub fn erase_selected(&mut self, set: usize) -> Result<Vec<usize>, SelectionError> {
        let ids = self.set(set)?.clone();
        for id in &ids {
            self.update_after_erase(*id);
        }
        Ok(ids)
    }

    fn set(&self, set: usize) -> Result<&Vec<usize>, SelectionError> {
        self.sets.get(set).ok_or(SelectionError::InvalidSet)
    }
    fn set_mut(&mut self, set: usize) -> Result<&mut Vec<usize>, SelectionError> {
        self.sets.get_mut(set).ok_or(SelectionError::InvalidSet)
    }
}
