// Group-by counting helper for command outputs (e.g. how many `function`s vs
// `struct`s a `symbol` query returned). Counts are computed on the *full*
// candidate set, not the paginated page, so they survive offset/limit.

use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Facets {
    pub total: usize,
    // BTreeMap so JSON output order is stable (alphabetical by kind).
    pub by_kind: BTreeMap<String, usize>,
}

impl Facets {
    pub fn from_kinds<I, S>(kinds: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
        let mut total = 0usize;
        for k in kinds {
            *by_kind.entry(k.as_ref().to_string()).or_insert(0) += 1;
            total += 1;
        }
        Self { total, by_kind }
    }
}

#[cfg(test)]
mod tests {
    use super::Facets;

    #[test]
    fn empty_input_yields_zero_total_and_no_buckets() {
        let f = Facets::from_kinds(Vec::<&str>::new());
        assert_eq!(f.total, 0);
        assert!(f.by_kind.is_empty());
    }

    #[test]
    fn counts_grouped_by_kind() {
        let f = Facets::from_kinds(vec!["function", "function", "struct", "function"]);
        assert_eq!(f.total, 4);
        assert_eq!(f.by_kind.get("function"), Some(&3));
        assert_eq!(f.by_kind.get("struct"), Some(&1));
    }

    #[test]
    fn buckets_are_sorted_alphabetically() {
        let f = Facets::from_kinds(vec!["zeta", "alpha", "mu", "alpha"]);
        let keys: Vec<&str> = f.by_kind.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["alpha", "mu", "zeta"]);
    }

    #[test]
    fn accepts_owned_strings_too() {
        let owned = vec![String::from("a"), String::from("b"), String::from("a")];
        let f = Facets::from_kinds(owned);
        assert_eq!(f.total, 3);
        assert_eq!(f.by_kind.get("a"), Some(&2));
        assert_eq!(f.by_kind.get("b"), Some(&1));
    }
}
