//! Ranking and levelling.
//!
//! # Status: PROVISIONAL
//!
//! The model implemented here ([`ProvisionalV0`]) exists so that the vertical
//! slice has a real, testable number to display. It is **not** a validated skill
//! estimator and it is not presented as one.
//!
//! What it deliberately does **not** do, and why:
//!
//! | Not implemented | Why not yet |
//! | --- | --- |
//! | Score decay | Requires longitudinal data we do not have |
//! | Top-N weighting (osu!-style `0.95^n`) | The coefficient would be invented, not measured |
//! | Population normalisation | Needs a population |
//! | Anti-grind weighting | Needs observed grinding behaviour to calibrate against |
//!
//! Freezing a weighting coefficient before we have data would produce a formula
//! that *looks* scientific and is not. So the model is behind a trait, every
//! score carries the id of the model that produced it, and replacing the model
//! is a normal operation rather than a migration crisis.
//!
//! Two properties are non-negotiable across any future model and are enforced by
//! tests here:
//!
//! 1. Ranking is never a function of raw solve count alone - difficulty matters.
//! 2. Only reviewed Trials (`Verified`/`Official`) contribute.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use rustly_domain::{Difficulty, Level, RankScore, TrialLifecycle};
use serde::{Deserialize, Serialize};

/// One accepted solve, as input to a ranking model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Solve {
    /// Difficulty of the solved Trial.
    pub difficulty: Difficulty,
    /// Editorial state of the Trial at the time the score is computed.
    pub lifecycle: TrialLifecycle,
    /// Whether the user revealed published solutions before solving.
    ///
    /// Revealing is allowed and carries no penalty to learning. It removes
    /// first-solve ranking credit only.
    pub revealed_solutions: bool,
}

impl Solve {
    /// A plain, unrevealed solve of a verified Trial.
    pub fn verified(difficulty: Difficulty) -> Self {
        Self {
            difficulty,
            lifecycle: TrialLifecycle::Verified,
            revealed_solutions: false,
        }
    }
}

/// A ranking model.
///
/// Implementations must be pure and deterministic: the same solves in any order
/// produce the same score. That makes the score recomputable from the solve
/// history, which is what lets us replace the model without a data migration.
pub trait RankingModel: Send + Sync + 'static {
    /// Stable identifier stored alongside every computed score, e.g.
    /// `"provisional-v0"`. Never reuse an id after changing behaviour.
    fn id(&self) -> &'static str;

    /// Whether this model has been validated against real data.
    ///
    /// The API surfaces this so the UI can label the number honestly.
    fn is_provisional(&self) -> bool;

    /// Compute a rank score from a user's solves.
    fn score(&self, solves: &[Solve]) -> RankScore;
}

/// The first, explicitly provisional model.
///
/// Score is the sum of difficulty weights over distinct solved Trials that count
/// for ranking. Revealed-solution solves contribute nothing to rank; they still
/// count as solved for the user's own progress and milestone badge.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProvisionalV0;

impl ProvisionalV0 {
    /// Difficulty weights.
    ///
    /// The spread is intentionally modest. A wide spread would encode a strong
    /// claim about relative difficulty that we cannot yet support.
    pub const fn weight(difficulty: Difficulty) -> f64 {
        match difficulty {
            Difficulty::Intro => 1.0,
            Difficulty::Easy => 3.0,
            Difficulty::Medium => 8.0,
            Difficulty::Hard => 18.0,
            Difficulty::Expert => 35.0,
        }
    }

    /// Whether a solve contributes to rank at all.
    pub const fn counts(solve: &Solve) -> bool {
        solve.lifecycle.counts_for_ranking() && !solve.revealed_solutions
    }
}

impl RankingModel for ProvisionalV0 {
    fn id(&self) -> &'static str {
        "provisional-v0"
    }

    fn is_provisional(&self) -> bool {
        true
    }

    fn score(&self, solves: &[Solve]) -> RankScore {
        let total: f64 = solves
            .iter()
            .filter(|s| Self::counts(s))
            .map(|s| Self::weight(s.difficulty))
            .sum();
        RankScore(total)
    }
}

/// The levelling curve.
///
/// Level is separate from rank on purpose. Rank is a comparison between users;
/// level is a personal progress signal that only ever goes up, so that a user
/// who takes a hard Trial and fails is never punished twice.
pub mod level {
    use super::*;

    /// Experience awarded for a solve, including revealed-solution solves.
    ///
    /// Learning happened either way, so experience is granted either way.
    pub fn experience_for(solve: &Solve) -> u64 {
        let base = ProvisionalV0::weight(solve.difficulty) * 10.0;
        let scaled = if solve.lifecycle.counts_for_ranking() {
            base
        } else {
            base * 0.5
        };
        scaled as u64
    }

    /// Total experience required to *reach* `level`.
    ///
    /// Quadratic: `50 * (level - 1)^2`. Early levels come quickly, later ones
    /// take real work, and the curve never plateaus into unreachability.
    pub const fn threshold(level: u32) -> u64 {
        let n = (level.saturating_sub(1)) as u64;
        50 * n * n
    }

