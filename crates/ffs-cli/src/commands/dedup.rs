// Stable de-duplication keyed by an arbitrary key function. Preserves the
// first occurrence of each key; later duplicates are dropped. Used when
// accumulating hits from multiple lookup paths that may point at the same
// (path, line) site.

use std::collections::HashSet;
use std::hash::Hash;

pub(crate) fn dedup_by<T, K, F>(items: Vec<T>, mut key: F) -> Vec<T>
where
    F: FnMut(&T) -> K,
    K: Hash + Eq,
{
    let mut seen: HashSet<K> = HashSet::with_capacity(items.len());
    items.into_iter().filter(|t| seen.insert(key(t))).collect()
}

#[cfg(test)]
mod tests {
    use super::dedup_by;

    #[test]
    fn empty_input_returns_empty() {
        let out: Vec<i32> = dedup_by(Vec::new(), |x| *x);
        assert!(out.is_empty());
    }

    #[test]
    fn keeps_first_occurrence_of_each_key() {
        let v = vec![1, 2, 1, 3, 2, 4];
        let out = dedup_by(v, |x| *x);
        assert_eq!(out, vec![1, 2, 3, 4]);
    }

    #[test]
    fn key_can_be_a_subset_of_the_value() {
        let v = vec![("a", 1), ("b", 2), ("a", 3), ("c", 4)];
        let out = dedup_by(v, |t| t.0);
        // "a" appeared first as ("a", 1); the later ("a", 3) is dropped.
        assert_eq!(out, vec![("a", 1), ("b", 2), ("c", 4)]);
    }

    #[test]
    fn distinct_keys_pass_through_unchanged() {
        let v = vec![10, 20, 30];
        let out = dedup_by(v.clone(), |x| *x);
        assert_eq!(out, v);
    }

    #[test]
    fn composite_key_distinguishes_otherwise_equal_lhs() {
        // Same path, same name, different lines → all kept.
        let v = vec![("foo", "p", 1u32), ("foo", "p", 2u32), ("foo", "p", 1u32)];
        let out = dedup_by(v, |t| (t.0, t.1, t.2));
        assert_eq!(out, vec![("foo", "p", 1), ("foo", "p", 2)]);
    }
}
