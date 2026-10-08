//! Back/forward navigation between entries.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub dict: usize,
    pub entry: u32,
}

#[derive(Default)]
pub struct History {
    back: Vec<Page>,
    forward: Vec<Page>,
}

impl History {
    /// Records that the user navigated away from `from`.
    pub fn push(&mut self, from: Page) {
        if self.back.last() != Some(&from) {
            self.back.push(from);
        }
        self.forward.clear();
    }

    pub fn back(&mut self, current: Option<Page>) -> Option<Page> {
        let page = self.back.pop()?;
        self.forward.extend(current);
        Some(page)
    }

    pub fn forward(&mut self, current: Option<Page>) -> Option<Page> {
        let page = self.forward.pop()?;
        self.back.extend(current);
        Some(page)
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn clear(&mut self) {
        self.back.clear();
        self.forward.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(entry: u32) -> Page {
        Page { dict: 0, entry }
    }

    #[test]
    fn back_and_forward() {
        let mut h = History::default();
        h.push(page(1));
        h.push(page(2));
        assert_eq!(h.back(Some(page(3))), Some(page(2)));
        assert_eq!(h.back(Some(page(2))), Some(page(1)));
        assert_eq!(h.back(Some(page(1))), None);
        assert_eq!(h.forward(Some(page(1))), Some(page(2)));
        assert!(h.can_go_forward());
        h.push(page(2));
        assert!(!h.can_go_forward());
    }
}
