//! The PostgreSQL backend must satisfy the same conformance suite as the
//! in-memory backend.
//!
//! Skipped unless `RUSTLY_TEST_DATABASE_URL` is set, so `cargo test` stays
//! hermetic on a laptop with no database. CI sets it against a `postgres:17`
//! service container, which is where this actually runs.

#![cfg(feature = "postgres")]

use rustly_storage::postgres::PostgresStore;

#[tokio::test]
async fn postgres_store_satisfies_the_conformance_suite() {
    let Ok(url) = std::env::var("RUSTLY_TEST_DATABASE_URL") else {
        eprintln!("skipping: RUSTLY_TEST_DATABASE_URL is not set");
        return;
    };

    let store = PostgresStore::connect(&url, 8)
        .await
        .expect("connect to the test database");
    store.migrate().await.expect("apply migrations");
    rustly_storage::conformance::run_suite(&store).await;
}