    /// The level corresponding to `experience`.
    pub fn from_experience(experience: u64) -> Level {
        // threshold(n+1) = 50 * n^2 <= xp  =>  n <= sqrt(xp / 50)
        let n = ((experience / 50) as f64).sqrt() as u32;
        // Correct for floating-point truncation at exact boundaries.
        let mut level = n + 1;
        while threshold(level + 1) <= experience {
            level += 1;
        }
        while level > 1 && threshold(level) > experience {
            level -= 1;
        }
        Level(level)
    }
}

#[cfg(test)]
mod tests {
    use super::level::{experience_for, from_experience, threshold};
    use super::*;

    #[test]
    fn model_declares_itself_provisional() {
        let model = ProvisionalV0;
        assert_eq!(model.id(), "provisional-v0");
        assert!(
            model.is_provisional(),
            "the v0 model must never claim to be validated"
        );
    }

    #[test]
    fn ranking_is_not_a_function_of_solve_count_alone() {
        let model = ProvisionalV0;
        let ten_intro: Vec<_> = (0..10)
            .map(|_| Solve::verified(Difficulty::Intro))
            .collect();
        let one_expert = [Solve::verified(Difficulty::Expert)];

        let many = model.score(&ten_intro).0;
        let few = model.score(&one_expert).0;
        assert!(
            few > many,
            "one Expert ({few}) must outweigh ten Intro ({many}); otherwise rank is a grind counter"
        );
    }

    #[test]
    fn difficulty_weights_are_strictly_increasing() {
        let weights: Vec<f64> = Difficulty::ALL
            .iter()
            .copied()
            .map(ProvisionalV0::weight)
            .collect();
        for pair in weights.windows(2) {
            assert!(pair[1] > pair[0], "weights must increase: {weights:?}");
        }
    }

    #[test]
    fn unreviewed_trials_do_not_contribute() {
        let model = ProvisionalV0;
        for lifecycle in [TrialLifecycle::Draft, TrialLifecycle::Beta] {
            let solve = Solve {
                difficulty: Difficulty::Expert,
                lifecycle,
                revealed_solutions: false,
            };
            assert_eq!(
                model.score(&[solve]),
                RankScore(0.0),
                "{lifecycle:?} must not score"
            );
        }
    }

    #[test]
    fn revealing_solutions_removes_rank_credit_but_not_experience() {
        let model = ProvisionalV0;
        let revealed = Solve {
            difficulty: Difficulty::Hard,
            lifecycle: TrialLifecycle::Verified,
            revealed_solutions: true,
        };
        assert_eq!(model.score(&[revealed]), RankScore(0.0));
        assert!(experience_for(&revealed) > 0, "learning still happened");
    }

    #[test]
    fn score_is_order_independent() {
        let model = ProvisionalV0;
        let mut solves = vec![
            Solve::verified(Difficulty::Intro),
            Solve::verified(Difficulty::Expert),
            Solve::verified(Difficulty::Medium),
        ];
        let a = model.score(&solves);
        solves.reverse();
        assert_eq!(a, model.score(&solves));
    }

    #[test]
    fn empty_history_scores_zero() {
        assert_eq!(ProvisionalV0.score(&[]), RankScore::ZERO);
    }

    #[test]
    fn level_curve_is_monotone_and_hits_exact_boundaries() {
        assert_eq!(threshold(1), 0);
        assert_eq!(threshold(2), 50);
        assert_eq!(threshold(3), 200);

        assert_eq!(from_experience(0), Level(1));
        assert_eq!(from_experience(49), Level(1));
        assert_eq!(from_experience(50), Level(2));
        assert_eq!(from_experience(199), Level(2));
        assert_eq!(from_experience(200), Level(3));

        let mut previous = Level(1);
        for xp in (0..200_000).step_by(37) {
            let level = from_experience(xp);
            assert!(level >= previous, "level must never decrease at xp={xp}");
            assert!(
                threshold(level.0) <= xp,
                "level {level:?} unreachable at xp={xp}"
            );
            assert!(
                threshold(level.0 + 1) > xp,
                "level {level:?} too low at xp={xp}"
            );
            previous = level;
        }
    }

    #[test]
    fn a_replacement_model_can_be_swapped_in_behind_the_trait() {
        struct CountingModel;
        impl RankingModel for CountingModel {
            fn id(&self) -> &'static str {
                "test-counting"
            }
            fn is_provisional(&self) -> bool {
                true
            }
            fn score(&self, solves: &[Solve]) -> RankScore {
                RankScore(solves.len() as f64)
            }
        }
        let models: Vec<Box<dyn RankingModel>> =
            vec![Box::new(ProvisionalV0), Box::new(CountingModel)];
        let solves = [Solve::verified(Difficulty::Expert)];
        let ids: Vec<_> = models.iter().map(|m| m.id()).collect();
        assert_eq!(ids, ["provisional-v0", "test-counting"]);
        assert_eq!(models[1].score(&solves), RankScore(1.0));
    }
}
