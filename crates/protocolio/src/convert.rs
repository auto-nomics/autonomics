use std::sync::Arc;

use arrow_array::{
    Array, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use serde_json::Value;

use crate::types::{Protocol, ProtocolStep, ProtocolSummary, Reagent};

pub(crate) fn ordered_steps(steps: &[ProtocolStep]) -> Vec<ProtocolStep> {
    if steps.is_empty() {
        return Vec::new();
    }

    let mut indexes = std::collections::HashMap::new();
    for (index, step) in steps.iter().enumerate() {
        if let Some(id) = step.id {
            indexes.insert(id, index);
        }
    }

    let mut next = std::collections::HashMap::new();
    let mut has_parent = vec![false; steps.len()];
    for (index, step) in steps.iter().enumerate() {
        let parent = step
            .previous_id
            .filter(|id| *id != 0)
            .and_then(|id| indexes.get(&id).copied());
        if let Some(parent) = parent {
            next.insert(parent, index);
            has_parent[index] = true;
        }
    }

    let roots = (0..steps.len()).filter(|index| !has_parent[*index]);
    let mut order = Vec::with_capacity(steps.len());
    let mut seen = vec![false; steps.len()];
    for mut index in roots {
        let mut guard = 0;
        while guard <= steps.len() && !seen[index] {
            seen[index] = true;
            guard += 1;
            order.push(index);
            match next.get(&index).copied() {
                Some(next_index) if !seen[next_index] => index = next_index,
                _ => break,
            }
        }
    }

    if order.len() != steps.len() {
        return steps.to_vec();
    }
    order
        .into_iter()
        .map(|index| steps[index].clone())
        .collect()
}

pub fn protocol_summaries(
    items: &[ProtocolSummary],
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let schema = Arc::new(Schema::new(vec![
        field("protocol_id", DataType::Int64),
        field("guid", DataType::Utf8),
        field("title", DataType::Utf8),
        field("doi", DataType::Utf8),
        field("uri", DataType::Utf8),
        field("url", DataType::Utf8),
        field("version_id", DataType::UInt64),
        field("published_on_unix", DataType::Int64),
        field("created_on_unix", DataType::Int64),
        field("creator_name", DataType::Utf8),
        field("creator_username", DataType::Utf8),
        field("authors", DataType::Utf8),
        field("number_of_steps", DataType::UInt64),
        field("is_public", DataType::Boolean),
        field("total_views", DataType::UInt64),
    ]));

    let mut columns: Vec<Arc<dyn Array>> = Vec::new();
    macro_rules! push {
        ($array:expr) => {
            columns.push(Arc::new($array))
        };
    }
    push!(Int64Array::from(
        items.iter().map(|item| item.id).collect::<Vec<_>>()
    ));
    push!(StringArray::from(
        items.iter().map(|x| x.guid.as_deref()).collect::<Vec<_>>()
    ));
    push!(StringArray::from(
        items.iter().map(|x| x.title.as_deref()).collect::<Vec<_>>()
    ));
    push!(StringArray::from(
        items.iter().map(|x| x.doi.as_deref()).collect::<Vec<_>>()
    ));
    push!(StringArray::from(
        items.iter().map(|x| x.uri.as_deref()).collect::<Vec<_>>()
    ));
    push!(StringArray::from(
        items.iter().map(|x| x.url.as_deref()).collect::<Vec<_>>()
    ));
    push!(UInt64Array::from(
        items
            .iter()
            .map(|x| x.version_id.map(u64::from))
            .collect::<Vec<_>>()
    ));
    push!(Int64Array::from(
        items.iter().map(|x| x.published_on).collect::<Vec<_>>()
    ));
    push!(Int64Array::from(
        items.iter().map(|x| x.created_on).collect::<Vec<_>>()
    ));
    push!(StringArray::from(
        items
            .iter()
            .map(|x| x.creator.as_ref().and_then(|x| x.name.as_deref()))
            .collect::<Vec<_>>()
    ));
    push!(StringArray::from(
        items
            .iter()
            .map(|x| x.creator.as_ref().and_then(|x| x.username.as_deref()))
            .collect::<Vec<_>>()
    ));
    push!(StringArray::from(
        items
            .iter()
            .map(|x| user_list(&x.authors))
            .collect::<Vec<_>>()
    ));
    push!(UInt64Array::from(
        items.iter().map(step_count).collect::<Vec<_>>()
    ));
    push!(BooleanArray::from(
        items
            .iter()
            .map(|x| public_flag(x.public.as_ref()))
            .collect::<Vec<_>>()
    ));
    push!(UInt64Array::from(
        items
            .iter()
            .map(|x| x.stats.as_ref().and_then(|x| x.number_of_views))
            .collect::<Vec<_>>()
    ));
    RecordBatch::try_new(schema, columns)
}

pub fn protocol(item: &Protocol) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let summary = &item.summary;
    let schema = Arc::new(Schema::new(vec![
        field("protocol_id", DataType::Int64),
        field("guid", DataType::Utf8),
        field("title", DataType::Utf8),
        field("doi", DataType::Utf8),
        field("uri", DataType::Utf8),
        field("url", DataType::Utf8),
        field("version_id", DataType::UInt64),
        field("version_uri", DataType::Utf8),
        field("published_on_unix", DataType::Int64),
        field("created_on_unix", DataType::Int64),
        field("creator_name", DataType::Utf8),
        field("creator_username", DataType::Utf8),
        field("authors", DataType::Utf8),
        field("is_public", DataType::Boolean),
        field("description", DataType::Utf8),
        field("before_start", DataType::Utf8),
        field("guidelines", DataType::Utf8),
        field("warning", DataType::Utf8),
        field("materials_text", DataType::Utf8),
        field("number_of_steps", DataType::UInt64),
        field("number_of_views", DataType::UInt64),
        field("number_of_reagents", DataType::UInt64),
        field("number_of_equipments", DataType::UInt64),
    ]));
    let columns: Vec<Arc<dyn Array>> = vec![
        Arc::new(Int64Array::from(vec![summary.id])),
        Arc::new(StringArray::from(vec![summary.guid.as_deref()])),
        Arc::new(StringArray::from(vec![summary.title.as_deref()])),
        Arc::new(StringArray::from(vec![summary.doi.as_deref()])),
        Arc::new(StringArray::from(vec![summary.uri.as_deref()])),
        Arc::new(StringArray::from(vec![summary.url.as_deref()])),
        Arc::new(UInt64Array::from(vec![summary.version_id.map(u64::from)])),
        Arc::new(StringArray::from(vec![summary.version_uri.as_deref()])),
        Arc::new(Int64Array::from(vec![summary.published_on])),
        Arc::new(Int64Array::from(vec![summary.created_on])),
        Arc::new(StringArray::from(vec![
            summary.creator.as_ref().and_then(|x| x.name.as_deref()),
        ])),
        Arc::new(StringArray::from(vec![
            summary.creator.as_ref().and_then(|x| x.username.as_deref()),
        ])),
        Arc::new(StringArray::from(vec![Some(user_list(&summary.authors))])),
        Arc::new(BooleanArray::from(vec![public_flag(
            summary.public.as_ref(),
        )])),
        Arc::new(StringArray::from(vec![rich_text(
            item.description.as_ref(),
        )])),
        Arc::new(StringArray::from(vec![rich_text(
            item.before_start.as_ref(),
        )])),
        Arc::new(StringArray::from(vec![rich_text(item.guidelines.as_ref())])),
        Arc::new(StringArray::from(vec![rich_text(item.warning.as_ref())])),
        Arc::new(StringArray::from(vec![rich_text(
            item.materials_text.as_ref(),
        )])),
        Arc::new(UInt64Array::from(vec![step_count(summary)])),
        Arc::new(UInt64Array::from(vec![
            summary.stats.as_ref().and_then(|x| x.number_of_views),
        ])),
        Arc::new(UInt64Array::from(vec![
            summary.stats.as_ref().and_then(|x| x.number_of_reagents),
        ])),
        Arc::new(UInt64Array::from(vec![
            summary.stats.as_ref().and_then(|x| x.number_of_equipments),
        ])),
    ];
    RecordBatch::try_new(schema, columns)
}

