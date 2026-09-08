//! The accepted-result decision, as a pure function.
//!
//! Recording a judge verdict has to be atomic and identical across backends. If
//! each backend re-implemented the rules, the in-memory store used by tests and
//! the PostgreSQL store used in production would drift, and the tests would stop
//! proving anything.
//!
//! So the *decision* lives here as a pure function over the current state, and
//! each backend's only job is to apply the resulting [`Plan`] inside its own
//! transaction.

use rustly_domain::{Level, RankScore, TrialLifecycle, TrialStatus, Verdict, VerdictClass};
use rustly_ranking::{level, ProvisionalV0, RankingModel, Solve};

/// Everything the decision needs to know about the current state.
#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    /// Verdict being reported.
    pub verdict: Verdict,
    /// Whether the submission is already in a terminal state.
    pub already_finished: bool,
    /// The verdict already recorded, if the submission is finished.
    pub existing_verdict: Option<Verdict>,
    /// The user's current status for this Trial.
    pub current_status: TrialStatus,
    /// Editorial state of the Trial.
    pub lifecycle: TrialLifecycle,
    /// Difficulty of the Trial.
    pub difficulty: rustly_domain::Difficulty,
    /// Whether the user revealed published solutions before solving.
    pub revealed_solutions: bool,
    /// The user's counting solves *before* this result.
    pub existing_solves: Vec<Solve>,
    /// The user's accumulated experience before this result.
    pub existing_experience: u64,
}

/// The state change to apply.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Whether this report finalises the submission. `false` means duplicate.
    pub accepted: bool,
    /// The verdict that should end up recorded.
    pub recorded_verdict: Verdict,
    /// Whether this is the user's first accepted solve of the Trial.
    pub first_solve: bool,
    /// The Trial status the user should now have.
    pub new_status: TrialStatus,
    /// Whether the attempt counter should increment.
    pub increment_attempts: bool,
    /// Whether `trials_solved` should increment.
    pub increment_trials_solved: bool,
    /// The recomputed rank score, when it changed.
    pub new_rank_score: Option<RankScore>,
    /// The new accumulated experience, when it changed.
    pub new_experience: Option<u64>,
    /// The new level, when it changed.
    pub new_level: Option<Level>,
    /// Whether a `MilestoneReached` event should be emitted.
    pub milestone_reached: bool,
}

