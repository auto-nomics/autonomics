use std::collections::HashSet;

use scraper::{Html, Selector};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dataset {
    pub tisch_id: String,
    pub dataset: String,
    pub species: String,
    pub treatment: String,
    pub patients: Option<u64>,
    pub cells: Option<u64>,
    pub platform: String,
    pub primary_or_metastatic: String,
    pub pubmed_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CellType {
    pub abbreviation: String,
    pub label: String,
}

#[derive(Debug, Clone, Default)]
pub struct DatasetFilter {
    pub query: Option<String>,
    pub species: Option<String>,
    pub treatment: Option<String>,
    pub dataset: Option<String>,
    pub max_results: Option<usize>,
}

pub fn parse_datasets(html: &str) -> Result<Vec<Dataset>, crate::error::TischError> {
    let document = Html::parse_document(html);
    let row_selector = Selector::parse("#dataset-table tbody tr")
        .map_err(|error| crate::error::TischError::Parse(error.to_string()))?;
    let cell_selector = Selector::parse("td")
        .map_err(|error| crate::error::TischError::Parse(error.to_string()))?;
    let mut datasets = Vec::new();
    let mut seen = HashSet::new();

    for row in document.select(&row_selector) {
        let cells: Vec<String> = row
            .select(&cell_selector)
            .map(|cell| clean_text(&cell.text().collect::<String>()))
            .collect();
        if cells.len() < 10 {
            continue;
        }
        let dataset = Dataset {
            tisch_id: cells[1].clone(),
            dataset: cells[2].clone(),
            species: cells[3].clone(),
            treatment: cells[4].clone(),
            patients: parse_count(&cells[5]),
            cells: parse_count(&cells[6]),
            platform: cells[7].clone(),
            primary_or_metastatic: cells[8].clone(),
            pubmed_id: cells[9].clone(),
        };
        if dataset.dataset.is_empty() || !seen.insert(dataset.dataset.clone()) {
            continue;
        }
        datasets.push(dataset);
    }
    Ok(datasets)
}

pub fn parse_cell_types(html: &str) -> Result<Vec<CellType>, crate::error::TischError> {
    let document = Html::parse_document(html);
    let row_selector = Selector::parse("#celltype_table tbody tr")
        .map_err(|error| crate::error::TischError::Parse(error.to_string()))?;
    let cell_selector = Selector::parse("td")
        .map_err(|error| crate::error::TischError::Parse(error.to_string()))?;
    let mut cell_types = Vec::new();
    for row in document.select(&row_selector) {
        let cells: Vec<String> = row
            .select(&cell_selector)
            .map(|cell| clean_text(&cell.text().collect::<String>()))
            .collect();
        if cells.len() == 2 && !cells[0].is_empty() {
            cell_types.push(CellType {
                abbreviation: cells[0].clone(),
                label: cells[1].clone(),
            });
        }
    }
    Ok(cell_types)
}

pub fn filter_datasets(datasets: &[Dataset], filter: &DatasetFilter) -> Vec<Dataset> {
    let query = normalized(&filter.query);
    let species = normalized(&filter.species);
    let treatment = normalized(&filter.treatment);
    let dataset = normalized(&filter.dataset);
    let mut result: Vec<Dataset> = datasets
        .iter()
        .filter(|row| {
            species
                .as_ref()
                .is_none_or(|value| row.species.to_ascii_lowercase().contains(value))
                && treatment
                    .as_ref()
                    .is_none_or(|value| row.treatment.to_ascii_lowercase().contains(value))
                && dataset
                    .as_ref()
                    .is_none_or(|value| row.dataset.to_ascii_lowercase().contains(value))
                && query.as_ref().is_none_or(|value| {
                    let haystack = format!(
                        "{} {} {} {} {} {} {}",
                        row.tisch_id,
                        row.dataset,
                        row.species,
                        row.treatment,
                        row.platform,
                        row.primary_or_metastatic,
                        row.pubmed_id
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

pub fn dataset_assets(base_url: &str, dataset: &str) -> (String, String) {
    let prefix = base_url.trim_end_matches('/');
    (
        format!("{prefix}/static/data/{dataset}/{dataset}_Data.zip"),
        format!("{prefix}/static/data/{dataset}/{dataset}_umap.png"),
    )
}

fn clean_text(value: &str) -> String {
    value.replace('\u{a0}', " ").trim().to_owned()
}

fn parse_count(value: &str) -> Option<u64> {
    value.replace(',', "").trim().parse().ok()
}

fn normalized(value: &Option<String>) -> Option<String> {
    let value = value.as_deref()?.trim();
    (!value.is_empty()).then(|| value.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dataset_rows_and_filters() {
        let html = r#"
            <table id="dataset-table"><tbody>
                <tr><td><input value="AEL_GSE142213"></td><td>T010001</td><td>AEL_GSE142213</td><td>Human</td><td>None</td><td>2</td><td>3,994</td><td>10x Genomics</td><td>Primary</td><td><a>32330454</a></td></tr>
            </tbody></table>
        "#;
        let datasets = parse_datasets(html).unwrap();
        assert_eq!(datasets.len(), 1);
        assert_eq!(datasets[0].cells, Some(3_994));
        let filtered = filter_datasets(
            &datasets,
            &DatasetFilter {
                query: Some("AEL".into()),
                species: Some("human".into()),
                ..Default::default()
            },
        );
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn parses_cell_type_rows() {
        let html = r#"
            <table id="celltype_table"><tbody>
                <tr><td>B</td><td>B Cells</td></tr>
            </tbody></table>
        "#;
        let cell_types = parse_cell_types(html).unwrap();
        assert_eq!(cell_types[0].label, "B Cells");
    }
}
