//! Compilation tools — compile document to PDF.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::WritingToolState;
use super::citation_tools::PreviewTexTool;
use crate::compile::{LatexEngine, compile_document};
use crate::store::WritingStore;

pub fn registrations(state: WritingToolState) -> Vec<ToolRegistration> {
    let mut regs = Vec::new();

    // doc_preview_tex — always available
    regs.push(ToolRegistration::from(PreviewTexTool {
        store: state.store.clone(),
    }));

    // doc_compile — only if engine is available
    if let Some(engine) = &state.engine {
        regs.push(ToolRegistration::from(DocCompileTool {
            store: state.store.clone(),
            engine: engine.clone(),
            bib: state.bib.clone(),
        }));
    }

    regs
}

// ===========================================================================
// doc_compile
// ===========================================================================

#[tool(
    name = "doc_compile",
    description = "Compile a document to PDF using the configured LaTeX engine (XeLaTeX or pdflatex). \
                  Automatically resolves citations and generates the .bib file. \
                  Returns whether compilation succeeded, page count, and any errors/warnings. \
                  The PDF is NOT returned inline — use the compilation status to check success."
)]
pub struct DocCompileInput {
    #[desc = "Document ID"]
    pub document_id: String,
}

struct DocCompileTool {
    store: Arc<WritingStore>,
    engine: Arc<dyn LatexEngine>,
    bib: Option<Arc<bib_base::BibBase>>,
}

#[async_trait]
impl ToolFunction for DocCompileTool {
    type Input = DocCompileInput;

    fn timeout_seconds(&self) -> u64 {
        120 // LaTeX compilation can be slow
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let doc = self
            .store
            .get_document(&input.document_id)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: Box::new(e),
            })?;

        // Build resolver if bib is available.
        let resolver = self
            .bib
            .as_ref()
            .map(|bib| crate::citation::CitationResolver::new(bib.clone()));

        let output = compile_document(&doc, resolver.as_ref(), self.engine.as_ref())
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: Box::new(e),
            })?;

        let errors: Vec<_> = output
            .errors
            .iter()
            .map(|e| {
                serde_json::json!({
                    "message": e.message,
                    "line": e.line,
                    "file": e.file,
                })
            })
            .collect();

        let warnings: Vec<_> = output
            .warnings
            .iter()
            .map(|w| serde_json::json!({ "message": w.message }))
            .collect();

        Ok(AgentToolResult::success_json(serde_json::json!({
            "success": output.success,
            "engine": output.engine_name,
            "pages": output.pages,
            "pdf_size": output.pdf_bytes.as_ref().map(|b| b.len()),
            "error_count": errors.len(),
            "warning_count": warnings.len(),
            "errors": errors,
            "warnings": warnings,
        })))
    }
}
