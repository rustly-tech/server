//! Community questions and answers.
//!
//! Status: `IMPLEMENTED` for the types and their validation; `PLANNED` for
//! persistence, voting storage, and moderation workflow (P2 epic). The types
//! exist now so that the API contract and the content model can be designed
//! against something real rather than against a guess.

use rustly_common::{Error, Id, Result, Timestamp};
use rustly_domain::user::UserId;
use serde::{Deserialize, Serialize};

/// Type tag for a question id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuestionTag;
/// A question identifier.
pub type QuestionId = Id<QuestionTag>;

/// Type tag for an answer id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnswerTag;
/// An answer identifier.
pub type AnswerId = Id<AnswerTag>;

/// Minimum question title length.
pub const MIN_TITLE: usize = 10;
/// Maximum question title length.
pub const MAX_TITLE: usize = 160;
/// Minimum body length, for questions and answers alike.
pub const MIN_BODY: usize = 20;
/// Maximum body length.
pub const MAX_BODY: usize = 20_000;
/// Maximum tags per question.
pub const MAX_TAGS: usize = 5;

/// A community question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    /// Stable identifier.
    pub id: QuestionId,
    /// Asking user.
    pub author: UserId,
    /// One-line title.
    pub title: String,
    /// Markdown body.
    pub body: String,
    /// Topic tags.
    pub tags: Vec<String>,
    /// Related Trial slug, when the question came from a Trial page.
    pub trial: Option<String>,
    /// Count of "useful" votes.
    pub useful_votes: u32,
    /// Creation time.
    pub created_at: Timestamp,
}

/// An answer to a question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// Stable identifier.
    pub id: AnswerId,
    /// Question being answered.
    pub question: QuestionId,
    /// Answering user.
    pub author: UserId,
    /// Markdown body.
    pub body: String,
    /// Count of "useful" votes.
    pub useful_votes: u32,
    /// Whether the asker marked this answer as resolving the question.
    pub accepted: bool,
    /// Creation time.
    pub created_at: Timestamp,
}

/// Validate a question title.
pub fn validate_title(title: &str) -> Result<()> {
    let trimmed = title.trim();
    let len = trimmed.chars().count();
    if len < MIN_TITLE {
        return Err(Error::invalid(
            "title",
            format!("must be at least {MIN_TITLE} characters"),
        ));
    }
    if len > MAX_TITLE {
        return Err(Error::invalid(
            "title",
            format!("must be at most {MAX_TITLE} characters"),
        ));
    }
    Ok(())
}

/// Validate a question or answer body.
pub fn validate_body(body: &str) -> Result<()> {
    let len = body.trim().chars().count();
    if len < MIN_BODY {
        return Err(Error::invalid(
            "body",
            format!("must be at least {MIN_BODY} characters"),
        ));
    }
    if len > MAX_BODY {
        return Err(Error::invalid(
            "body",
            format!("must be at most {MAX_BODY} characters"),
        ));
    }
    Ok(())
}

/// Validate the tag list.
pub fn validate_tags(tags: &[String]) -> Result<()> {
    if tags.len() > MAX_TAGS {
        return Err(Error::invalid("tags", format!("at most {MAX_TAGS} tags")));
    }
    for tag in tags {
        if tag.is_empty() || tag.len() > 32 {
            return Err(Error::invalid("tags", "each tag must be 1-32 characters"));
        }
        if !tag
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(Error::invalid(
                "tags",
                "tags may contain only a-z, 0-9 and '-'",
            ));
        }
    }
    let mut seen: Vec<&String> = tags.iter().collect();
    seen.sort();
    seen.dedup();
    if seen.len() != tags.len() {
        return Err(Error::invalid("tags", "tags must be distinct"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_must_be_substantial_but_bounded() {
        assert!(validate_title("Why does this move?").is_ok());
        assert!(
            validate_title("why?").is_err(),
            "too short to be answerable"
        );
        assert!(validate_title(&"a".repeat(MAX_TITLE + 1)).is_err());
        assert!(
            validate_title("   why?   ").is_err(),
            "whitespace does not pad a title"
        );
    }

    #[test]
    fn bodies_must_be_substantial_but_bounded() {
        assert!(validate_body(&"x".repeat(MIN_BODY)).is_ok());
        assert!(validate_body("too short").is_err());
        assert!(validate_body(&"x".repeat(MAX_BODY + 1)).is_err());
    }

    #[test]
    fn tags_are_lowercase_distinct_and_few() {
        assert!(validate_tags(&["ownership".into(), "borrow-checker".into()]).is_ok());
        assert!(validate_tags(&[]).is_ok());
        assert!(
            validate_tags(&["Ownership".into()]).is_err(),
            "uppercase rejected"
        );
        assert!(validate_tags(&["a b".into()]).is_err(), "spaces rejected");
        assert!(
            validate_tags(&["x".into(), "x".into()]).is_err(),
            "duplicates rejected"
        );
        let too_many: Vec<String> = (0..=MAX_TAGS).map(|i| format!("t{i}")).collect();
        assert!(validate_tags(&too_many).is_err());
    }

    #[test]
    fn a_question_round_trips_through_json() {
        let question = Question {
            id: QuestionId::new(),
            author: UserId::new(),
            title: "Why does this value move here?".into(),
            body: "I expected a borrow but the compiler says the value moved.".into(),
            tags: vec!["ownership".into()],
            trial: Some("ownership-move-or-borrow".into()),
            useful_votes: 0,
            created_at: Timestamp::now(),
        };
        assert!(validate_title(&question.title).is_ok());
        assert!(validate_body(&question.body).is_ok());
        assert!(validate_tags(&question.tags).is_ok());

        let json = serde_json::to_string(&question).unwrap();
        assert_eq!(serde_json::from_str::<Question>(&json).unwrap(), question);
    }
}
