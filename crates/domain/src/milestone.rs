//! Trial milestone badges.
//!
//! Product rule: exactly one badge is ever displayed, and it upgrades in place.
//! Badges do not stack, there is no gold/silver/diamond tiering, and there are no
//! collectible course badges. This module is the single place that rule lives.

use serde::{Deserialize, Serialize};

/// A Trial-count milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialMilestone {
    /// 10 Trials solved.
    Ten,
    /// 50 Trials solved.
    Fifty,
    /// 100 Trials solved.
    Hundred,
    /// 250 Trials solved.
    TwoFifty,
    /// 500 Trials solved.
    FiveHundred,
    /// 1000 Trials solved.
    Thousand,
}

impl TrialMilestone {
    /// Every milestone, ascending.
    pub const ALL: [TrialMilestone; 6] = [
        Self::Ten,
        Self::Fifty,
        Self::Hundred,
        Self::TwoFifty,
        Self::FiveHundred,
        Self::Thousand,
    ];

    /// The solve count at which this milestone is earned.
    pub const fn threshold(self) -> u32 {
        match self {
            Self::Ten => 10,
            Self::Fifty => 50,
            Self::Hundred => 100,
            Self::TwoFifty => 250,
            Self::FiveHundred => 500,
            Self::Thousand => 1000,
        }
    }

    /// The single milestone to display for `solved`, or `None` below the first
    /// threshold.
    pub fn highest_for(solved: u32) -> Option<Self> {
        Self::ALL
            .into_iter()
            .rev()
            .find(|m| solved >= m.threshold())
    }

    /// The next milestone to aim for, or `None` at the top of the ladder.
    pub fn next_after(solved: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|m| solved < m.threshold())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_follow_the_published_ladder() {
        assert_eq!(
            TrialMilestone::ALL.map(TrialMilestone::threshold),
            [10, 50, 100, 250, 500, 1000]
        );
    }

    #[test]
    fn no_badge_below_the_first_threshold() {
        for solved in [0, 1, 9] {
            assert_eq!(TrialMilestone::highest_for(solved), None);
        }
    }

    #[test]
    fn badge_upgrades_in_place_at_every_boundary() {
        let cases = [
            (10, TrialMilestone::Ten),
            (49, TrialMilestone::Ten),
            (50, TrialMilestone::Fifty),
            (99, TrialMilestone::Fifty),
            (100, TrialMilestone::Hundred),
            (249, TrialMilestone::Hundred),
            (250, TrialMilestone::TwoFifty),
            (500, TrialMilestone::FiveHundred),
            (999, TrialMilestone::FiveHundred),
            (1000, TrialMilestone::Thousand),
            (100_000, TrialMilestone::Thousand),
        ];
        for (solved, expected) in cases {
            assert_eq!(
                TrialMilestone::highest_for(solved),
                Some(expected),
                "solved={solved}"
            );
        }
    }

    #[test]
    fn next_milestone_is_the_first_unreached_one() {
        assert_eq!(TrialMilestone::next_after(0), Some(TrialMilestone::Ten));
        assert_eq!(TrialMilestone::next_after(10), Some(TrialMilestone::Fifty));
        assert_eq!(
            TrialMilestone::next_after(999),
            Some(TrialMilestone::Thousand)
        );
        assert_eq!(TrialMilestone::next_after(1000), None);
    }

    #[test]
    fn milestones_are_totally_ordered_by_threshold() {
        let mut sorted = TrialMilestone::ALL;
        sorted.sort();
        assert_eq!(sorted, TrialMilestone::ALL);
    }
}
