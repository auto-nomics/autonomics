//! Reject ambiguous candidate arguments before fetching an external asset.
pub fn validate(
    action: &str,
    proposal_id: Option<&str>,
    document_sha256: Option<&str>,
    markup_ids: Option<&[String]>,
) -> Result<(), &'static str> {
    if !matches!(action, "insert" | "replace")
        && (proposal_id.is_some() || document_sha256.is_some() || markup_ids.is_some())
    {
        return Err(
            "proposal_id, document_sha256 and markup_ids are supported only for replace/insert",
        );
    }
    match (proposal_id, document_sha256) {
        (None, None) => Ok(()),
        (Some(id), Some(hash)) if !id.trim().is_empty() && !hash.trim().is_empty() => Ok(()),
        _ => Err(
            "candidate context requires both proposal_id and document_sha256 from visualization read_proposal",
        ),
    }
}

/// Candidate SVG reads do not accept review decisions; writes must carry them explicitly.
pub fn validate_vector(
    action: &str,
    proposal_id: Option<&str>,
    document_sha256: Option<&str>,
    markup_ids: Option<&[String]>,
) -> Result<(), &'static str> {
    if (proposal_id.is_some() || document_sha256.is_some())
        && !matches!(
            action,
            "insert" | "replace" | "palette" | "parts" | "geometry" | "recolor" | "edit_parts"
        )
    {
        return Err(
            "proposal_id and document_sha256 are supported only for replace/insert/palette/parts/geometry/recolor/edit_parts",
        );
    }
    if markup_ids.is_some() && !matches!(action, "insert" | "replace" | "recolor" | "edit_parts") {
        return Err(
            "markup_ids is supported only for replace/insert/recolor/edit_parts, not read-only SVG inspection",
        );
    }
    match (proposal_id, document_sha256) {
        (None, None) => Ok(()),
        (Some(id), Some(hash)) if !id.trim().is_empty() && !hash.trim().is_empty() => Ok(()),
        _ => Err(
            "candidate context requires both proposal_id and document_sha256 from visualization read_proposal",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn registered_assets_reject_ambiguous_or_ignored_candidate_arguments_before_network() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("figures.sqlite");
        let mut registrations = crate::bioart_registrations(path.clone());
        registrations.extend(crate::bioicons_registrations(path));
        for tool in registrations {
            for input in [
                json!({"action":"insert","proposal_id":"parent"}),
                json!({"action":"insert","document_sha256":"hash"}),
                json!({"action":"insert","proposal_id":"parent","document_sha256":""}),
                json!({"action":"replace","proposal_id":"parent"}),
                json!({"action":"replace","document_sha256":"hash"}),
                json!({"action":"replace","proposal_id":"","document_sha256":"hash"}),
                json!({"action":"preview","proposal_id":"parent","document_sha256":"hash"}),
                json!({"action":"search","query":"receptor","markup_ids":[]}),
            ] {
                let error = tool
                    .implementation
                    .execute(input)
                    .await
                    .unwrap_err()
                    .to_string();
                assert!(
                    error.contains("requires both proposal_id")
                        || error.contains("supported only for replace/insert"),
                    "{error}"
                );
            }
            assert!(tool.implementation.execute(json!({"action":"replace","proposal_id":"parent","document_sha256":"hash","markup_ids":{}})).await.is_err());
        }
        assert!(validate("replace", Some("parent"), Some("hash"), Some(&[])).is_ok());
        assert!(validate("insert", Some("parent"), Some("hash"), Some(&[])).is_ok());
        assert!(validate_vector("insert", Some("parent"), Some("hash"), Some(&[])).is_ok());
        for tool in crate::bioart_registrations(dir.path().join("unused.sqlite"))
            .into_iter()
            .chain(crate::bioicons_registrations(
                dir.path().join("unused.sqlite"),
            ))
        {
            let error = tool.implementation.execute(json!({"action":"insert","element_id":"existing-node","proposal_id":"parent","document_sha256":"hash"})).await.unwrap_err().to_string();
            assert!(error.contains("insert creates a new image"), "{error}");
            let error = tool
                .implementation
                .execute(json!({"action":"replace","x":10}))
                .await
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("replace preserves current placement"),
                "{error}"
            );
        }
    }
}