pub fn steps(
    identifier: &str,
    version_id: Option<u32>,
    steps: &[ProtocolStep],
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let ordered = ordered_steps(steps);
    let schema = Arc::new(Schema::new(vec![
        field("protocol_identifier", DataType::Utf8),
        field("protocol_version_id", DataType::UInt64),
        field("step_position", DataType::UInt64),
        field("step_id", DataType::Int64),
        field("step_guid", DataType::Utf8),
        field("previous_step_id", DataType::Int64),
        field("previous_step_guid", DataType::Utf8),
        field("section", DataType::Utf8),
        field("section_color", DataType::Utf8),
        field("body_markdown", DataType::Utf8),
        field("components_json", DataType::Utf8),
        field("modified_on_unix", DataType::Int64),
    ]));
    let mut columns: Vec<Arc<dyn Array>> = vec![
        Arc::new(StringArray::from(vec![Some(identifier); ordered.len()])),
        Arc::new(UInt64Array::from(vec![
            version_id.map(u64::from);
            ordered.len()
        ])),
    ];
    let mut position = Vec::with_capacity(ordered.len());
    for (index, _) in ordered.iter().enumerate() {
        position.push(Some(index as u64 + 1));
    }
    columns.push(Arc::new(UInt64Array::from(position)));
    columns.extend([
        Arc::new(Int64Array::from(
            ordered.iter().map(|x| x.id).collect::<Vec<_>>(),
        )) as Arc<dyn Array>,
        Arc::new(StringArray::from(
            ordered
                .iter()
                .map(|x| x.guid.as_deref())
                .collect::<Vec<_>>(),
        )),
        Arc::new(Int64Array::from(
            ordered.iter().map(|x| x.previous_id).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            ordered
                .iter()
                .map(|x| x.previous_guid.as_deref())
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            ordered
                .iter()
                .map(|x| rich_text(x.section.as_ref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            ordered
                .iter()
                .map(|x| x.section_color.as_deref())
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            ordered
                .iter()
                .map(|x| rich_text(x.step.as_ref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            ordered
                .iter()
                .map(|x| json_text(x.components.as_ref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(Int64Array::from(
            ordered.iter().map(|x| x.modified_on).collect::<Vec<_>>(),
        )),
    ]);
    RecordBatch::try_new(schema, columns)
}

pub fn materials(
    identifier: &str,
    items: &[Reagent],
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    reagent_rows(identifier, items, true)
}

pub fn reagents(items: &[Reagent]) -> Result<RecordBatch, arrow_schema::ArrowError> {
    reagent_rows("", items, false)
}

fn reagent_rows(
    identifier: &str,
    items: &[Reagent],
    with_protocol: bool,
) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let mut fields = Vec::new();
    if with_protocol {
        fields.push(field("protocol_identifier", DataType::Utf8));
    }
    fields.extend([
        field("material_id", DataType::Int64),
        field("name", DataType::Utf8),
        field("sku", DataType::Utf8),
        field("vendor_name", DataType::Utf8),
        field("vendor_url", DataType::Utf8),
        field("product_url", DataType::Utf8),
        field("linear_formula", DataType::Utf8),
        field("mol_weight", DataType::Float64),
        field("is_citeab", DataType::Boolean),
    ]);
    let schema: SchemaRef = Arc::new(Schema::new(fields));
    let mut columns: Vec<Arc<dyn Array>> = Vec::new();
    if with_protocol {
        columns.push(Arc::new(StringArray::from(vec![
            Some(identifier);
            items.len()
        ])));
    }
    columns.extend([
        Arc::new(Int64Array::from(
            items.iter().map(|x| x.id).collect::<Vec<_>>(),
        )) as Arc<dyn Array>,
        Arc::new(StringArray::from(
            items.iter().map(|x| x.name.as_deref()).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            items.iter().map(|x| x.sku.as_deref()).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            items
                .iter()
                .map(|x| x.vendor.as_ref().and_then(|x| x.name.as_deref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            items
                .iter()
                .map(|x| x.vendor.as_ref().and_then(|x| x.link.as_deref()))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            items.iter().map(|x| x.url.as_deref()).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            items
                .iter()
                .map(|x| x.linfor.as_deref())
                .collect::<Vec<_>>(),
        )),
        Arc::new(Float64Array::from(
            items.iter().map(|x| x.mol_weight).collect::<Vec<_>>(),
        )),
        Arc::new(BooleanArray::from(
            items.iter().map(|x| x.is_citeab).collect::<Vec<_>>(),
        )),
    ]);
    RecordBatch::try_new(schema, columns)
}

fn field(name: &str, data_type: DataType) -> Field {
    Field::new(name, data_type, true)
}

fn user_list(users: &[crate::types::User]) -> String {
    users
        .iter()
        .filter_map(|user| user.label())
        .collect::<Vec<_>>()
        .join("; ")
}

fn step_count(summary: &ProtocolSummary) -> Option<u64> {
    summary.number_of_steps.or(summary
        .stats
        .as_ref()
        .and_then(|stats| stats.number_of_steps))
}

fn public_flag(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => value.as_i64().map(|value| value != 0),
        Value::String(value) => Some(value == "1" || value.eq_ignore_ascii_case("true")),
        _ => None,
    }
}

fn rich_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => Some(value.clone()),
        Value::Null => None,
        value => Some(value.to_string()),
    }
}

fn json_text(value: Option<&Value>) -> Option<String> {
    value.map(|value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(id: i64, previous: Option<i64>) -> ProtocolStep {
        ProtocolStep {
            id: Some(id),
            previous_id: previous,
            step: Some(json!("step")),
            ..Default::default()
        }
    }

    #[test]
    fn reconstructs_step_order() {
        let steps = vec![step(3, Some(2)), step(1, None), step(2, Some(1))];
        let ordered = ordered_steps(&steps);
        let ids: Vec<_> = ordered.iter().filter_map(|step| step.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn falls_back_when_links_form_a_cycle() {
        let steps = vec![step(2, Some(1)), step(1, Some(2))];
        let ordered = ordered_steps(&steps);
        assert_eq!(ordered.len(), 2);
    }
}
