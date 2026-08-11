use crate::config;
use crate::config::IcebergConfig;
use datafusion::prelude::{SessionConfig, SessionContext};
use iceberg::{Catalog, CatalogBuilder, NamespaceIdent, TableIdent, table};
use iceberg_catalog_rest::RestCatalog;
use iceberg_datafusion::IcebergCatalogProvider;
use iceberg_storage_opendal::OpenDalStorageFactory;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::OnceCell;

pub struct Datalake {
    config: config::IcebergConfig,
    catalog: OnceCell<Arc<RestCatalog>>,
}

impl Default for Datalake {
    fn default() -> Self {
        Self {
            config: Default::default(),
            catalog: OnceCell::new(),
        }
    }
}

impl Datalake {
    // TODO: accept customized configuration
    pub fn new() -> Self {
        Self {
            config: IcebergConfig::default(),
            catalog: OnceCell::new(),
        }
    }

    pub async fn get_catalog(&self) -> crate::error::Result<Arc<RestCatalog>> {
        self.catalog.get_or_try_init(|| self.init()).await?;
        Ok(self.catalog.get().unwrap().clone())
    }

    async fn init(&self) -> crate::error::Result<Arc<RestCatalog>> {
        let builder = iceberg_catalog_rest::RestCatalogBuilder::default().with_storage_factory(
            Arc::new(OpenDalStorageFactory::S3 {
                customized_credential_load: None,
            }),
        );
        let catalog = builder
            .load("rest", self.config.to_properties())
            .await
            .map_err(|e| format!("build RestCatalog failed: {e}"))?;

        Ok(Arc::new(catalog))
    }

    pub async fn list_all_tables(&self) -> crate::error::Result<Vec<(Vec<String>, String)>> {
        let catalog = self.get_catalog().await?;
        let mut result = Vec::new();
        // Recursively walk the namespace tree so nested namespaces
        // (e.g. `genetics.ld_score`) are not skipped. Per-namespace errors
        // are tolerated: a single unreadable namespace must not abort the
        // whole listing.
        Self::collect_tables_recursive(&catalog, None, &mut result).await;
        Ok(result)
    }

    /// Recursively collect `(namespace, table)` pairs under `parent`.
    ///
    /// `parent = None` lists the root level. For each namespace we record its
    /// direct tables, then descend into its child namespaces. Errors at any
    /// single namespace/table listing are swallowed so a partial catalog
    /// still yields a (possibly incomplete) result rather than failing.
    async fn collect_tables_recursive(
        catalog: &Arc<RestCatalog>,
        parent: Option<&NamespaceIdent>,
        out: &mut Vec<(Vec<String>, String)>,
    ) {
        // Direct tables at this namespace level.
        if let Some(ns) = parent {
            if let Ok(tables) = catalog.list_tables(ns).await {
                for t in tables {
                    out.push((t.namespace.inner(), t.name));
                }
            }
        }
        // Descend into child namespaces (e.g. `genetics` → `genetics.ld_score`).
        let children = match catalog.list_namespaces(parent).await {
            Ok(c) => c,
            Err(_) => return,
        };
        for child in children {
            // async recursion requires boxing the future.
            Box::pin(Self::collect_tables_recursive(catalog, Some(&child), out)).await;
        }
    }

    pub async fn list_tables_in_namespace(
        &self,
        namespace: &NamespaceIdent,
    ) -> crate::error::Result<Vec<TableIdent>> {
        Ok(self.get_catalog().await?.list_tables(namespace).await?)
    }

    /// Load a table's current schema directly from the Iceberg catalog,
    /// bypassing DataFusion entirely (so multi-level/nested namespaces like
    /// `genetics.ld_score` work — DataFusion's SQL parser caps identifiers
    /// at 3 parts).
    ///
    /// `ident` is a dotted `namespace...table` string (e.g.
    /// `genetics.ld_score.ukbb_eur`); the last segment is the table name,
    /// the rest form the (possibly nested) namespace.
    pub async fn table_schema(
        &self,
        ident: &str,
    ) -> crate::error::Result<Vec<iceberg::spec::NestedField>> {
        let catalog = self.get_catalog().await?;
        let table_ident = parse_table_ident(ident)?;
        let table = catalog.load_table(&table_ident).await?;
        let schema = table.current_schema_ref();
        let fields: Vec<iceberg::spec::NestedField> = schema
            .as_struct()
            .fields()
            .iter()
            .map(|f| (**f).clone())
            .collect();
        Ok(fields)
    }

