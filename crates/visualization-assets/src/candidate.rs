//! Asset refinement on immutable, version-checked snapshots.
use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use visualization::{
    Store, image,
    proposal::{Proposal, ProposedRevision},
};

#[derive(Clone, Copy)]
pub struct Reference<'a> {
    pub proposal_id: &'a str,
    pub document_sha256: &'a str,
}
impl<'a> Reference<'a> {
    pub fn optional(
        proposal_id: Option<&'a str>,
        document_sha256: Option<&'a str>,
    ) -> Result<Option<Self>> {
        match (proposal_id, document_sha256) {
            (None, None) => Ok(None),
            (Some(id), Some(hash)) if !id.trim().is_empty() && !hash.trim().is_empty() => {
                Ok(Some(Self {
                    proposal_id: id,
                    document_sha256: hash,
                }))
            }
            _ => Err(Error::Invalid(
                "candidate context requires both proposal_id and document_sha256 from read_proposal",
            )),
        }
    }
}
/// Resolve only a pending candidate from this exact current figure revision.
pub fn read(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    reference: Reference<'_>,
) -> Result<Proposal> {
    if store.get(project, figure)?.revision != revision {
        return Err(visualization::Error::Conflict.into());
    }
    let parent = store.proposal(project, figure, reference.proposal_id)?;
    if parent.status != "pending"
        || parent.base_revision != revision
        || parent.document_sha256 != reference.document_sha256
    {
        return Err(visualization::Error::Conflict.into());
    }
    Ok(parent)
}
pub struct Revision<'a> {
    pub reference: Reference<'a>,
    pub summary: &'a str,
    /// Omission must not silently discard the parent's open review links.
    pub markup_ids: Option<&'a [String]>,
}
pub fn revise(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    input: Revision<'_>,
    transform: impl FnOnce(&Value) -> Result<Value>,
) -> Result<Proposal> {
    let parent = read(store, project, figure, revision, input.reference)?;
    if input.markup_ids.is_none() && parent.review_markups.iter().any(|m| m.status == "open") {
        return Err(Error::Invalid(
            "explicit markup_ids required to retain or drop the parent's open review links",
        ));
    }
    let document = transform(&parent.document)?;
    Ok(store.propose_revision(
        project,
        figure,
        revision,
        ProposedRevision {
            parent_id: &parent.id,
            parent_document_sha256: &parent.document_sha256,
            summary: input.summary,
            document: &document,
            markup_ids: input.markup_ids.unwrap_or(&[]),
        },
    )?)
}
pub struct Replacement<'a> {
    pub parent_id: &'a str,
    pub parent_document_sha256: &'a str,
    pub target: &'a str,
    pub image: image::Image<'a>,
    pub summary: &'a str,
    pub markup_ids: Option<&'a [String]>,
}
pub fn replace(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    input: Replacement<'_>,
) -> Result<(Proposal, String)> {
    let mut image_id = None;
    let proposal = revise(
        store,
        project,
        figure,
        revision,
        Revision {
            reference: Reference {
                proposal_id: input.parent_id,
                document_sha256: input.parent_document_sha256,
            },
            summary: input.summary,
            markup_ids: input.markup_ids,
        },
        |doc| {
            let (document, id) = image::illustrate(doc, input.target, input.image)?;
            image_id = Some(id);
            Ok(document)
        },
    )?;
    Ok((proposal, image_id.unwrap()))
}
pub struct Insertion<'a> {
    pub reference: Reference<'a>,
    pub image: image::Image<'a>,
    pub x: f64,
    pub y: f64,
    pub summary: &'a str,
    pub markup_ids: Option<&'a [String]>,
}
/// Append a new standalone asset to the exact pending document, preserving prior work.
pub fn insert(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    input: Insertion<'_>,
) -> Result<(Proposal, String)> {
    let mut image_id = None;
    let proposal = revise(
        store,
        project,
        figure,
        revision,
        Revision {
            reference: input.reference,
            summary: input.summary,
            markup_ids: input.markup_ids,
        },
        |doc| {
            let (document, id) = image::insert(doc, input.image, input.x, input.y)?;
            image_id = Some(id);
            Ok(document)
        },
    )?;
    Ok((proposal, image_id.unwrap()))
}
pub enum Inspection<'a> {
    Palette,
    Parts,
    Geometry { sha256: &'a str, part_id: &'a str },
}
/// Read saved or candidate SVG information without exposing raw SVG/file data URLs.
/// Preview PNGs are returned for the caller to transmit as vision blocks.
pub fn inspect_svg(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    reference: Option<Reference<'_>>,
    target: &str,
    kind: Inspection<'_>,
) -> Result<Value> {
    let parent = reference
        .map(|r| read(store, project, figure, revision, r))
        .transpose()?;
    let saved;
    let document = if let Some(parent) = &parent {
        &parent.document
    } else {
        saved = crate::current(store, project, figure, revision)?;
        &saved
    };
    let mut value = match kind {
        Inspection::Palette => crate::palette_document(document, target)?,
        Inspection::Parts => crate::parts_document(document, target)?,
        Inspection::Geometry { sha256, part_id } => {
            crate::geometry_document(document, target, sha256, part_id)?
        }
    };
    value["revision"] = json!(revision);
    if let Some(parent) = parent {
        value["scope"] = json!("proposal_candidate");
        value["proposal_id"] = json!(parent.id);
        value["document_sha256"] = json!(parent.document_sha256);
        value["base_revision"] = json!(parent.base_revision);
    } else {
        value["scope"] = json!("saved_revision");
    }
    Ok(value)
}
pub enum SvgEdit<'a> {
    Recolor(&'a BTreeMap<String, String>),
    Parts {
        sha256: &'a str,
        edits: &'a [bioicons::parts::Edit],
    },
}
pub struct SvgRefinement<'a> {
    pub reference: Option<Reference<'a>>,
    pub target: &'a str,
    pub edit: SvgEdit<'a>,
    pub markup_ids: Option<&'a [String]>,
}
pub fn refine_svg(
    store: &Store,
    project: &str,
    figure: &str,
    revision: i64,
    input: SvgRefinement<'_>,
) -> Result<Proposal> {
    let summary = match &input.edit {
        SvgEdit::Recolor(_) => "调整矢量素材配色",
        SvgEdit::Parts { .. } => "调整矢量素材局部路径",
    };
    let transform = |doc: &Value| match input.edit {
        SvgEdit::Recolor(colors) => crate::recolor_document(doc, input.target, colors),
        SvgEdit::Parts { sha256, edits } => {
            crate::edit_parts_document(doc, input.target, sha256, edits)
        }
    };
    if let Some(reference) = input.reference {
        revise(
            store,
            project,
            figure,
            revision,
            Revision {
                reference,
                summary,
                markup_ids: input.markup_ids,
            },
            transform,
        )
    } else {
        let document = crate::current(store, project, figure, revision)?;
        let next = transform(&document)?;
        Ok(store.propose_with_markups(
            project,
            figure,
            revision,
            summary,
            &next,
            input.markup_ids.unwrap_or(&[]),
        )?)
    }
}
