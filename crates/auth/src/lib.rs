//! Authentication and authorisation.
//!
//! # Token format
//!
//! Tokens are `v1.<base64url(payload)>.<base64url(HMAC-SHA256)>`. The MAC covers
//! the version prefix and the payload, so the format version cannot be stripped
//! or downgraded. Verification is constant-time.
//!
//! This is a stateless bearer token over a signed JSON payload - the same
//! construction as a JWT with a fixed `HS256` algorithm and no algorithm
//! negotiation, which removes the `alg: none` and algorithm-confusion classes of
//! bug entirely.
//!
//! # Two principals, two credential families
//!
//! Users and judge workers authenticate with structurally identical tokens but
//! disjoint scopes. A user token can never lease a job; a worker token can never
//! read a profile. A worker's [`TrustClass`] comes from its **token**, never from
//! the request body, so a worker cannot self-declare itself trusted and be sent
//! hidden tests.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use rustly_common::{Error, Result, Timestamp};
use rustly_domain::user::UserId;
use rustly_protocol::broker::TrustClass;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Token format version. Part of the signed material.
pub const TOKEN_VERSION: &str = "v1";

/// A capability held by a principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Read the caller's own account.
    ReadSelf,
    /// Write the caller's progress checkpoints.
    WriteProgress,
    /// Create submissions.
    Submit,
    /// Moderate community content.
    Moderate,
    /// Lease judge jobs and report results.
    JudgeWork,
}

impl Scope {
    /// The scopes granted to an ordinary signed-in user.
    pub const USER_DEFAULT: [Scope; 3] = [Self::ReadSelf, Self::WriteProgress, Self::Submit];
    /// The scopes granted to a judge worker.
    pub const WORKER_DEFAULT: [Scope; 1] = [Self::JudgeWork];
}

/// The signed token payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// Subject: user id, or worker id for worker tokens.
    pub sub: String,
    /// Principal kind.
    pub kind: PrincipalKind,
    /// Granted scopes.
    pub scopes: Vec<Scope>,
    /// Worker trust class. Only meaningful for worker tokens; the authority for
    /// hidden-test dispatch decisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_class: Option<TrustClass>,
    /// Expiry, unix seconds.
    pub exp: i64,
    /// Issued at, unix seconds.
    pub iat: i64,
}

/// What kind of principal a token names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    /// A human user.
    User,
    /// A judge worker.
    Worker,
}

/// An authenticated caller.
#[derive(Debug, Clone, PartialEq)]
pub enum Principal {
    /// Nobody presented a credential.
    Anonymous,
    /// A signed-in user.
    User {
        /// User identifier.
        id: UserId,
        /// Granted scopes.
        scopes: Vec<Scope>,
    },
    /// A judge worker.
    Worker {
        /// Worker identifier.
        id: String,
        /// Trust class, taken from the token and never from the request body.
        trust_class: TrustClass,
        /// Granted scopes.
        scopes: Vec<Scope>,
    },
}

impl Principal {
    /// Whether the principal holds `scope`.
    pub fn has_scope(&self, scope: Scope) -> bool {
        match self {
            Self::Anonymous => false,
            Self::User { scopes, .. } | Self::Worker { scopes, .. } => scopes.contains(&scope),
        }
    }

    /// Require `scope`, or fail with the right error.
    ///
    /// Anonymous callers get `401` so a client knows to sign in; authenticated
    /// callers missing a scope get `403`, because signing in again will not help.
    pub fn require(&self, scope: Scope) -> Result<()> {
        match self {
            Self::Anonymous => Err(Error::Unauthenticated("credentials required".into())),
            _ if self.has_scope(scope) => Ok(()),
            _ => Err(Error::Forbidden(format!("scope {scope:?} required"))),
        }
    }

    /// The user id, if this principal is a user.
    pub fn user_id(&self) -> Option<UserId> {
        match self {
            Self::User { id, .. } => Some(*id),
            _ => None,
        }
    }

    /// A privacy-safe reference for logs: never the raw user id.
    pub fn log_ref(&self) -> String {
        match self {
            Self::Anonymous => "anon".into(),
            Self::User { id, .. } => {
                let uuid = id.as_uuid().to_string();
                format!("u:{}", &uuid[..8])
            }
            Self::Worker { id, .. } => format!("w:{id}"),
        }
    }
}

/// Issues and verifies tokens.
#[derive(Clone)]
pub struct TokenIssuer {
    secret: Vec<u8>,
}

