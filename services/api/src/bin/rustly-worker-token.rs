//! Issue a signed credential for an operator-managed judge worker.
//!
//! The command reads the same signing secret as the API and writes only the
//! resulting token to stdout, so it can be piped directly into a secret store.

use anyhow::{bail, Context as _};
use rustly_auth::TokenIssuer;
use rustly_common::Timestamp;
use rustly_protocol::broker::TrustClass;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let worker_id = args.next().context(
        "usage: rustly-worker-token <worker-id> <volunteer|community|trusted> [ttl-seconds]",
    )?;
    let trust = match args.next().as_deref() {
        Some("volunteer") => TrustClass::Volunteer,
        Some("community") => TrustClass::Community,
        Some("trusted") => TrustClass::Trusted,
        _ => bail!(
            "usage: rustly-worker-token <worker-id> <volunteer|community|trusted> [ttl-seconds]"
        ),
    };
    let ttl = args
        .next()
        .map(|value| {
            value
                .parse::<i64>()
                .context("ttl-seconds must be an integer")
        })
        .transpose()?
        .unwrap_or(30 * 24 * 60 * 60);
    if ttl <= 0 || args.next().is_some() {
        bail!("usage: rustly-worker-token <worker-id> <volunteer|community|trusted> [ttl-seconds]");
    }

    let secret = std::env::var("RUSTLY_TOKEN_SECRET")
        .context("RUSTLY_TOKEN_SECRET must match the API signing secret")?;
    let issuer = TokenIssuer::new(secret.into_bytes()).context("invalid token signing secret")?;
    println!(
        "{}",
        issuer.issue_worker(&worker_id, trust, Timestamp::now(), ttl)
    );
    Ok(())
}
