use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;

pub struct Session {
    expanded: RefCell<HashSet<String>>, // "path:line" -> already inlined
}

impl Session {
    pub fn new() -> Self {
        Session {
            expanded: RefCell::new(HashSet::new()),
        }
    }

    pub fn is_expanded(&self, path: &Path, line: u32) -> bool {
        let key = format!("{}:{}", path.display(), line);
        self.expanded.borrow().contains(&key)
    }

    pub fn record_expand(&self, path: &Path, line: u32) {
        let key = format!("{}:{}", path.display(), line);
        self.expanded.borrow_mut().insert(key);
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_tracks_expanded() {
        let s = Session::new();
        let p = Path::new("src/main.rs");
        assert!(!s.is_expanded(p, 42));
        s.record_expand(p, 42);
        assert!(s.is_expanded(p, 42));
        assert!(!s.is_expanded(p, 43));
    }

    #[test]
    fn session_isolated_per_instance() {
        let s1 = Session::new();
        let s2 = Session::new();
        let p = Path::new("a.rs");
        s1.record_expand(p, 1);
        assert!(s1.is_expanded(p, 1));
        assert!(!s2.is_expanded(p, 1));
    }
}
