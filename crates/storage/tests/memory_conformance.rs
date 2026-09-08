//! The in-memory backend must satisfy the same conformance suite as PostgreSQL.

use rustly_storage::memory::MemoryStore;

#[tokio::test]
async fn memory_store_satisfies_the_conformance_suite() {
    let store = MemoryStore::new();
    rustly_storage::conformance::run_suite(&store).await;
}
