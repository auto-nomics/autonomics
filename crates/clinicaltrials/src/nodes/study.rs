use std::sync::Arc;

use arrow_array::{Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::ClinicalTrialsClient;
use crate::types::Study;

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ClinicalTrialsStudySpec {
    /// NCT identifier.
    pub nct_id: String,
}

#[derive(Clone)]
pub struct ClinicalTrialsStudyNode {
    meta: NodePorts,
    spec: ClinicalTrialsStudySpec,
}

pub struct ClinicalTrialsStudyNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for ClinicalTrialsStudyNodeFactory {
    fn kind(&self) -> &'static str {
        "source_clinicaltrials_study"
    }

    fn desc(&self) -> &'static str {
        "Fetch one ClinicalTrials.gov study as a table."
    }

    fn doc(&self) -> &'static str {
        "A zero-input source node that resolves one ClinicalTrials.gov study. Output columns: \
        nct_id, brief_title, official_title, overall_status, start_date, completion_date, \
        lead_sponsor, organization, study_type, phases, enrollment, conditions, keywords, \
        interventions, primary_outcomes, brief_summary."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ClinicalTrialsStudySpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec = serde_json::from_value::<ClinicalTrialsStudySpec>(spec)?;
        Ok(Box::new(ClinicalTrialsStudyNode {
            meta: port_layout(),
            spec,
        }))
    }
}

#[async_trait]
impl DagNode for ClinicalTrialsStudyNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "source_clinicaltrials_study"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let study = ClinicalTrialsClient::new()
            .study(&self.spec.nct_id)
            .await
            .map_err(|error| {
                DagError::Schedule(format!("ClinicalTrials request failed: {error}"))
            })?;
        let batch = record_batch(&study)
            .map_err(|error| DagError::Schedule(format!("ClinicalTrials batch failed: {error}")))?;
        let dataframe = ctx
            .session()
            .read_batch(batch)
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        let mut output = PortOutputs::new();
        output.insert(0, dataframe);
        Ok(output)
    }
}

fn join(values: &[String], separator: &str) -> Option<String> {
    (!values.is_empty()).then(|| values.join(separator))
}

fn record_batch(study: &Study) -> Result<RecordBatch, arrow_schema::ArrowError> {
    let protocol = &study.protocol_section;
    let identification = &protocol.identification_module;
    let status = &protocol.status_module;
    let schema = Arc::new(Schema::new(vec![
        Field::new("nct_id", DataType::Utf8, true),
        Field::new("brief_title", DataType::Utf8, true),
        Field::new("official_title", DataType::Utf8, true),
        Field::new("overall_status", DataType::Utf8, true),
        Field::new("start_date", DataType::Utf8, true),
        Field::new("completion_date", DataType::Utf8, true),
        Field::new("lead_sponsor", DataType::Utf8, true),
        Field::new("organization", DataType::Utf8, true),
        Field::new("study_type", DataType::Utf8, true),
        Field::new("phases", DataType::Utf8, true),
        Field::new("enrollment", DataType::UInt64, true),
        Field::new("conditions", DataType::Utf8, true),
        Field::new("keywords", DataType::Utf8, true),
        Field::new("interventions", DataType::Utf8, true),
        Field::new("primary_outcomes", DataType::Utf8, true),
        Field::new("brief_summary", DataType::Utf8, true),
    ]));

    let interventions = protocol
        .arms_interventions_module
        .interventions
        .iter()
        .map(|intervention| {
            format!(
                "{}:{}",
                intervention
                    .intervention_type
                    .as_deref()
                    .unwrap_or("INTERVENTION"),
                intervention.name.as_deref().unwrap_or("unknown")
            )
        })
        .collect::<Vec<_>>();
    let primary_outcomes = protocol
        .outcomes_module
        .primary_outcomes
        .iter()
        .filter_map(|outcome| outcome.measure.clone())
        .collect::<Vec<_>>();
    let columns = vec![
        Arc::new(StringArray::from(vec![identification.nct_id.as_deref()])) as Arc<dyn Array>,
        Arc::new(StringArray::from(vec![
            identification.brief_title.as_deref(),
        ])),
        Arc::new(StringArray::from(vec![
            identification.official_title.as_deref(),
        ])),
        Arc::new(StringArray::from(vec![status.overall_status.as_deref()])),
        Arc::new(StringArray::from(vec![
            status
                .start_date_struct
                .as_ref()
                .and_then(|date| date.date.as_deref()),
        ])),
        Arc::new(StringArray::from(vec![
            status
                .completion_date_struct
                .as_ref()
                .and_then(|date| date.date.as_deref()),
        ])),
        Arc::new(StringArray::from(vec![
            protocol
                .sponsor_collaborators_module
                .lead_sponsor
                .as_ref()
                .and_then(|sponsor| sponsor.name.as_deref()),
        ])),
        Arc::new(StringArray::from(vec![
            identification
                .organization
                .as_ref()
                .and_then(|organization| organization.full_name.as_deref()),
        ])),
        Arc::new(StringArray::from(vec![
            protocol.design_module.study_type.as_deref(),
        ])),
        Arc::new(StringArray::from(vec![
            join(&protocol.design_module.phases, ";").as_deref(),
        ])),
        Arc::new(UInt64Array::from(vec![
            protocol
                .design_module
                .enrollment_info
                .as_ref()
                .and_then(|enrollment| enrollment.count),
        ])),
        Arc::new(StringArray::from(vec![
            join(&protocol.conditions_module.conditions, ";").as_deref(),
        ])),
        Arc::new(StringArray::from(vec![
            join(&protocol.conditions_module.keywords, ";").as_deref(),
        ])),
        Arc::new(StringArray::from(vec![
            join(&interventions, ";").as_deref(),
        ])),
        Arc::new(StringArray::from(vec![
            join(&primary_outcomes, ";").as_deref(),
        ])),
        Arc::new(StringArray::from(vec![
            protocol.description_module.brief_summary.as_deref(),
        ])),
    ];
    RecordBatch::try_new(schema, columns)
}
