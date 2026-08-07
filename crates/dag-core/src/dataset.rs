use datafusion::prelude::{CsvReadOptions, DataFrame, SessionContext};

pub enum BuiltinDataset {
    Iris,
}
pub async fn get_builtin_dataset(dataset_index: BuiltinDataset) -> DataFrame {
    match dataset_index {
        BuiltinDataset::Iris => {
            let ctx = SessionContext::new();
            ctx.read_csv("test_datasets/Iris.csv", CsvReadOptions::new())
                .await
                .unwrap()
        }
    }
}