impl std::fmt::Debug for TokenIssuer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never let the signing key reach a log line through a Debug impl.
        f.debug_struct("TokenIssuer")
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl TokenIssuer {
    /// Minimum acceptable secret length in bytes.
    pub const MIN_SECRET_BYTES: usize = 32;

    /// Construct from a signing secret.
    pub fn new(secret: impl Into<Vec<u8>>) -> Result<Self> {
        let secret = secret.into();
        if secret.len() < Self::MIN_SECRET_BYTES {
            return Err(Error::invalid(
                "token_secret",
                format!("must be at least {} bytes", Self::MIN_SECRET_BYTES),
            ));
        }
        Ok(Self { secret })
    }

    /// Issue a user token valid for `ttl_seconds`.
    pub fn issue_user(
        &self,
        id: UserId,
        scopes: &[Scope],
        now: Timestamp,
        ttl_seconds: i64,
    ) -> String {
        self.sign(&Claims {
            sub: id.to_string(),
            kind: PrincipalKind::User,
            scopes: scopes.to_vec(),
            trust_class: None,
            exp: now.unix_seconds() + ttl_seconds,
            iat: now.unix_seconds(),
        })
    }

    /// Issue a worker token binding a trust class.
    pub fn issue_worker(
        &self,
        worker_id: &str,
        trust_class: TrustClass,
        now: Timestamp,
        ttl_seconds: i64,
    ) -> String {
        self.sign(&Claims {
            sub: worker_id.to_owned(),
            kind: PrincipalKind::Worker,
            scopes: Scope::WORKER_DEFAULT.to_vec(),
            trust_class: Some(trust_class),
            exp: now.unix_seconds() + ttl_seconds,
            iat: now.unix_seconds(),
        })
    }

    fn sign(&self, claims: &Claims) -> String {
        let payload = serde_json::to_vec(claims).expect("claims are always serialisable");
        let encoded = URL_SAFE_NO_PAD.encode(&payload);
        let signing_input = format!("{TOKEN_VERSION}.{encoded}");
        let mac = self.mac(signing_input.as_bytes());
        format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(mac))
    }

    fn mac(&self, data: &[u8]) -> Vec<u8> {
        let mut mac =
            HmacSha256::new_from_slice(&self.secret).expect("HMAC accepts any key length");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }

    /// Verify a token and resolve the principal.
    pub fn verify(&self, token: &str, now: Timestamp) -> Result<Principal> {
        let mut parts = token.split('.');
        let (version, payload, signature) =
            match (parts.next(), parts.next(), parts.next(), parts.next()) {
                (Some(v), Some(p), Some(s), None) => (v, p, s),
                _ => return Err(Error::Unauthenticated("malformed token".into())),
            };
        if version != TOKEN_VERSION {
            return Err(Error::Unauthenticated("unsupported token version".into()));
        }

        let provided = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| Error::Unauthenticated("malformed signature".into()))?;
        let expected = self.mac(format!("{version}.{payload}").as_bytes());
        if provided.ct_eq(&expected).unwrap_u8() != 1 {
            return Err(Error::Unauthenticated("signature mismatch".into()));
        }

        let bytes = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| Error::Unauthenticated("malformed payload".into()))?;
        let claims: Claims = serde_json::from_slice(&bytes)
            .map_err(|_| Error::Unauthenticated("malformed claims".into()))?;

        if claims.exp <= now.unix_seconds() {
            return Err(Error::Unauthenticated("token expired".into()));
        }

        match claims.kind {
            PrincipalKind::User => {
                let id: UserId = claims
                    .sub
                    .parse()
                    .map_err(|_| Error::Unauthenticated("malformed subject".into()))?;
                Ok(Principal::User {
                    id,
                    scopes: claims.scopes,
                })
            }
            PrincipalKind::Worker => {
                let trust_class = claims.trust_class.ok_or_else(|| {
                    Error::Unauthenticated("worker token without a trust class".into())
                })?;
                Ok(Principal::Worker {
                    id: claims.sub,
                    trust_class,
                    scopes: claims.scopes,
                })
            }
        }
    }
}

