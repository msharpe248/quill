//! Sets of IMAP UIDs, kept as ranges.
//!
//! A mailbox's UIDs are mostly contiguous, so fifty thousand of them are a
//! handful of ranges: small enough to carry in a sync cursor and to send in a
//! command.

use std::fmt;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UidSet {
    /// Inclusive, ascending, never touching or overlapping.
    ranges: Vec<(u32, u32)>,
}

impl UidSet {
    pub fn from_uids(uids: impl IntoIterator<Item = u32>) -> Self {
        let mut uids: Vec<u32> = uids.into_iter().collect();
        uids.sort_unstable();
        uids.dedup();
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for uid in uids {
            match ranges.last_mut() {
                Some((_, end)) if end.checked_add(1) == Some(uid) => *end = uid,
                _ => ranges.push((uid, uid)),
            }
        }
        Self { ranges }
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ranges
            .iter()
            .map(|(start, end)| (end - start) as usize + 1)
            .sum()
    }

    pub fn contains(&self, uid: u32) -> bool {
        self.ranges
            .binary_search_by(|(start, end)| {
                if uid < *start {
                    std::cmp::Ordering::Greater
                } else if uid > *end {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    /// Every UID, ascending.
    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.ranges.iter().flat_map(|(start, end)| *start..=*end)
    }

    /// The set as IMAP sequence sets, each of at most `max_ranges` ranges,
    /// so that no single command line grows without bound.
    pub fn sequence_sets(&self, max_ranges: usize) -> Vec<String> {
        self.ranges
            .chunks(max_ranges.max(1))
            .map(|chunk| {
                Self {
                    ranges: chunk.to_vec(),
                }
                .to_string()
            })
            .collect()
    }

    /// Parses what [`fmt::Display`] writes. The empty string is the empty set.
    pub fn parse(text: &str) -> Option<Self> {
        let mut uids = Vec::new();
        let mut ranges = Vec::new();
        for part in text.split(',').filter(|part| !part.is_empty()) {
            let (start, end) = match part.split_once(':') {
                Some((start, end)) => (start.parse().ok()?, end.parse().ok()?),
                None => {
                    let uid: u32 = part.parse().ok()?;
                    (uid, uid)
                }
            };
            if start == 0 || start > end {
                return None;
            }
            ranges.push((start, end));
        }
        // Normalise through the UIDs only when the text was not already in
        // canonical form, which text we wrote ourselves always is.
        let canonical = ranges.windows(2).all(|pair| pair[0].1 + 1 < pair[1].0);
        if canonical {
            return Some(Self { ranges });
        }
        for (start, end) in ranges {
            uids.extend(start..=end);
        }
        Some(Self::from_uids(uids))
    }
}

impl fmt::Display for UidSet {
    /// IMAP sequence-set syntax: `1:4,7,9:12`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, (start, end)) in self.ranges.iter().enumerate() {
            if index > 0 {
                f.write_str(",")?;
            }
            if start == end {
                write!(f, "{start}")?;
            } else {
                write!(f, "{start}:{end}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uids_collapse_into_ranges() {
        let set = UidSet::from_uids([5, 1, 2, 3, 9, 10, 3, 7]);
        assert_eq!(set.to_string(), "1:3,5,7,9:10");
        assert_eq!(set.len(), 7);
        assert_eq!(set.iter().collect::<Vec<_>>(), [1, 2, 3, 5, 7, 9, 10]);
        assert!(UidSet::from_uids([]).is_empty());
        assert_eq!(UidSet::default().to_string(), "");
    }

    #[test]
    fn membership() {
        let set = UidSet::from_uids([1, 2, 3, 7, 9, 10]);
        for uid in [1, 2, 3, 7, 9, 10] {
            assert!(set.contains(uid), "{uid}");
        }
        for uid in [0, 4, 6, 8, 11, u32::MAX] {
            assert!(!set.contains(uid), "{uid}");
        }
        assert!(!UidSet::default().contains(1));
    }

    #[test]
    fn display_and_parse_round_trip() {
        for text in ["", "1", "1:3", "1:3,5,7,9:10", "4294967295"] {
            assert_eq!(UidSet::parse(text).unwrap().to_string(), text);
        }
        let set = UidSet::from_uids((1..=50_000).filter(|uid| uid % 1000 != 0));
        assert_eq!(UidSet::parse(&set.to_string()).unwrap(), set);
        assert_eq!(set.len(), 49_950);
        // Fifty thousand UIDs with fifty gaps is a short string.
        assert!(set.to_string().len() < 1000);
    }

    #[test]
    fn parse_normalises_and_rejects_nonsense() {
        assert_eq!(UidSet::parse("3,1:2,2:4").unwrap().to_string(), "1:4");
        assert_eq!(UidSet::parse("1,2,3").unwrap().to_string(), "1:3");
        for bad in ["a", "1:", ":2", "3:1", "0", "1;2", "1:2:3", "-1"] {
            assert_eq!(UidSet::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_largest_uid_does_not_overflow() {
        let set = UidSet::from_uids([u32::MAX - 1, u32::MAX]);
        assert_eq!(set.to_string(), "4294967294:4294967295");
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn sequence_sets_bound_the_ranges_per_command() {
        let set = UidSet::from_uids((1..=20).filter(|uid| uid % 2 == 1));
        let sets = set.sequence_sets(4);
        assert_eq!(sets, ["1,3,5,7", "9,11,13,15", "17,19"]);
        assert!(UidSet::default().sequence_sets(4).is_empty());
        assert_eq!(UidSet::from_uids([1, 2, 3]).sequence_sets(0), ["1:3"]);
    }
}