/// Decide what recording `context.verdict` should do.
///
/// Key properties, all covered by tests below:
///
/// * A duplicate report is a no-op that echoes the recorded verdict.
/// * A system fault (`JE`/`IE`) never changes user-visible state at all: a queue
///   outage cannot turn `Unsolved` into `Attempted`.
/// * A second accepted solve of the same Trial does not double-count.
/// * Rank is recomputed from the whole solve set, never incremented in place, so
///   swapping the ranking model is a recompute rather than a migration.
pub fn plan(context: &Context, trials_solved_before: u32) -> Plan {
    // Duplicate report: whoever finalised first wins.
    if context.already_finished {
        return Plan {
            accepted: false,
            recorded_verdict: context.existing_verdict.unwrap_or(context.verdict),
            first_solve: false,
            new_status: context.current_status,
            increment_attempts: false,
            increment_trials_solved: false,
            new_rank_score: None,
            new_experience: None,
            new_level: None,
            milestone_reached: false,
        };
    }

    // Infrastructure failure: record the verdict for operators, touch nothing else.
    if context.verdict.class() == VerdictClass::SystemFault {
        return Plan {
            accepted: true,
            recorded_verdict: context.verdict,
            first_solve: false,
            new_status: context.current_status,
            increment_attempts: false,
            increment_trials_solved: false,
            new_rank_score: None,
            new_experience: None,
            new_level: None,
            milestone_reached: false,
        };
    }

    let already_solved = context.current_status == TrialStatus::Solved;
    let first_solve = context.verdict == Verdict::Accepted && !already_solved;

    let new_status = match (context.verdict, context.current_status) {
        (Verdict::Accepted, _) => TrialStatus::Solved,
        (_, TrialStatus::Solved) => TrialStatus::Solved,
        _ => TrialStatus::Attempted,
    };

    if !first_solve {
        return Plan {
            accepted: true,
            recorded_verdict: context.verdict,
            first_solve: false,
            new_status,
            increment_attempts: true,
            increment_trials_solved: false,
            new_rank_score: None,
            new_experience: None,
            new_level: None,
            milestone_reached: false,
        };
    }

    let solve = Solve {
        difficulty: context.difficulty,
        lifecycle: context.lifecycle,
        revealed_solutions: context.revealed_solutions,
    };

    let mut solves = context.existing_solves.clone();
    solves.push(solve);

    let rank = ProvisionalV0.score(&solves);
    let experience = context.existing_experience + level::experience_for(&solve);
    let new_level = level::from_experience(experience);

    let solved_after = trials_solved_before + 1;
    let milestone_reached = rustly_domain::TrialMilestone::highest_for(solved_after)
        != rustly_domain::TrialMilestone::highest_for(trials_solved_before);

    Plan {
        accepted: true,
        recorded_verdict: context.verdict,
        first_solve: true,
        new_status: TrialStatus::Solved,
        increment_attempts: true,
        increment_trials_solved: true,
        new_rank_score: Some(rank),
        new_experience: Some(experience),
        new_level: Some(new_level),
        milestone_reached,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustly_domain::Difficulty;

    fn context(verdict: Verdict) -> Context {
        Context {
            verdict,
            already_finished: false,
            existing_verdict: None,
            current_status: TrialStatus::Unsolved,
            lifecycle: TrialLifecycle::Verified,
            difficulty: Difficulty::Medium,
            revealed_solutions: false,
            existing_solves: vec![],
            existing_experience: 0,
        }
    }

    #[test]
    fn first_accept_solves_scores_and_levels() {
        let plan = plan(&context(Verdict::Accepted), 0);
        assert!(plan.accepted);
        assert!(plan.first_solve);
        assert_eq!(plan.new_status, TrialStatus::Solved);
        assert!(plan.increment_trials_solved);
        assert_eq!(plan.new_rank_score, Some(RankScore(8.0)));
        assert!(plan.new_experience.unwrap() > 0);
        assert!(plan.new_level.is_some());
    }

    #[test]
    fn a_duplicate_report_is_a_no_op_that_echoes_the_recorded_verdict() {
        let mut ctx = context(Verdict::WrongAnswer);
        ctx.already_finished = true;
        ctx.existing_verdict = Some(Verdict::Accepted);
        ctx.current_status = TrialStatus::Solved;

        let plan = plan(&ctx, 5);
        assert!(!plan.accepted, "a second report must not finalise again");
        assert_eq!(plan.recorded_verdict, Verdict::Accepted);
        assert!(!plan.increment_attempts);
        assert!(!plan.increment_trials_solved);
        assert_eq!(plan.new_rank_score, None);
    }

    #[test]
    fn infrastructure_failure_leaves_the_user_untouched() {
        for verdict in [Verdict::JudgeError, Verdict::InternalError] {
            let plan = plan(&context(verdict), 0);
            assert!(plan.accepted, "{verdict:?} is recorded for operators");
            assert_eq!(
                plan.new_status,
                TrialStatus::Unsolved,
                "{verdict:?} must not mark the Trial attempted"
            );
            assert!(!plan.increment_attempts);
            assert!(!plan.increment_trials_solved);
            assert_eq!(plan.new_rank_score, None);
            assert_eq!(plan.new_experience, None);
        }
    }

    #[test]
    fn a_wrong_answer_marks_the_trial_attempted() {
        let plan = plan(&context(Verdict::WrongAnswer), 0);
        assert_eq!(plan.new_status, TrialStatus::Attempted);
        assert!(plan.increment_attempts);
        assert!(!plan.increment_trials_solved);
        assert_eq!(plan.new_rank_score, None);
    }

    #[test]
    fn solving_twice_does_not_double_count() {
        let mut ctx = context(Verdict::Accepted);
        ctx.current_status = TrialStatus::Solved;
        ctx.existing_solves = vec![Solve::verified(Difficulty::Medium)];

        let plan = plan(&ctx, 1);
        assert!(!plan.first_solve);
        assert!(!plan.increment_trials_solved);
        assert_eq!(
            plan.new_rank_score, None,
            "rank must not move on a re-solve"
        );
    }

    #[test]
    fn a_later_wrong_answer_does_not_unsolve_a_trial() {
        let mut ctx = context(Verdict::WrongAnswer);
        ctx.current_status = TrialStatus::Solved;
        assert_eq!(plan(&ctx, 3).new_status, TrialStatus::Solved);
    }

    #[test]
    fn revealed_solutions_forfeit_rank_but_still_solve_and_level() {
        let mut ctx = context(Verdict::Accepted);
        ctx.revealed_solutions = true;

        let plan = plan(&ctx, 0);
        assert!(plan.first_solve);
        assert!(plan.increment_trials_solved);
        assert_eq!(plan.new_rank_score, Some(RankScore(0.0)));
        assert!(plan.new_experience.unwrap() > 0);
    }

    #[test]
    fn beta_trials_solve_without_moving_rank() {
        let mut ctx = context(Verdict::Accepted);
        ctx.lifecycle = TrialLifecycle::Beta;
        let plan = plan(&ctx, 0);
        assert!(plan.increment_trials_solved);
        assert_eq!(plan.new_rank_score, Some(RankScore(0.0)));
    }

    #[test]
    fn rank_is_recomputed_from_the_whole_solve_set() {
        let mut ctx = context(Verdict::Accepted);
        ctx.existing_solves = vec![
            Solve::verified(Difficulty::Intro),
            Solve::verified(Difficulty::Expert),
        ];
        // 1.0 + 35.0 existing, plus 8.0 for this Medium solve.
        assert_eq!(plan(&ctx, 2).new_rank_score, Some(RankScore(44.0)));
    }

    #[test]
    fn milestone_fires_only_when_crossing_a_threshold() {
        let ctx = context(Verdict::Accepted);
        assert!(
            plan(&ctx, 9).milestone_reached,
            "9 -> 10 crosses the first threshold"
        );
        assert!(
            !plan(&ctx, 10).milestone_reached,
            "10 -> 11 crosses nothing"
        );
        assert!(
            plan(&ctx, 49).milestone_reached,
            "49 -> 50 upgrades the badge"
        );
        assert!(!plan(&ctx, 500).milestone_reached);
        assert!(plan(&ctx, 999).milestone_reached, "999 -> 1000");
    }

    #[test]
    fn security_events_count_as_user_attempts() {
        let plan = plan(&context(Verdict::SecurityEvent), 0);
        assert_eq!(plan.new_status, TrialStatus::Attempted);
        assert!(plan.increment_attempts);
    }
}
