//! A read-only canonical projection avoids copying full text into another
//! database table or tokenizing every article synchronously during open.
use crate::Result;
use turso::Connection;

pub(crate) async fn ensure(conn: &Connection, replace: bool) -> Result<()> {
    let mut rows = conn
        .query(
            "SELECT type FROM sqlite_master WHERE name='searchDocuments'",
            (),
        )
        .await?;
    let kind = rows
        .next()
        .await?
        .map(|row| row.get::<String>(0))
        .transpose()?;
    drop(rows);
    if kind.as_deref() == Some("table") {
        conn.execute("DROP TABLE searchDocuments", ()).await?;
    } else if replace && kind.is_some() {
        conn.execute("DROP VIEW searchDocuments", ()).await?;
    }
    conn.execute_batch(format!(
        "CREATE VIEW IF NOT EXISTS searchDocuments AS {}",
        select(None)
    ))
    .await?;
    Ok(())
}

/// Put article constraints inside every UNION branch rather than relying on
/// a backend-specific view predicate push-down optimization.
pub(crate) fn select(article_predicate: Option<&str>) -> String {
    let columns = |anchor: &str, kind: &str, body: &str| {
        format!(
            "SELECT {anchor} || ':{kind}' AS docID,a.id AS articleID,{anchor} AS anchorKey,'{kind}' AS kind, \
         a.title,COALESCE(a.abstract,'') AS abstract,'' AS metadata,{body} AS body, \
         COALESCE((SELECT GROUP_CONCAT(last_name || ' ' || COALESCE(fore_name,''),char(30)) FROM authors WHERE article_id=a.id),'') AS creatorText, \
         COALESCE(a.journal,'') AS publicationText, \
         COALESCE((SELECT GROUP_CONCAT(kind || ' ' || value,char(30)) FROM identifiers WHERE article_id=a.id),'') AS identifierText, \
         COALESCE(a.keywords,'[]') AS keywordText, \
         COALESCE((SELECT GROUP_CONCAT(t.name,char(30)) FROM itemTags it JOIN tags t ON t.tagID=it.tagID WHERE it.itemID=m.itemID),'') AS tagText, \
         COALESCE(CAST(a.year AS TEXT),'') AS yearText, \
         COALESCE((SELECT json_group_array(value) FROM identifiers WHERE article_id=a.id),'[]') AS identifierExact "
        )
    };
    let parent = format!(
        "{} FROM articles a JOIN articleItems m ON m.article_id=a.id JOIN items i ON i.itemID=m.itemID",
        columns("i.key", "item", "''")
    );
    let attachment = format!(
        "{} FROM articles a JOIN articleItems m ON m.article_id=a.id JOIN itemAttachments ia ON ia.parentItemID=m.itemID JOIN items i ON i.itemID=ia.itemID LEFT JOIN fulltextItems f ON f.itemID=i.itemID",
        columns("i.key", "attachment", "COALESCE(f.textContent,'')")
    );
    let annotation = format!(
        "{} FROM articles a JOIN articleItems m ON m.article_id=a.id JOIN annotations n ON n.article_id=a.id LEFT JOIN annotationItems am ON am.annotation_id=n.id LEFT JOIN items i ON i.itemID=am.itemID",
        columns(
            "COALESCE(i.key,n.id)",
            "annotation",
            "n.content || ' ' || n.comment || ' ' || n.tags"
        )
    );
    let predicate = article_predicate
        .map(|predicate| format!(" WHERE {predicate}"))
        .unwrap_or_default();
    format!(
        "{parent}{predicate} UNION ALL {attachment}{predicate} UNION ALL {annotation}{predicate}"
    )
}
