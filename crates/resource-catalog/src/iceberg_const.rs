//! The single source of truth for the Iceberg catalog name registered in
//! every `SessionContext`. Before this crate existed, the literal `"iceberg"`
//! was hardcoded at >=8 call sites. Import this constant everywhere a catalog
//! is registered or referenced by name.

/// The catalog name under which the Iceberg catalog is registered in
/// DataFusion `SessionContext`s across the workspace.
pub const CATALOG_NAME: &str = "iceberg";