/// Extract a bearer token from an `Authorization` header value.
pub fn bearer(header: &str) -> Option<&str> {
    let (scheme, token) = header.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issuer() -> TokenIssuer {
        TokenIssuer::new(vec![7u8; 32]).unwrap()
    }

    #[test]
    fn rejects_a_short_secret() {
        assert!(TokenIssuer::new(vec![0u8; 31]).is_err());
        assert!(TokenIssuer::new(vec![0u8; 32]).is_ok());
    }

    #[test]
    fn user_token_round_trips() {
        let issuer = issuer();
        let now = Timestamp::now();
        let id = UserId::new();
        let token = issuer.issue_user(id, &Scope::USER_DEFAULT, now, 3600);

        match issuer.verify(&token, now).unwrap() {
            Principal::User { id: got, scopes } => {
                assert_eq!(got, id);
                assert_eq!(scopes, Scope::USER_DEFAULT.to_vec());
            }
            other => panic!("expected a user principal, got {other:?}"),
        }
    }

    #[test]
    fn expiry_is_enforced() {
        let issuer = issuer();
        let now = Timestamp::now();
        let token = issuer.issue_user(UserId::new(), &Scope::USER_DEFAULT, now, 60);
        assert!(issuer.verify(&token, now.plus_seconds(59)).is_ok());
        let err = issuer.verify(&token, now.plus_seconds(61)).unwrap_err();
        assert_eq!(err.code(), "unauthenticated");
    }

    #[test]
    fn a_tampered_payload_fails_verification() {
        let issuer = issuer();
        let now = Timestamp::now();
        let token = issuer.issue_user(UserId::new(), &Scope::USER_DEFAULT, now, 3600);

        let mut parts: Vec<&str> = token.split('.').collect();
        let forged_claims = Claims {
            sub: UserId::new().to_string(),
            kind: PrincipalKind::User,
            scopes: vec![Scope::Moderate],
            trust_class: None,
            exp: now.unix_seconds() + 3600,
            iat: now.unix_seconds(),
        };
        let forged = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged_claims).unwrap());
        parts[1] = &forged;
        assert!(issuer.verify(&parts.join("."), now).is_err());
    }

    #[test]
    fn a_token_from_another_secret_is_rejected() {
        let now = Timestamp::now();
        let token = TokenIssuer::new(vec![1u8; 32]).unwrap().issue_user(
            UserId::new(),
            &Scope::USER_DEFAULT,
            now,
            3600,
        );
        assert!(issuer().verify(&token, now).is_err());
    }

    #[test]
    fn the_version_prefix_is_covered_by_the_mac() {
        let issuer = issuer();
        let now = Timestamp::now();
        let token = issuer.issue_user(UserId::new(), &Scope::USER_DEFAULT, now, 3600);
        let downgraded = format!("v0{}", token.trim_start_matches("v1"));
        assert!(issuer.verify(&downgraded, now).is_err());
    }

    #[test]
    fn worker_trust_class_comes_from_the_token() {
        let issuer = issuer();
        let now = Timestamp::now();
        let token = issuer.issue_worker("w-7", TrustClass::Volunteer, now, 3600);
        match issuer.verify(&token, now).unwrap() {
            Principal::Worker {
                trust_class,
                id,
                scopes,
            } => {
                assert_eq!(id, "w-7");
                assert_eq!(trust_class, TrustClass::Volunteer);
                assert!(!trust_class.may_receive_hidden_tests());
                assert_eq!(scopes, Scope::WORKER_DEFAULT.to_vec());
            }
            other => panic!("expected a worker principal, got {other:?}"),
        }
    }

    #[test]
    fn user_and_worker_scopes_are_disjoint() {
        let issuer = issuer();
        let now = Timestamp::now();

        let user = issuer
            .verify(
                &issuer.issue_user(UserId::new(), &Scope::USER_DEFAULT, now, 60),
                now,
            )
            .unwrap();
        assert!(!user.has_scope(Scope::JudgeWork));
        assert!(user.require(Scope::JudgeWork).is_err());

        let worker = issuer
            .verify(
                &issuer.issue_worker("w1", TrustClass::Trusted, now, 60),
                now,
            )
            .unwrap();
        assert!(!worker.has_scope(Scope::Submit));
        assert!(!worker.has_scope(Scope::ReadSelf));
    }

    #[test]
    fn anonymous_gets_401_and_scopeless_users_get_403() {
        assert_eq!(
            Principal::Anonymous
                .require(Scope::Submit)
                .unwrap_err()
                .code(),
            "unauthenticated"
        );
        let limited = Principal::User {
            id: UserId::new(),
            scopes: vec![Scope::ReadSelf],
        };
        assert_eq!(
            limited.require(Scope::Submit).unwrap_err().code(),
            "forbidden"
        );
    }

    #[test]
    fn malformed_tokens_are_rejected_not_panicked_on() {
        let issuer = issuer();
        let now = Timestamp::now();
        for bad in ["", ".", "v1", "v1.a", "v1.a.b.c", "v1.!!!.???", "garbage"] {
            assert!(issuer.verify(bad, now).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn log_reference_never_contains_the_full_user_id() {
        let id = UserId::new();
        let principal = Principal::User { id, scopes: vec![] };
        let reference = principal.log_ref();
        assert!(reference.starts_with("u:"));
        assert!(!reference.contains(&id.to_string()));
        assert_eq!(Principal::Anonymous.log_ref(), "anon");
    }

    #[test]
    fn debug_never_prints_the_signing_secret() {
        let text = format!(
            "{:?}",
            TokenIssuer::new(b"super-secret-key-material-32byte".to_vec()).unwrap()
        );
        assert!(!text.contains("super-secret"));
        assert!(text.contains("redacted"));
    }

    #[test]
    fn parses_bearer_headers_case_insensitively() {
        assert_eq!(bearer("Bearer abc"), Some("abc"));
        assert_eq!(bearer("bearer abc"), Some("abc"));
        assert_eq!(bearer("BEARER  abc "), Some("abc"));
        assert_eq!(bearer("Basic abc"), None);
        assert_eq!(bearer("Bearer "), None);
        assert_eq!(bearer("nonsense"), None);
    }
}
