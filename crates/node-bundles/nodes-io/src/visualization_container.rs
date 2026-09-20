//! Terminal-only containerized ggplot2 visualization node.
//!
//! Data and constrained plot code enter as ordinary File values so the existing
//! container staging path owns all host/VFS I/O. The plot script is validated
//! before staging; the image then loads plot-ready data as `df`, evaluates the
//! constrained ggplot2 expression, and saves `p` as a PNG. The DAG rejects
//! outgoing edges from this node.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::Arc;

use dag_core::node::DagNode;
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::{NodeInput, NodePorts, dag::DagError, dag::graph::PortOutputs, value::PortType};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::container_command::{
    ContainerCommandNode, ContainerCommandOutputSpec, ContainerCommandSpec,
};
use crate::image_registry::acr_image;
use container_runtime::{PanelCache, PodmanConnection, PullPolicy};

pub const VISUALIZATION_CONTAINER_KIND: &str = "visualization_container";
pub const VISUALIZATION_IMAGE_REPOSITORY: &str = "visualization";
pub const VISUALIZATION_IMAGE_DIGEST: &str =
    "sha256:ee9592b77bc5ea0cebfafafbe39550c377204019f451d7a37e13e4ce2e884f15";

const DEFAULT_ARTIFACT_PREFIX: &str = "/artifacts/visualization_container";
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const DEFAULT_WIDTH: f64 = 8.0;
const DEFAULT_HEIGHT: f64 = 6.0;
const DEFAULT_DPI: f64 = 150.0;
const DEFAULT_CPUS: f64 = 2.0;
const DEFAULT_MEMORY: &str = "2Gi";
const DEFAULT_PIDS_LIMIT: i64 = 256;
const MAX_PLOT_SCRIPT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VisualizationDataFormat {
    Csv,
    Tsv,
    Parquet,
    ArrowStream,
    ArrowFile,
}

