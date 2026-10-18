#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Navigation {
    history: Vec<String>,
    current: Option<usize>,
}

impl Navigation {
    pub fn visit(&mut self, url: impl Into<String>) {
        if let Some(current) = self.current {
            self.history.truncate(current + 1);
        }
        self.history.push(url.into());
        self.current = Some(self.history.len() - 1);
    }

    pub fn current(&self) -> Option<&str> {
        self.current.map(|index| self.history[index].as_str())
    }

    pub fn back(&mut self) -> Option<&str> {
        let current = self.current?;
        if current > 0 {
            self.current = Some(current - 1);
        }
        self.current()
    }

    pub fn forward(&mut self) -> Option<&str> {
        let current = self.current?;
        if current + 1 < self.history.len() {
            self.current = Some(current + 1);
        }
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::Navigation;

    #[test]
    fn visit_after_back_discards_forward_history() {
        let mut navigation = Navigation::default();
        navigation.visit("one");
        navigation.visit("two");
        assert_eq!(navigation.back(), Some("one"));
        navigation.visit("three");
        assert_eq!(navigation.forward(), Some("three"));
    }
}
