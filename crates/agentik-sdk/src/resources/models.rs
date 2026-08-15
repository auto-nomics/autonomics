use crate::client::Anthropic;
use crate::types::{AnthropicError, ModelList, ModelListParams, ModelObject, Result};

/// Resource for managing models
pub struct ModelsResource<'a> {
    client: &'a Anthropic,
}

impl<'a> ModelsResource<'a> {
    pub(crate) fn new(client: &'a Anthropic) -> Self {
        Self { client }
    }

    /// List all available models with pagination support
    ///
    /// # Arguments
    ///
    /// * `params` - Optional pagination parameters
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use agentik_sdk::{Anthropic, ModelListParams};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = Anthropic::new("test-api-key", "https://api.example.com")?;
    ///
    /// // List all models
    /// let models = client.models().list(None).await?;
    /// println!("Found {} models", models.data.len());
    ///
    /// // List with pagination
    /// let params = ModelListParams::new().limit(10);
    /// let models = client.models().list(Some(params)).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list(&self, params: Option<ModelListParams>) -> Result<ModelList> {
        let mut query_params = Vec::new();

        if let Some(params) = params {
            if let Some(before_id) = params.before_id {
                query_params.push(("before_id", before_id));
            }
            if let Some(after_id) = params.after_id {
                query_params.push(("after_id", after_id));
            }
            if let Some(limit) = params.limit {
                query_params.push(("limit", limit.to_string()));
            }
        }

        let url = format!("{}/v1/models", self.client.config().base_url);
        let request = self
            .client
            .http_client()
            .get(&url)
            .query(&query_params)
            .build()
            .map_err(|e| AnthropicError::Connection {
                message: e.to_string(),
            })?;
        let response = self.client.http_client().send(request).await?;

        if response.status().is_success() {
            let model_list: ModelList = response.json().await?;
            Ok(model_list)
        } else {
            let status = response.status().as_u16();
            let error_text = response.text().await?;
            Err(AnthropicError::from_status(status, error_text))
        }
    }

    /// Get a specific model by ID or alias
    ///
    /// # Arguments
    ///
    /// * `model_id` - Model identifier or alias (e.g., "claude-3-5-sonnet-latest")
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use agentik_sdk::Anthropic;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = Anthropic::new("test-api-key", "https://api.example.com")?;
    ///
    /// // Get specific model
    /// let model = client.models().get("claude-3-5-sonnet-latest").await?;
    /// println!("Model: {} ({})", model.display_name, model.id);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get(&self, model_id: &str) -> Result<ModelObject> {
        let url = format!("{}/v1/models/{}", self.client.config().base_url, model_id);
        let request = self.client.http_client().get(&url).build().map_err(|e| {
            AnthropicError::Connection {
                message: e.to_string(),
            }
        })?;
        let response = self.client.http_client().send(request).await?;

        if response.status().is_success() {
            let model: ModelObject = response.json().await?;
            Ok(model)
        } else {
            let status = response.status().as_u16();
            let error_text = response.text().await?;
            Err(AnthropicError::from_status(status, error_text))
        }
    }

    /// List models by family (e.g., "claude-3", "claude-3-5")
    ///
    /// # Arguments
    ///
    /// * `family` - Model family to filter by
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use agentik_sdk::Anthropic;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = Anthropic::new("test-api-key", "https://api.example.com")?;
    ///
    /// let claude35_models = client.models().list_by_family("claude-3-5").await?;
    /// println!("Found {} Claude 3.5 models", claude35_models.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_by_family(&self, family: &str) -> Result<Vec<ModelObject>> {
        let all_models = self.list(None).await?;
        let filtered_models = all_models
            .data
            .into_iter()
            .filter(|model| model.is_family(family))
            .collect();

        Ok(filtered_models)
    }
}
