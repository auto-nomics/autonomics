//! Shared helpers for the service-module unit tests.

#![cfg(test)]

use uuid::Uuid;

use crate::Storage;
use crate::language::Language;
use crate::storage::types::Nomenclature;

use super::KmsService;

/// Build a Chinese-language nomenclature for entity fixtures.
pub(crate) fn make_name(full: &str) -> Nomenclature {
    Nomenclature {
        id: Uuid::new_v4(),
        lang: Language::ZH,
        full: full.to_string(),
        abbr: None,
    }
}

/// Open a `KmsService` backed by an in-memory Turso database.
pub(crate) async fn setup() -> KmsService {
    let storage = Storage::open_in_memory().await.unwrap();
    KmsService::from_storage(storage).await.unwrap()
}
