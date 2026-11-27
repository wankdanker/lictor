//! The in-memory store must pass the same suite as every other backend.
lictor_store::store_conformance_tests!(async { Some(lictor_store::MemoryStore::new()) });