    pub async fn get_provider(&self) -> crate::error::Result<IcebergCatalogProvider> {
        let rest_catalog = self.get_catalog().await?;

        let catalog_provider: IcebergCatalogProvider =
            IcebergCatalogProvider::try_new(rest_catalog.clone()).await?;
        Ok(catalog_provider)
    }

    pub async fn get_ctx(&self) -> crate::error::Result<SessionContext> {
        let rest_catalog = self.get_catalog().await?;

        let catalog_provider: IcebergCatalogProvider =
            IcebergCatalogProvider::try_new(rest_catalog.clone()).await?;

        let ctx = SessionContext::new();
        // The catalog name "iceberg" matches resource_catalog::CATALOG_NAME;
        // kept as a literal here to avoid pulling resource-catalog into datalake.
        ctx.register_catalog("iceberg", Arc::new(catalog_provider));

        Ok(ctx)
    }

    /// Like [`get_ctx`](Self::get_ctx) but with a custom `target_partitions`
    /// setting.  Use a low value (e.g. 1–2) for large bulk INSERTs to avoid
    /// memory spikes from many concurrent scan partitions.
    pub async fn get_ctx_with_partitions(
        &self,
        partitions: usize,
    ) -> crate::error::Result<SessionContext> {
        let rest_catalog = self.get_catalog().await?;
        let catalog_provider: IcebergCatalogProvider =
            IcebergCatalogProvider::try_new(rest_catalog.clone()).await?;

        let mut config = SessionConfig::new();
        config.options_mut().execution.target_partitions = partitions;
        let ctx = SessionContext::new_with_config(config);
        ctx.register_catalog("iceberg", Arc::new(catalog_provider));
        Ok(ctx)
    }

    // /// Get a new SessionContext with shared runtime_env.
    // ///
    // /// This method provider a better SessionContext that keeps object store and iceberg connected
    // /// while isolate namespace of tables.
    // pub async fn get_iso_ctx(&self) -> crate::error::Result<SessionContext> {
    //     let ctx = SessionContext::new_with_config_rt(SessionConfig::new(), self.ctx.runtime_env());
    //     todo!()
    // }

    pub async fn create_namespace_if_not_exist(
        &self,
        namespace: &NamespaceIdent,
    ) -> crate::error::Result<iceberg::Namespace> {
        let catalog = self.get_catalog().await?;
        let namespace_exist = catalog.namespace_exists(namespace).await?;
        if namespace_exist {
            let result_np: iceberg::Namespace = catalog.get_namespace(namespace).await?;
            Ok(result_np)
        } else {
            let result_np = catalog.create_namespace(namespace, HashMap::new()).await?;
            Ok(result_np)
        }
    }

    pub async fn create_table(
        &self,
        namespace: &NamespaceIdent,
        creation: iceberg::TableCreation,
    ) -> crate::error::Result<iceberg::table::Table> {
        let rest_catalog = self.get_catalog().await?;
        let table: table::Table = rest_catalog.create_table(namespace, creation).await?;
        Ok(table)
    }

    pub async fn create_table_if_not_exist(
        &self,
        namespace: &NamespaceIdent,
        creation: iceberg::TableCreation,
    ) -> crate::error::Result<iceberg::table::Table> {
        let rest_catalog = self.get_catalog().await?;

        // Ensure namespace exist
        self.create_namespace_if_not_exist(namespace).await?;

        let tableident: iceberg::TableIdent =
            iceberg::TableIdent::new(namespace.clone(), creation.name.clone());
        if rest_catalog.table_exists(&tableident).await? {
            Ok(rest_catalog.load_table(&tableident).await?)
        } else {
            let table: table::Table = rest_catalog.create_table(namespace, creation).await?;
            Ok(table)
        }
    }
}

/// Parse a dotted `namespace...table` string into a [`TableIdent`].
///
/// The last dot-separated segment is the table name; the preceding segments
/// form the (possibly nested) namespace. Whitespace-only segments around the
/// dots are trimmed and empty parts are rejected. Requires at least one
/// namespace level plus a table name (≥ 2 parts).
fn parse_table_ident(ident: &str) -> crate::error::Result<TableIdent> {
    TableIdent::from_strs(ident.split('.').map(str::trim))
        .map_err(|e| format!("invalid table identifier {ident:?}: {e}").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_list_namespace() {
        let dk = Datalake::new();
        let namespace = &dk.list_all_tables().await.unwrap();
        println!("{:#?}", namespace);
        dbg!("hi");
    }
}
