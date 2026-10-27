//! Edit-against-edit overlap, as named policies.
//!
//! Two callers refuse overlapping edits and they differ on purpose. Applying
//! a diff's word edits requires sorted input and lets coincident insertions
//! stack in order. An agent's patch list is sorted first and refuses two
//! insertions at one position. Each rule is a constant here so the
//! difference is visible instead of buried in two loops.
//!
//! A range is `(start, end)` in whatever single unit the caller counts. An
//! empty range is an insertion. This is not the question "does a range meet
//! a suggestion mark"; that one counts touching as meeting and lives with the
//! marks.

/// Whether a case is accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Allow {
    Yes,
    No,
}

/// The cases on which edit-overlap rules differ.
#[derive(Clone, Copy, Debug)]
pub struct OverlapPolicy {
    /// Two non-empty ranges meeting at one end: `[0,2)` and `[2,4)`.
    pub touching: Allow,
    /// An insertion at the start or end of a non-empty range. An insertion
    /// strictly inside a range is always refused.
    pub insert_at_boundary: Allow,
    /// Two insertions at the same position.
    pub coincident_inserts: Allow,
    /// True: the input must already be sorted by start, and an unsorted pair
    /// is refused. False: the ranges are sorted by `(start, end)` first.
    pub sorted_input: bool,
}

/// The rule of `apply_text_edits`: sorted input, everything that only
/// touches is allowed.
pub const APPLY_SORTED: OverlapPolicy = OverlapPolicy {
    touching: Allow::Yes,
    insert_at_boundary: Allow::Yes,
    coincident_inserts: Allow::Yes,
    sorted_input: true,
};

/// The rule for an agent's patch list: sorted first, coincident insertions
/// refused.
pub const AGENT_PATCH: OverlapPolicy = OverlapPolicy {
    touching: Allow::Yes,
    insert_at_boundary: Allow::Yes,
    coincident_inserts: Allow::No,
    sorted_input: false,
};

/// Two ranges that conflict, as indexes into the input of `check_overlaps`.
#[derive(Debug, PartialEq, Eq)]
pub struct Overlap {
    pub first: usize,
    pub second: usize,
}

/// Checks each range against the one before it, in start order.
pub fn check_overlaps(ranges: &[(usize, usize)], policy: OverlapPolicy) -> Result<(), Overlap> {
    let mut order: Vec<usize> = (0..ranges.len()).collect();
    if !policy.sorted_input {
        order.sort_by_key(|&i| ranges[i]);
    }
    for pair in order.windows(2) {
        if conflicts(ranges[pair[0]], ranges[pair[1]], policy) {
            return Err(Overlap {
                first: pair[0],
                second: pair[1],
            });
        }
    }
    Ok(())
}

/// Whether `b`, which follows `a`, conflicts with it.
fn conflicts(a: (usize, usize), b: (usize, usize), policy: OverlapPolicy) -> bool {
    if b.0 < a.0 {
        return true;
    }
    let refused = |allow: Allow| allow == Allow::No;
    match (a.0 == a.1, b.0 == b.1) {
        (true, true) => a.0 == b.0 && refused(policy.coincident_inserts),
        (true, false) => a.0 == b.0 && refused(policy.insert_at_boundary),
        (false, true) => {
            if b.0 == a.1 {
                refused(policy.insert_at_boundary)
            } else {
                b.0 < a.1
            }
        }
        (false, false) => {
            if b.0 == a.1 {
                refused(policy.touching)
            } else {
                b.0 < a.1
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(ranges: &[(usize, usize)], policy: OverlapPolicy) -> bool {
        check_overlaps(ranges, policy).is_ok()
    }

    #[test]
    fn touching_ranges() {
        let r = [(0, 2), (2, 4)];
        assert!(ok(&r, APPLY_SORTED));
        assert!(ok(&r, AGENT_PATCH));
    }

    #[test]
    fn insert_then_range_starting_there() {
        let r = [(2, 2), (2, 4)];
        assert!(ok(&r, APPLY_SORTED));
        assert!(ok(&r, AGENT_PATCH));
    }

    #[test]
    fn range_then_insert_at_its_end() {
        let r = [(0, 2), (2, 2)];
        assert!(ok(&r, APPLY_SORTED));
        assert!(ok(&r, AGENT_PATCH));
    }

    #[test]
    fn coincident_inserts() {
        let r = [(2, 2), (2, 2)];
        assert!(ok(&r, APPLY_SORTED));
        assert_eq!(
            check_overlaps(&r, AGENT_PATCH),
            Err(Overlap {
                first: 0,
                second: 1
            })
        );
    }

    #[test]
    fn intersecting_ranges() {
        let r = [(0, 3), (2, 4)];
        assert!(!ok(&r, APPLY_SORTED));
        assert!(!ok(&r, AGENT_PATCH));
    }

    #[test]
    fn unsorted_input() {
        let r = [(2, 4), (0, 2)];
        assert!(!ok(&r, APPLY_SORTED));
        assert!(ok(&r, AGENT_PATCH));
        assert!(!ok(&[(2, 4), (0, 3)], AGENT_PATCH));
    }

    #[test]
    fn insert_strictly_inside_a_range() {
        assert!(!ok(&[(0, 4), (2, 2)], APPLY_SORTED));
        assert!(!ok(&[(0, 4), (2, 2)], AGENT_PATCH));
        assert!(!ok(&[(2, 2), (0, 4)], AGENT_PATCH));
    }

    #[test]
    fn insert_after_a_range_that_starts_there() {
        let r = [(2, 4), (2, 2)];
        assert!(!ok(&r, APPLY_SORTED));
        assert!(ok(&r, AGENT_PATCH));
    }

    #[test]
    fn indexes_refer_to_the_input_order() {
        assert_eq!(
            check_overlaps(&[(2, 4), (0, 3)], AGENT_PATCH),
            Err(Overlap {
                first: 1,
                second: 0
            })
        );
    }

    #[test]
    fn disallowed_cases_are_refused() {
        let strict = OverlapPolicy {
            touching: Allow::No,
            insert_at_boundary: Allow::No,
            coincident_inserts: Allow::No,
            sorted_input: false,
        };
        assert!(!ok(&[(0, 2), (2, 4)], strict));
        assert!(!ok(&[(2, 2), (2, 4)], strict));
        assert!(!ok(&[(0, 2), (2, 2)], strict));
        assert!(!ok(&[(2, 2), (2, 2)], strict));
        assert!(ok(&[(0, 2), (3, 4)], strict));
    }
}
