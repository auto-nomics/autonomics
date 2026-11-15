//! Recoverable deletion and compare-and-swap metadata updates.
use crate::bib_base::{BibCore, sync_search_index};
use crate::{BibBase, Error, Result};
use bib_types::Article;
use sha2::{Digest, Sha256};
use turso::{
    Connection,
    transaction::{Transaction, TransactionBehavior},
};

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS articleTrash (articleID TEXT PRIMARY KEY REFERENCES articles(id) ON DELETE CASCADE, deletedAt TEXT NOT NULL); CREATE TABLE IF NOT EXISTS articleExtraFields (articleID TEXT PRIMARY KEY REFERENCES articles(id) ON DELETE CASCADE, data TEXT NOT NULL);";

/// Metadata content token. Every writer, including agents/imports, changes this
/// token when metadata changes; timestamps intentionally are not part of it.
pub fn article_revision(article: &Article) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(article).expect("Article is serializable"))
    )
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TrashedArticle {
    pub article: Article,
    pub deleted_at: String,
}

pub(crate) async fn is_trashed(conn: &Connection, id: &str) -> Result<bool> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM articleTrash WHERE articleID=?1",
            turso::params![id],
        )
        .await?;
    Ok(rows.next().await?.is_some())
}

pub(crate) async fn ensure_identifiers_available(conn: &Connection, article: &Article) -> Result<()> {
    for identifier in &article.identifiers {
        // Arbitrary identifiers include URLs whose path components are case-sensitive.
        let collation = if identifier.kind == bib_types::IdKind::Other {
            "BINARY"
        } else {
            "NOCASE"
        };
        let mut rows = conn.query(format!("SELECT article_id FROM identifiers WHERE kind=?1 AND value=?2 COLLATE {collation} AND article_id<>?3 LIMIT 1"),turso::params![identifier.kind.as_str(), identifier.value.trim(), article.id.as_str()]).await?;
        if let Some(row) = rows.next().await? {
            return Err(Error::MetadataConflict(format!(
                "Identifier {}:{} belongs to record {} (possibly in trash).",
                identifier.kind.as_str(),
                identifier.value,
                row.get::<String>(0)?
            )));
        }
    }
    Ok(())
}

impl BibCore {
    pub(crate) async fn create_manual_article_checked(
        &self,
        mut article: Article,
    ) -> Result<Article> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            article.id = uuid::Uuid::new_v4().to_string();
            article.created_at = Some(chrono::Utc::now());
            article.updated_at = article.created_at;
            ensure_identifiers_available(&tx, &article).await?;
            Self::upsert_article_in_tx(&tx, &article).await?;
            crate::zotero_model::ensure_article_item(&tx, &article.id).await?;
            crate::zotero_model::sync_article_item(&tx, &article.id).await?;
            sync_search_index(&tx, &article.id).await?;
            Self::get_article_on(&tx, &article.id)
                .await?
                .ok_or_else(|| Error::NotFound(article.id.clone()))
        }
        .await;
        match result {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }

    pub(crate) async fn update_article_checked(
        &self,
        mut article: Article,
        expected: Option<String>,
    ) -> Result<Article> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            if is_trashed(&tx, &article.id).await? {
                return Err(Error::NotFound(article.id.clone()));
            }
            let current = Self::get_article_on(&tx, &article.id)
                .await?
                .ok_or_else(|| Error::NotFound(article.id.clone()))?;
            if expected
                .as_ref()
                .is_some_and(|token| *token != article_revision(&current))
            {
                return Err(Error::MetadataConflict(
                    "This record changed after editing began. Reload and review your changes."
                        .into(),
                ));
            }
            if article.title.trim().is_empty() {
                return Err(Error::Unknown("title cannot be empty".into()));
            }
            ensure_identifiers_available(&tx, &article).await?;
            article.created_at = current.created_at;
            article.updated_at = Some(chrono::Utc::now());
            Self::upsert_article_in_tx(&tx, &article).await?;
            crate::zotero_model::ensure_article_item(&tx, &article.id).await?;
            crate::zotero_model::sync_article_item(&tx, &article.id).await?;
            sync_search_index(&tx, &article.id).await?;
            // Return canonical storage order (identifiers are keyed by kind).
            Self::get_article_on(&tx, &article.id)
                .await?
                .ok_or_else(|| Error::NotFound(article.id.clone()))
        }
        .await;
        match result {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }
    pub(crate) async fn set_article_trash(&self, id: &str, trashed: bool) -> Result<bool> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result = async {
            if Self::get_article_on(&tx, id).await?.is_none() {
                return Err(Error::NotFound(id.into()));
            }
            let changed = if trashed {
                tx.execute(
                    "INSERT OR IGNORE INTO articleTrash(articleID,deletedAt) VALUES (?1,?2)",
                    turso::params![id, chrono::Utc::now().to_rfc3339()],
                )
                .await?
            } else {
                tx.execute(
                    "DELETE FROM articleTrash WHERE articleID=?1",
                    turso::params![id],
                )
                .await?
            };
            if changed > 0 {
                sync_search_index(&tx, id).await?;
            }
            Ok(changed > 0)
        }
        .await;
        match result {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }
    pub(crate) async fn list_article_trash(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<TrashedArticle>, u64)> {
        let conn = self.conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Deferred).await?;
        let result = async {
            let mut rows = tx.query("SELECT COUNT(*) FROM articleTrash", ()).await?;
            let total = rows.next().await?.expect("count").get::<i64>(0)? as u64;
            drop(rows);
            let mut rows = tx.query("SELECT articleID,deletedAt FROM articleTrash ORDER BY deletedAt DESC,articleID LIMIT ?1 OFFSET ?2", turso::params![limit.min(500) as i64,offset as i64]).await?;
            let mut ids = Vec::new();
            while let Some(row) = rows.next().await? { ids.push((row.get::<String>(0)?,row.get::<String>(1)?)); }
            drop(rows);
            let mut articles = Vec::new();
            for (id,deleted_at) in ids { if let Some(article) = Self::get_article_on(&tx,&id).await? { articles.push(TrashedArticle{article,deleted_at}); } }
            Ok((articles,total))
        }.await;
        match result {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }
}

impl BibBase {
    pub async fn create_manual_article_checked(&self, article: Article) -> Result<Article> {
        self.call(|reply| crate::actor::BibCommand::CreateManualArticleChecked { article, reply })
            .await
    }

    pub async fn erase_trashed_article(&self, id: &str) -> Result<u64> {
        self.call(|reply| crate::actor::BibCommand::EraseTrashedArticle {
            id: id.into(),
            reply,
        })
        .await
    }
    pub async fn update_article_checked(
        &self,
        article: Article,
        expected_revision: Option<String>,
    ) -> Result<Article> {
        self.call(|reply| crate::actor::BibCommand::UpdateArticleChecked {
            article,
            expected_revision,
            reply,
        })
        .await
    }
    pub async fn set_article_trash(&self, id: &str, trashed: bool) -> Result<bool> {
        self.call(|reply| crate::actor::BibCommand::SetArticleTrash {
            id: id.into(),
            trashed,
            reply,
        })
        .await
    }
    pub async fn list_article_trash(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<TrashedArticle>, u64)> {
        self.call(|reply| crate::actor::BibCommand::ListArticleTrash {
            limit,
            offset,
            reply,
        })
        .await
    }
}
