use std::path::PathBuf;

const MAX: usize = 100;

/// Per-tab back/forward stacks, bounded.
#[derive(Clone, Debug, Default)]
pub struct History {
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
}

impl History {
    /// Records a navigation away from `from`. Clears the forward stack.
    pub fn push(&mut self, from: PathBuf) {
        if self.back.last() != Some(&from) {
            self.back.push(from);
            if self.back.len() > MAX {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    pub fn back(&mut self, current: PathBuf) -> Option<PathBuf> {
        let p = self.back.pop()?;
        self.forward.push(current);
        Some(p)
    }

    pub fn forward(&mut self, current: PathBuf) -> Option<PathBuf> {
        let p = self.forward.pop()?;
        self.back.push(current);
        Some(p)
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }
    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_and_forward() {
        let mut h = History::default();
        h.push("/a".into());
        h.push("/b".into());
        assert_eq!(h.back("/c".into()), Some("/b".into()));
        assert_eq!(h.forward("/b".into()), Some("/c".into()));
        assert!(!h.can_forward());
        h.back("/c".into());
        h.push("/b".into()); // new branch drops forward history
        assert!(!h.can_forward());
    }
}
