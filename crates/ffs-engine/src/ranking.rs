//! Score search hits.
//!
//! Inputs:
//! * symbol-definition weight (from `ffs_symbol::definition_weight`)
//! * frecency score (passed in by caller)
//! * exact / fuzzy match flags
//!
//! Output: deterministic score where higher = better.

use ffs_symbol::types::Match;

/// Inputs to compute a single score.
#[derive(Debug, Clone, Copy)]
pub struct RankInputs {
    pub def_weight: u16,
    pub frecency_score: u32,
    pub exact: bool,
    pub is_definition: bool,
    pub in_comment: bool,
}

/// Compute a single score in `[0, u64::MAX)` with definition + exact matches first.
///
/// Semantics (intentional, verified against the ranking tests):
/// * `is_definition` adds `def_weight * 1_000_000` — a definition with
///   `def_weight >= 50` outranks an exact-match *usage* (50_000_000). This is
///   deliberate: `ffs find`/`ffs symbol` are definition-first, and
///   `def_weight` is language-kind aware (fn/class > var > field).
/// * `exact` adds a flat 50_000_000 regardless of definition status, so an
///   exact usage still beats a fuzzy definition.
/// * `in_comment` subtracts a flat 10_000_000 with `saturating_sub`: a weak
///   match can be pushed to 0, where it ties other zero-scored hits and is
///   ordered by the caller's deterministic tie-break. It is a penalty, not
///   a filter — comment hits are never excluded.
/// * `frecency_score * 10` is added last and can lift a comment hit back
///   above weak non-comment hits; frecency wins ties by design.
#[must_use]
pub fn score_one(inputs: RankInputs) -> u64 {
    let mut score: u64 = 0;

    if inputs.is_definition {
        score = score.saturating_add(u64::from(inputs.def_weight) * 1_000_000);
    }
    if inputs.exact {
        score = score.saturating_add(50_000_000);
    }
    if inputs.in_comment {
        score = score.saturating_sub(10_000_000);
    }
    score = score.saturating_add(u64::from(inputs.frecency_score) * 10);

    score
}

/// Sort `matches` in-place by score, highest first. `frecency_lookup` maps
/// each match to its frecency score (0 if untracked).
pub fn rank_matches<F>(matches: &mut [Match], frecency_lookup: F)
where
    F: Fn(&Match) -> u32,
{
    let mut scores: Vec<(usize, u64)> = matches
        .iter()
        .enumerate()
        .map(|(i, m)| {
            (
                i,
                score_one(RankInputs {
                    def_weight: m.def_weight,
                    frecency_score: frecency_lookup(m),
                    exact: m.exact,
                    is_definition: m.is_definition,
                    in_comment: m.in_comment,
                }),
            )
        })
        .collect();

    // Sort indices by score descending.
    scores.sort_by_key(|b| std::cmp::Reverse(b.1));

    // Reorder matches to match score order.
    let permutation: Vec<usize> = scores.iter().map(|(i, _)| *i).collect();
    apply_permutation(matches, &permutation);
}

fn apply_permutation<T>(slice: &mut [T], perm: &[usize]) {
    let mut visited = vec![false; slice.len()];
    for i in 0..slice.len() {
        if visited[i] || perm[i] == i {
            visited[i] = true;
            continue;
        }
        let mut current = i;
        while !visited[current] {
            visited[current] = true;
            let next = perm[current];
            if next == i {
                break;
            }
            slice.swap(current, next);
            current = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_outranks_usage() {
        let def = score_one(RankInputs {
            def_weight: 100,
            frecency_score: 0,
            exact: true,
            is_definition: true,
            in_comment: false,
        });
        let usage = score_one(RankInputs {
            def_weight: 0,
            frecency_score: 0,
            exact: true,
            is_definition: false,
            in_comment: false,
        });
        assert!(def > usage);
    }

    #[test]
    fn comment_match_penalized() {
        let comment = score_one(RankInputs {
            def_weight: 0,
            frecency_score: 100,
            exact: true,
            is_definition: false,
            in_comment: true,
        });
        let normal = score_one(RankInputs {
            def_weight: 0,
            frecency_score: 100,
            exact: true,
            is_definition: false,
            in_comment: false,
        });
        assert!(normal > comment);
    }
}
