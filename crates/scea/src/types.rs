use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Experiment {
    #[serde(rename = "experimentAccession")]
    pub accession: String,
    #[serde(rename = "experimentDescription")]
    pub description: String,
    pub species: String,
    pub kingdom: String,
    #[serde(rename = "loadDate")]
    pub load_date: String,
    #[serde(rename = "lastUpdate")]
    pub last_update: String,
    #[serde(rename = "rawExperimentType")]
    pub raw_experiment_type: String,
    #[serde(rename = "technologyType")]
    pub technology_type: Vec<String>,
    #[serde(rename = "numberOfAssays")]
    pub number_of_assays: Option<u64>,
    #[serde(rename = "experimentalFactors")]
    pub experimental_factors: Vec<String>,
    #[serde(rename = "experimentProjects")]
    pub experiment_projects: Vec<String>,
    #[serde(rename = "experimentType")]
    pub experiment_type: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExperimentIndex {
    #[serde(default)]
    pub experiments: Vec<Experiment>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MarkerGene {
    pub x: u32,
    pub y: u32,
    #[serde(rename = "geneName")]
    pub gene_name: String,
    pub value: Option<f64>,
    #[serde(rename = "cellGroupValue")]
    pub cell_group_value: String,
    #[serde(rename = "cellGroupValueWhereMarker")]
    pub cell_group_value_where_marker: String,
    #[serde(rename = "pValue")]
    pub p_value: Option<f64>,
    #[serde(rename = "expressionUnit")]
    pub expression_unit: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ExperimentPageTab {
    #[serde(rename = "type")]
    pub tab_type: String,
    pub name: Option<String>,
    pub props: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ExperimentPageContent {
    #[serde(rename = "experimentAccession")]
    pub accession: String,
    #[serde(rename = "accessKey")]
    pub access_key: String,
    pub species: String,
    pub disclaimer: String,
    #[serde(default)]
    pub tabs: Vec<ExperimentPageTab>,
}

#[derive(Debug, Clone, Default)]
pub struct ExperimentFilter {
    pub query: Option<String>,
    pub species: Option<String>,
    pub kingdom: Option<String>,
    pub experiment_type: Option<String>,
    pub max_results: Option<usize>,
}

pub fn filter_experiments(
    experiments: &[Experiment],
    filter: &ExperimentFilter,
) -> Vec<Experiment> {
    let query = normalized(&filter.query);
    let species = normalized(&filter.species);
    let kingdom = normalized(&filter.kingdom);
    let experiment_type = normalized(&filter.experiment_type);
    let mut result: Vec<Experiment> = experiments
        .iter()
        .filter(|experiment| {
            species
                .as_ref()
                .is_none_or(|value| experiment.species.to_ascii_lowercase().contains(value))
                && kingdom
                    .as_ref()
                    .is_none_or(|value| experiment.kingdom.eq_ignore_ascii_case(value))
                && experiment_type.as_ref().is_none_or(|value| {
                    experiment
                        .experiment_type
                        .to_ascii_lowercase()
                        .contains(value)
                })
                && query.as_ref().is_none_or(|value| {
                    let haystack = format!(
                        "{} {} {} {}",
                        experiment.accession,
                        experiment.description,
                        experiment.species,
                        experiment.raw_experiment_type
                    )
                    .to_ascii_lowercase();
                    haystack.contains(value)
                })
        })
        .cloned()
        .collect();
    if let Some(max_results) = filter.max_results {
        result.truncate(max_results.max(1));
    }
    result
}

fn normalized(value: &Option<String>) -> Option<String> {
    let value = value.as_deref()?.trim();
    (!value.is_empty()).then(|| value.to_ascii_lowercase())
}