impl VisualizationDataFormat {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::Parquet => "parquet",
            Self::ArrowStream => "arrow_stream",
            Self::ArrowFile => "arrow_file",
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct VisualizationContainerSpec {
    pub data_format: VisualizationDataFormat,
    #[serde(default = "default_width")]
    pub width: f64,
    #[serde(default = "default_height")]
    pub height: f64,
    #[serde(default = "default_dpi")]
    pub dpi: f64,
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

fn default_width() -> f64 {
    DEFAULT_WIDTH
}

fn default_height() -> f64 {
    DEFAULT_HEIGHT
}

fn default_dpi() -> f64 {
    DEFAULT_DPI
}

fn default_artifact_prefix() -> String {
    DEFAULT_ARTIFACT_PREFIX.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct VisualizationContainerNodeFactory {
    pub(crate) runtime: Arc<dyn PodmanConnection>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl VisualizationContainerNodeFactory {
    pub fn new(runtime: Arc<dyn PodmanConnection>, panel_cache: Arc<PanelCache>) -> Self {
        Self {
            runtime,
            panel_cache,
        }
    }
}

pub struct VisualizationContainerNode {
    ports: NodePorts,
    inner: Box<dyn DagNode>,
}

impl Clone for VisualizationContainerNode {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            inner: self.inner.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for VisualizationContainerNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        VISUALIZATION_CONTAINER_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let script_input = inputs.iter().find(|input| input.port == 1).ok_or_else(|| {
            DagError::Schedule("visualization_container requires an R script on port 1".into())
        })?;
        let script_file = script_input.file_value().map_err(|error| {
            DagError::Schedule(format!(
                "visualization_container port 1 must be a File: {error}"
            ))
        })?;
        let script = read_plot_script(ctx, script_file).await?;
        validate_plot_script(&script)?;
        self.inner.execute(ctx, inputs, reporter).await
    }

    fn is_terminal(&self) -> bool {
        true
    }
}

pub fn validate(spec: &VisualizationContainerSpec) -> Result<(), String> {
    if !spec.width.is_finite() || spec.width <= 0.0 {
        return Err("width must be finite and greater than zero".into());
    }
    if !spec.height.is_finite() || spec.height <= 0.0 {
        return Err("height must be finite and greater than zero".into());
    }
    if !spec.dpi.is_finite() || spec.dpi <= 0.0 {
        return Err("dpi must be finite and greater than zero".into());
    }
    if !spec.artifact_prefix.starts_with('/') {
        return Err("artifact_prefix must be an absolute VFS path".into());
    }
    if spec.timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".into());
    }
    if let Some(cpus) = spec.cpus
        && (!cpus.is_finite() || cpus <= 0.0)
    {
        return Err("cpus must be finite and greater than zero".into());
    }
    if let Some(limit) = spec.pids_limit
        && limit <= 0
    {
        return Err("pids_limit must be greater than zero".into());
    }
    if let Some(memory) = &spec.memory
        && memory.trim().is_empty()
    {
        return Err("memory cannot be empty".into());
    }
    Ok(())
}

async fn read_plot_script(
    ctx: &NodeCtx,
    file: &dag_core::value::FileRef,
) -> Result<String, DagError> {
    if let Some(fingerprint) = &file.fingerprint
        && fingerprint.size > MAX_PLOT_SCRIPT_BYTES as u64
    {
        return Err(DagError::Schedule(format!(
            "visualization R script is too large: {} bytes (maximum {} bytes)",
            fingerprint.size, MAX_PLOT_SCRIPT_BYTES
        )));
    }

    let bytes = if let Some(virtual_path) = file.path.strip_prefix("vfs://") {
        let storage = ctx.opendal.as_ref().ok_or_else(|| {
            DagError::Schedule(format!(
                "visualization R script `{}` requires a registered runtime VFS",
                file.path
            ))
        })?;
        let key = storage.resolve_path(virtual_path);
        storage
            .resolve(virtual_path)
            .read(&key)
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|error| {
                DagError::Schedule(format!("cannot read R script `{}`: {error}", file.path))
            })?
    } else if Path::new(&file.path).is_absolute() {
        tokio::fs::read(&file.path).await.map_err(|error| {
            DagError::Schedule(format!("cannot read R script `{}`: {error}", file.path))
        })?
    } else {
        return Err(DagError::Schedule(format!(
            "visualization R script path must be a `vfs://` URI or absolute path: `{}`",
            file.path
        )));
    };

    if bytes.len() > MAX_PLOT_SCRIPT_BYTES {
        return Err(DagError::Schedule(format!(
            "visualization R script is too large: {} bytes (maximum {} bytes)",
            bytes.len(),
            MAX_PLOT_SCRIPT_BYTES
        )));
    }
    String::from_utf8(bytes).map_err(|error| {
        DagError::Schedule(format!(
            "visualization R script `{}` is not valid UTF-8: {error}",
            file.path
        ))
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RToken {
    Assignment,
    Plus,
    DoubleColon,
    Identifier(String),
    Number(String),
    StringLiteral(String),
    Comma,
    Equals,
    LeftParen,
    RightParen,
}

fn lex_plot_script(script: &str) -> Result<Vec<RToken>, String> {
    let mut tokens = Vec::new();
    let mut chars = script.chars().peekable();

    while let Some(&ch) = chars.peek() {
        match ch {
            ' ' | '\t' | '\r' | '\n' => {
                chars.next();
            }
            '#' => return Err("comments are not allowed in visualization scripts".into()),
            '<' => {
                chars.next();
                if chars.next() != Some('-') {
                    return Err("only the `<-` assignment is allowed".into());
                }
                tokens.push(RToken::Assignment);
            }
            '+' => {
                chars.next();
                tokens.push(RToken::Plus);
            }
            ':' => {
                chars.next();
                if chars.next() != Some(':') {
                    return Err("single `:` is not allowed in visualization scripts".into());
                }
                tokens.push(RToken::DoubleColon);
            }
            ',' => {
                chars.next();
                tokens.push(RToken::Comma);
            }
            '=' => {
                chars.next();
                tokens.push(RToken::Equals);
            }
            '(' => {
                chars.next();
                tokens.push(RToken::LeftParen);
            }
            ')' => {
                chars.next();
                tokens.push(RToken::RightParen);
            }
            '"' | '\'' => {
                let quote = ch;
                chars.next();
                let mut value = String::new();
                let mut closed = false;
                while let Some(current) = chars.next() {
                    if current == quote {
                        closed = true;
                        break;
                    }
                    if current == '\n' {
                        return Err("unterminated string literal in visualization script".into());
                    }
                    if current == '\\' {
                        value.push(current);
                        if let Some(escaped) = chars.next() {
                            value.push(escaped);
                        }
                        continue;
                    }
                    value.push(current);
                }
                if !closed {
                    return Err("unterminated string literal in visualization script".into());
                }
                tokens.push(RToken::StringLiteral(value));
            }
            '-' | '.' | '0'..='9' => {
                let mut value = String::new();
                if ch == '-' {
                    chars.next();
                    value.push('-');
                    if !chars
                        .peek()
                        .is_some_and(|next| next.is_ascii_digit() || *next == '.')
                    {
                        return Err("minus is only allowed as part of a numeric constant".into());
                    }
                }
                let mut has_digit = false;
                while let Some(&current) = chars.peek() {
                    if current.is_ascii_digit() {
                        has_digit = true;
                        value.push(current);
                        chars.next();
                    } else if current == '.' || current == 'e' || current == 'E' {
                        value.push(current);
                        chars.next();
                    } else if (current == '+' || current == '-') && value.ends_with(['e', 'E']) {
                        value.push(current);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if !has_digit {
                    return Err("invalid numeric constant in visualization script".into());
                }
                tokens.push(RToken::Number(value));
            }
            _ if ch.is_ascii_alphabetic() || ch == '_' => {
                let mut value = String::new();
                while let Some(&current) = chars.peek() {
                    if current.is_ascii_alphanumeric() || current == '_' || current == '.' {
                        value.push(current);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(RToken::Identifier(value));
            }
            other => {
                return Err(format!(
                    "character `{other}` is not allowed in visualization scripts"
                ));
            }
        }
    }

    Ok(tokens)
}

fn is_ggplot_function(name: &str) -> bool {
    matches!(
        name,
        "aes"
            | "aes_"
            | "ggplot"
            | "labs"
            | "ggtitle"
            | "xlab"
            | "ylab"
            | "lims"
            | "xlim"
            | "ylim"
            | "expansion"
            | "sec_axis"
            | "dup_axis"
            | "guide_axis"
            | "guide_legend"
            | "guide_colourbar"
            | "guide_colorbar"
            | "guide_bins"
            | "guide_coloursteps"
            | "guide_colorsteps"
            | "guide_none"
            | "element_blank"
            | "annotation_custom"
            | "annotation_logticks"
            | "annotation_map"
            | "annotation_raster"
            | "annotate"
            | "draft"
    ) || name == "stat_identity"
        || matches!(
            name,
            "geom_point"
                | "geom_jitter"
                | "geom_text"
                | "geom_label"
                | "geom_line"
                | "geom_path"
                | "geom_step"
                | "geom_ribbon"
                | "geom_area"
                | "geom_tile"
                | "geom_rect"
                | "geom_polygon"
                | "geom_segment"
                | "geom_curve"
                | "geom_errorbar"
                | "geom_errorbarh"
                | "geom_crossbar"
                | "geom_linerange"
                | "geom_pointrange"
                | "geom_rug"
                | "geom_raster"
                | "geom_hline"
                | "geom_vline"
                | "geom_abline"
                | "geom_blank"
                | "geom_col"
        )
        || name.starts_with("scale_")
        || name.starts_with("coord_")
        || name.starts_with("facet_")
        || name.starts_with("theme_")
        || name.starts_with("position_")
}

fn is_ggplot_call(tokens: &[RToken]) -> bool {
    tokens.len() >= 3
        && matches!(
            (&tokens[0], &tokens[1], &tokens[2]),
            (
                RToken::Identifier(package),
                RToken::DoubleColon,
                RToken::Identifier(function)
            ) if package == "ggplot2" && is_ggplot_function(function)
        )
}

fn validate_plot_call(tokens: &[RToken]) -> Result<(), String> {
    if tokens.len() < 4 || !is_ggplot_call(tokens) || tokens[3] != RToken::LeftParen {
        return Err("each visualization layer must call an allowlisted ggplot2 function".into());
    }

    let mut depth = 1usize;
    let mut previous: VecDeque<RToken> = VecDeque::with_capacity(3);
    for (index, token) in tokens.iter().enumerate().skip(4) {
        match token {
            RToken::LeftParen => {
                depth += 1;
                let valid_call = previous.len() == 3
                    && is_ggplot_call(&[
                        previous[0].clone(),
                        previous[1].clone(),
                        previous[2].clone(),
                    ]);
                if !valid_call {
                    return Err(
                        "all function calls in visualization scripts must be ggplot2:: functions"
                            .into(),
                    );
                }
            }
            RToken::RightParen => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| "unbalanced parentheses in visualization script".to_string())?;
                if depth == 0 && index != tokens.len() - 1 {
                    return Err(
                        "text follows the closing parenthesis of a visualization layer".into(),
                    );
                }
            }
            RToken::Plus | RToken::Assignment => {
                return Err(format!(
                    "`{}` is only allowed in the top-level plot expression",
                    token_name(token)
                ));
            }
            RToken::Identifier(_)
            | RToken::Number(_)
            | RToken::StringLiteral(_)
            | RToken::Comma
            | RToken::Equals
            | RToken::DoubleColon => {}
        }
        previous.push_back(token.clone());
        if previous.len() > 3 {
            previous.pop_front();
        }
    }
    if depth != 0 {
        return Err("unbalanced parentheses in visualization script".into());
    }

    Ok(())
}

fn token_name(token: &RToken) -> &'static str {
    match token {
        RToken::Assignment => "<-",
        RToken::Plus => "+",
        RToken::DoubleColon => "::",
        RToken::Identifier(_) => "identifier",
        RToken::Number(_) => "number",
        RToken::StringLiteral(_) => "string",
        RToken::Comma => ",",
        RToken::Equals => "=",
        RToken::LeftParen => "(",
        RToken::RightParen => ")",
    }
}

pub fn validate_plot_script(script: &str) -> Result<(), DagError> {
    if script.trim().is_empty() {
        return Err(DagError::Schedule(
            "visualization R script cannot be empty".into(),
        ));
    }
    let tokens = lex_plot_script(script).map_err(DagError::Schedule)?;
    if tokens.len() < 3
        || tokens[0] != RToken::Identifier("p".into())
        || tokens[1] != RToken::Assignment
    {
        return Err(DagError::Schedule(
            "visualization script must be exactly `p <- ggplot2::...`".into(),
        ));
    }

    let mut start = 2;
    let mut paren_depth = 0usize;
    for index in 2..tokens.len() {
        match tokens[index] {
            RToken::LeftParen => paren_depth += 1,
            RToken::RightParen => {
                paren_depth = paren_depth.saturating_sub(1);
            }
            RToken::Plus if paren_depth == 0 => {
                let layer = &tokens[start..index];
                if layer.is_empty() {
                    return Err(DagError::Schedule(
                        "visualization script cannot contain an empty layer".into(),
                    ));
                }
                validate_plot_call(layer).map_err(DagError::Schedule)?;
                start = index + 1;
            }
            _ => {}
        }
    }
    let last = &tokens[start..];
    if last.is_empty() {
        return Err(DagError::Schedule(
            "visualization script cannot contain an empty layer".into(),
        ));
    }
    validate_plot_call(last).map_err(DagError::Schedule)
}

pub fn container_spec(spec: &VisualizationContainerSpec) -> Result<ContainerCommandSpec, String> {
    validate(spec)?;
    let env = BTreeMap::from([
        (
            "AUTONOMICS_VISUALIZATION_DATA_FORMAT".into(),
            spec.data_format.as_label().into(),
        ),
        (
            "AUTONOMICS_VISUALIZATION_WIDTH".into(),
            spec.width.to_string(),
        ),
        (
            "AUTONOMICS_VISUALIZATION_HEIGHT".into(),
            spec.height.to_string(),
        ),
        ("AUTONOMICS_VISUALIZATION_DPI".into(), spec.dpi.to_string()),
    ]);

    Ok(ContainerCommandSpec {
        image: acr_image(VISUALIZATION_IMAGE_REPOSITORY, VISUALIZATION_IMAGE_DIGEST)?,
        command: vec!["Rscript".into(), "/opt/autonomics/render.R".into()],
        script: None,
        files: Default::default(),
        env,
        outputs: vec![ContainerCommandOutputSpec {
            path: "plot.png".into(),
            format: Some("png".into()),
        }],
        workdir: None,
        artifact_prefix: spec.artifact_prefix.clone(),
        timeout_secs: spec.timeout_secs,
        panels: Vec::new(),
        panel_bundles: Vec::new(),
        network: "isolated".into(),
        read_only_rootfs: true,
        pull_policy: PullPolicy::Missing,
        cpus: Some(spec.cpus.unwrap_or(DEFAULT_CPUS)),
        memory: Some(spec.memory.clone().unwrap_or_else(|| DEFAULT_MEMORY.into())),
        pids_limit: Some(spec.pids_limit.unwrap_or(DEFAULT_PIDS_LIMIT)),
        shm_size: None,
        gpus: None,
        user: None,
    })
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port_of_type_with_label(None, PortType::File, "data")
        .add_input_port_of_type_with_label(None, PortType::File, "r_script")
        .add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for VisualizationContainerNodeFactory {
    fn kind(&self) -> &'static str {
        VISUALIZATION_CONTAINER_KIND
    }

    fn desc(&self) -> &'static str {
        "Terminal-only renderer for plot-ready data and a constrained ggplot2 script."
    }

    fn doc(&self) -> &'static str {
        "Terminal-only plot renderer in an isolated R/ggplot2 OCI container. \
        Input port 0 must already contain plot-ready data (`csv`, `tsv`, \
        `parquet`, Arrow IPC stream, or Arrow IPC file). Input port 1 is a \
        small R script containing exactly `p <- ggplot2::... + ggplot2::...`; \
        filtering, aggregation, modeling, indexing, file I/O, arbitrary \
        functions, and computation inside aesthetics are rejected before the \
        container starts. The node cannot have downstream DAG edges. The \
        fixed entrypoint renders `p` to immutable `plot.png` with networking \
        disabled, a read-only root filesystem, and CPU, memory, PID, and \
        runtime limits."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(VisualizationContainerSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: VisualizationContainerSpec = serde_json::from_value(spec)?;
        let container_spec =
            container_spec(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        let runtime: Arc<dyn container_runtime::PodmanConnection> = self.runtime.clone();
        let node =
            ContainerCommandNode::new(container_spec, runtime, Arc::clone(&self.panel_cache))
                .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(VisualizationContainerNode {
            ports: port_layout(),
            inner: Box::new(node),
        }))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: VisualizationContainerSpec = serde_json::from_value(spec)?;
        validate(&spec).map_err(dag_core::registry::error::Error::Unknown)?;
        Ok(port_layout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> VisualizationContainerSpec {
        VisualizationContainerSpec {
            data_format: VisualizationDataFormat::Csv,
            width: default_width(),
            height: default_height(),
            dpi: default_dpi(),
            artifact_prefix: default_artifact_prefix(),
            timeout_secs: default_timeout_secs(),
            cpus: None,
            memory: None,
            pids_limit: None,
        }
    }

    #[test]
    fn builds_an_isolated_file_to_png_contract() {
        let container = container_spec(&spec()).unwrap();

        assert_eq!(
            container.image,
            acr_image(VISUALIZATION_IMAGE_REPOSITORY, VISUALIZATION_IMAGE_DIGEST).unwrap()
        );
        assert_eq!(container.network, "isolated");
        assert!(container.read_only_rootfs);
        assert_eq!(
            container.command,
            vec![
                "Rscript".to_string(),
                "/opt/autonomics/render.R".to_string()
            ]
        );
        assert_eq!(container.outputs.len(), 1);
        assert_eq!(container.outputs[0].path, "plot.png");
        assert_eq!(
            container.env.get("AUTONOMICS_VISUALIZATION_DATA_FORMAT"),
            Some(&"csv".to_string())
        );
        assert_eq!(container.cpus, Some(DEFAULT_CPUS));
        assert_eq!(container.memory.as_deref(), Some(DEFAULT_MEMORY));
        assert_eq!(container.pids_limit, Some(DEFAULT_PIDS_LIMIT));
    }

    #[test]
    fn rejects_nonpositive_figure_dimensions() {
        let mut value = spec();
        value.width = 0.0;
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.height = f64::NAN;
        assert!(validate(&value).is_err());

        let mut value = spec();
        value.dpi = -1.0;
        assert!(validate(&value).is_err());
    }

    #[test]
    fn accepts_only_a_single_constrained_ggplot_expression() {
        if let Err(error) = validate_plot_script(
            "p <- ggplot2::ggplot(df, ggplot2::aes(x, y)) + ggplot2::geom_point()",
        ) {
            panic!("valid ggplot script was rejected: {error}");
        }
        assert!(validate_plot_script(
            "p <- ggplot2::ggplot(df, ggplot2::aes(x = beta, y = se)) + ggplot2::geom_point(size = 1.5)"
        )
        .is_ok());

        assert!(
            validate_plot_script(
                "p <- ggplot2::ggplot(df) + ggplot2::geom_point(); write.csv(df, '/tmp/x')"
            )
            .is_err()
        );
        assert!(
            validate_plot_script(
                "p <- ggplot2::ggplot(df, ggplot2::aes(x, log10(y))) + ggplot2::geom_point()"
            )
            .is_err()
        );
        assert!(
            validate_plot_script("p <- ggplot2::ggplot(df, ggplot2::aes(df$x, df$y))").is_err()
        );
        assert!(
            validate_plot_script(
                "p <- ggplot2::ggplot(df, ggplot2::aes(x, y)) + ggplot2::geom_histogram()"
            )
            .is_err()
        );
        assert!(
            validate_plot_script(
                "p <- ggplot2::ggplot(df, ggplot2::aes(x, y)) + ggplot2::stat_bin()"
            )
            .is_err()
        );
        assert!(validate_plot_script("# plot\np <- ggplot2::ggplot(df)").is_err());
        assert!(validate_plot_script("x <- 1\np <- ggplot2::ggplot(df)").is_err());
    }
}
