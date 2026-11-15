//! Source-first LaTeX projects persisted using the versioned VFS directory protocol.
use chrono::Utc;
use serde::{Deserialize, Serialize};
mod persistence;
use std::sync::Arc;
use uuid::Uuid;
use vfs::OpendalFileStorage;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("record not found")]
    NotFound,
    #[error("revision conflict; reload before saving")]
    Conflict,
    #[error(transparent)]
    Storage(#[from] opendal::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
fn now() -> String {
    Utc::now().to_rfc3339()
}
fn uid() -> String {
    Uuid::new_v4().to_string()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub title: String,
    pub main_file: String,
    pub article_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct File {
    pub path: String,
    pub content: String,
    pub revision: u64,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Revision {
    pub id: String,
    pub path: String,
    pub content: String,
    pub revision: u64,
    pub actor: String,
    pub message: String,
    pub created_at: String,
    #[serde(default)]
    pub deleted: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub path: String,
    pub base_revision: u64,
    pub original: String,
    pub proposed: String,
    pub status: String,
    pub author: String,
    pub summary: String,
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub body: String,
    pub author: String,
    pub resolved: bool,
    pub created_at: String,
    pub revision: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub path: String,
    pub revision: u64,
    pub byte_size: usize,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDetail {
    pub project: Project,
    #[serde(default)]
    pub assets: Vec<Asset>,
    pub files: Vec<File>,
    pub proposals: Vec<Proposal>,
    pub comments: Vec<Comment>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stored {
    #[serde(flatten)]
    detail: ProjectDetail,
    history: Vec<Revision>,
    #[serde(default)]
    asset_data: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    asset_versions: std::collections::BTreeMap<String, u64>,
}
#[derive(Debug, Default, Deserialize)]
pub struct ProjectPatch {
    pub title: Option<String>,
    pub main_file: Option<String>,
    pub article_ids: Option<Vec<String>>,
}
#[derive(Clone)]
pub struct ManuscriptStore {
    storage: Arc<OpendalFileStorage>,
    root: String,
    lock: std::sync::Arc<tokio::sync::Mutex<()>>,
}

/// Portable, relative project paths, safe for compiler materialization on every platform.
pub fn validate_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 240
        || path.contains(['\\', ':'])
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(Error::Invalid(
            "expected a safe relative project file path".into(),
        ));
    }
    Ok(())
}
fn validate_content(content: &str) -> Result<()> {
    if content.len() > 4 * 1024 * 1024 {
        return Err(Error::Invalid("source file exceeds 4 MiB".into()));
    }
    Ok(())
}
fn valid_title(title: &str) -> Result<()> {
    if title.trim().is_empty() || title.len() > 500 {
        return Err(Error::Invalid("title must contain 1–500 bytes".into()));
    }
    Ok(())
}
const TEMPLATE: &str = "\\documentclass{article}\n\\usepackage[utf8]{inputenc}\n\\usepackage{graphicx}\n\\title{Untitled manuscript}\n\\author{}\n\\begin{document}\n\\maketitle\n\\begin{abstract}\nWrite your abstract here.\n\\end{abstract}\n\\section{Introduction}\nStart writing here.\n\\bibliographystyle{plain}\n\\bibliography{references}\n\\end{document}\n";
impl ManuscriptStore {
    pub async fn create_project(&self, title: &str) -> Result<Project> {
        self.create_project_as(title, "human").await
    }
    pub async fn create_project_as(&self, title: &str, actor: &str) -> Result<Project> {
        valid_title(title)?;
        let time = now();
        let project = Project {
            id: uid(),
            title: title.trim().into(),
            main_file: "main.tex".into(),
            article_ids: vec![],
            created_at: time.clone(),
            updated_at: time,
        };
        let mut state = Stored {
            detail: ProjectDetail {
                project: project.clone(),
                assets: vec![],
                files: vec![],
                proposals: vec![],
                comments: vec![],
            },
            history: vec![],
            asset_data: Default::default(),
            asset_versions: Default::default(),
        };
        put(
            &mut state,
            "main.tex",
            &TEMPLATE.replace("Untitled manuscript", &escape_latex(title.trim())),
            0,
            actor,
            "Create project",
        )?;
        put(
            &mut state,
            "references.bib",
            "% References from the bibliography library\n",
            0,
            actor,
            "Create bibliography",
        )?;
        self.insert_project(&state).await?;
        Ok(project)
    }
    pub async fn detail(&self, id: &str) -> Result<ProjectDetail> {
        Ok(self.load(id).await?.1.detail)
    }
    pub async fn update_project(&self, id: &str, patch: ProjectPatch) -> Result<Project> {
        let _guard = self.lock.lock().await;
        let (v, mut state) = self.load(id).await?;
        if let Some(title) = patch.title {
            valid_title(&title)?;
            state.detail.project.title = title.trim().into();
        }
        if let Some(main) = patch.main_file {
            validate_path(&main)?;
            if !main.ends_with(".tex") || !state.detail.files.iter().any(|f| f.path == main) {
                return Err(Error::Invalid(
                    "main file must be an existing .tex file".into(),
                ));
            }
            state.detail.project.main_file = main;
        }
        if let Some(ids) = patch.article_ids {
            state.detail.project.article_ids = ids;
        }
        self.persist(v, &mut state).await?;
        Ok(state.detail.project)
    }
    pub async fn save_file(
        &self,
        id: &str,
        path: &str,
        content: &str,
        expected_revision: u64,
        actor: &str,
        message: &str,
    ) -> Result<File> {
        let _guard = self.lock.lock().await;
        let (v, mut state) = self.load(id).await?;
        let file = put(&mut state, path, content, expected_revision, actor, message)?;
        self.persist(v, &mut state).await?;
        Ok(file)
    }
    pub async fn delete_file(&self, id: &str, path: &str, expected_revision: u64) -> Result<()> {
        let _guard = self.lock.lock().await;
        let (v, mut state) = self.load(id).await?;
        if path == state.detail.project.main_file {
            return Err(Error::Invalid("cannot delete the main file".into()));
        }
        let index = state
            .detail
            .files
            .iter()
            .position(|f| f.path == path)
            .ok_or(Error::NotFound)?;
        let file = &state.detail.files[index];
        if file.revision != expected_revision {
            return Err(Error::Conflict);
        }
        state.history.push(Revision {
            id: uid(),
            path: path.into(),
            content: file.content.clone(),
            revision: file.revision + 1,
            actor: "human".into(),
            message: "Delete file".into(),
            created_at: now(),
            deleted: true,
        });
        state.detail.files.remove(index);
        self.persist(v, &mut state).await
    }
    pub async fn history(&self, id: &str, path: Option<&str>) -> Result<Vec<Revision>> {
        let mut history = self.load(id).await?.1.history;
        history.retain(|r| path.is_none_or(|p| p == r.path));
        history.reverse();
        Ok(history)
    }
    pub async fn restore(
        &self,
        id: &str,
        path: &str,
        revision: u64,
        expected_revision: u64,
    ) -> Result<File> {
        let _guard = self.lock.lock().await;
        let (v, mut state) = self.load(id).await?;
        let old = state
            .history
            .iter()
            .find(|r| r.path == path && r.revision == revision && !r.deleted)
            .ok_or(Error::NotFound)?
            .content
            .clone();
        let file = put(
            &mut state,
            path,
            &old,
            expected_revision,
            "human",
            "Restore revision",
        )?;
        self.persist(v, &mut state).await?;
        Ok(file)
    }
    pub async fn propose(
        &self,
        id: &str,
        path: &str,
        content: &str,
        base_revision: u64,
        summary: &str,
        author: &str,
    ) -> Result<Proposal> {
        validate_path(path)?;
        validate_content(content)?;
        let _guard = self.lock.lock().await;
        let (v, mut state) = self.load(id).await?;
        let file = state
            .detail
            .files
            .iter()
            .find(|f| f.path == path)
            .ok_or(Error::NotFound)?;
        if file.revision != base_revision {
            return Err(Error::Conflict);
        }
        let proposal = Proposal {
            id: uid(),
            path: path.into(),
            base_revision,
            original: file.content.clone(),
            proposed: content.into(),
            status: "pending".into(),
            author: author.into(),
            summary: summary.into(),
            created_at: now(),
        };
        state.detail.proposals.push(proposal.clone());
        self.persist(v, &mut state).await?;
        Ok(proposal)
    }
    pub async fn resolve_proposal(
        &self,
        id: &str,
        proposal_id: &str,
        accept: bool,
    ) -> Result<Proposal> {
        let _guard = self.lock.lock().await;
        let (v, mut state) = self.load(id).await?;
        let index = state
            .detail
            .proposals
            .iter()
            .position(|p| p.id == proposal_id)
            .ok_or(Error::NotFound)?;
        let proposal = state.detail.proposals[index].clone();
        if proposal.status != "pending" {
            return Err(Error::Conflict);
        }
        if accept {
            put(
                &mut state,
                &proposal.path,
                &proposal.proposed,
                proposal.base_revision,
                &proposal.author,
                &format!(
                    "Human accepted proposal {}: {}",
                    proposal.id, proposal.summary
                ),
            )?;
        }
        state.detail.proposals[index].status = if accept { "accepted" } else { "rejected" }.into();
        let result = state.detail.proposals[index].clone();
        self.persist(v, &mut state).await?;
        Ok(result)
    }
    pub async fn add_comment(
        &self,
        id: &str,
        path: &str,
        start_line: usize,
        end_line: usize,
        body: &str,
        author: &str,
    ) -> Result<Comment> {
        self.add_comment_at_revision(id, path, start_line, end_line, body, author, None)
            .await
    }
    pub async fn add_comment_at_revision(
        &self,
        id: &str,
        path: &str,
        start_line: usize,
        end_line: usize,
        body: &str,
        author: &str,
        expected_revision: Option<u64>,
    ) -> Result<Comment> {
        if body.trim().is_empty() || body.len() > 32_000 {
            return Err(Error::Invalid("comment must contain 1–32000 bytes".into()));
        }
        let _guard = self.lock.lock().await;
        let (v, mut state) = self.load(id).await?;
        let file = state
            .detail
            .files
            .iter()
            .find(|f| f.path == path)
            .ok_or(Error::NotFound)?;
        if expected_revision.is_some_and(|r| r != file.revision) {
            return Err(Error::Conflict);
        }
        if start_line == 0
            || end_line < start_line
            || end_line > file.content.lines().count().max(1)
        {
            return Err(Error::Invalid(
                "comment line range is outside the file".into(),
            ));
        }
        let comment = Comment {
            id: uid(),
            path: path.into(),
            start_line,
            end_line,
            body: body.into(),
            author: author.into(),
            resolved: false,
            created_at: now(),
            revision: file.revision,
        };
        state.detail.comments.push(comment.clone());
        self.persist(v, &mut state).await?;
        Ok(comment)
    }
    pub async fn resolve_comment(
        &self,
        id: &str,
        comment_id: &str,
        resolved: bool,
    ) -> Result<Comment> {
        let _guard = self.lock.lock().await;
        let (v, mut state) = self.load(id).await?;
        let comment = state
            .detail
            .comments
            .iter_mut()
            .find(|c| c.id == comment_id)
            .ok_or(Error::NotFound)?;
        comment.resolved = resolved;
        let result = comment.clone();
        self.persist(v, &mut state).await?;
        Ok(result)
    }
}
fn put(
    state: &mut Stored,
    path: &str,
    content: &str,
    expected: u64,
    actor: &str,
    message: &str,
) -> Result<File> {
    validate_path(path)?;
    validate_content(content)?;
    if state
        .detail
        .assets
        .iter()
        .any(|a| paths_overlap(&a.path, path))
    {
        return Err(Error::Invalid("source path conflicts with an asset".into()));
    }
    if state.detail.files.iter().any(|f| {
        f.path != path
            && (f.path.starts_with(&format!("{path}/"))
                || path.starts_with(&format!("{}/", f.path)))
    }) {
        return Err(Error::Invalid("file and directory paths overlap".into()));
    }
    let active_size = state
        .detail
        .files
        .iter()
        .filter(|f| f.path != path)
        .map(|f| f.content.len())
        .sum::<usize>()
        + state
            .detail
            .assets
            .iter()
            .map(|a| a.byte_size)
            .sum::<usize>()
        + content.len();
    if active_size > 64 * 1024 * 1024
        || (!state.detail.files.iter().any(|f| f.path == path)
            && state.detail.files.len() + state.detail.assets.len() >= 512)
    {
        return Err(Error::Invalid("project exceeds 64 MiB / 512 files".into()));
    }
    let current = state
        .detail
        .files
        .iter()
        .find(|f| f.path == path)
        .map(|f| f.revision)
        .unwrap_or(0);
    if current != expected {
        return Err(Error::Conflict);
    }
    let revision = state
        .history
        .iter()
        .filter(|r| r.path == path)
        .map(|r| r.revision)
        .max()
        .unwrap_or(0)
        .max(state.asset_versions.get(path).copied().unwrap_or(0))
        + 1;
    let file = File {
        path: path.into(),
        content: content.into(),
        revision,
        updated_at: now(),
    };
    state.history.push(Revision {
        id: uid(),
        path: path.into(),
        content: content.into(),
        revision,
        actor: actor.into(),
        message: message.into(),
        created_at: file.updated_at.clone(),
        deleted: false,
    });
    state.detail.files.retain(|f| f.path != path);
    state.detail.files.push(file.clone());
    Ok(file)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Build {
    #[serde(default)]
    pub main_file: String,
    pub id: String,
    pub status: String,
    pub log: String,
    pub diagnostics: Vec<serde_json::Value>,
    pub source_revisions: std::collections::BTreeMap<String, u64>,
    pub created_at: String,
}
impl ManuscriptStore {
    /// Save bibliography bindings and generated source as a single mutation.
    pub async fn sync_references(
        &self,
        id: &str,
        article_ids: Vec<String>,
        content: &str,
        expected_revision: u64,
    ) -> Result<File> {
        let _guard = self.lock.lock().await;
        let (v, mut state) = self.load(id).await?;
        let file = put(
            &mut state,
            "references.bib",
            content,
            expected_revision,
            "human",
            "Synchronize library references",
        )?;
        state.detail.project.article_ids = article_ids;
        self.persist(v, &mut state).await?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn stale_agent_proposal_cannot_overwrite_human_edit() {
        let store = ManuscriptStore::open_in_memory().await.unwrap();
        let p = store.create_project("Paper").await.unwrap();
        let proposal = store
            .propose(&p.id, "main.tex", "Agent draft", 1, "Rewrite", "agent:test")
            .await
            .unwrap();
        store
            .save_file(&p.id, "main.tex", "Human draft", 1, "human", "Edit")
            .await
            .unwrap();
        assert!(matches!(
            store.resolve_proposal(&p.id, &proposal.id, true).await,
            Err(Error::Conflict)
        ));
        let detail = store.detail(&p.id).await.unwrap();
        assert_eq!(
            detail
                .files
                .iter()
                .find(|f| f.path == "main.tex")
                .unwrap()
                .content,
            "Human draft"
        );
        assert_eq!(detail.proposals[0].status, "pending");
    }
    #[tokio::test]
    async fn accept_restore_and_recreate_preserve_monotonic_history() {
        let store = ManuscriptStore::open_in_memory().await.unwrap();
        let p = store.create_project("Paper").await.unwrap();
        let proposal = store
            .propose(&p.id, "main.tex", "New source", 1, "Rewrite", "agent:test")
            .await
            .unwrap();
        store
            .resolve_proposal(&p.id, &proposal.id, true)
            .await
            .unwrap();
        let restored = store.restore(&p.id, "main.tex", 1, 2).await.unwrap();
        assert_eq!(restored.revision, 3);
        assert_eq!(
            restored.content,
            TEMPLATE.replace("Untitled manuscript", "Paper")
        );
        store
            .save_file(&p.id, "part.tex", "first", 0, "human", "add")
            .await
            .unwrap();
        store.delete_file(&p.id, "part.tex", 1).await.unwrap();
        let recreated = store
            .save_file(&p.id, "part.tex", "second", 0, "human", "recreate")
            .await
            .unwrap();
        assert_eq!(recreated.revision, 3);
        assert!(matches!(
            store
                .save_file(&p.id, "part.tex", "stale", 1, "human", "edit")
                .await,
            Err(Error::Conflict)
        ));
        let history = store.history(&p.id, Some("part.tex")).await.unwrap();
        assert_eq!(history.len(), 3);
        assert!(history[1].deleted);
    }
    #[tokio::test]
    async fn persistent_projects_and_pdf_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects");
        let path = path.to_str().unwrap();
        let id;
        {
            let store = ManuscriptStore::open(path).await.unwrap();
            let p = store.create_project("Persistent").await.unwrap();
            id = p.id;
            let b = Build {
                main_file: "main.tex".into(),
                id: "b1".into(),
                status: "succeeded".into(),
                log: String::new(),
                diagnostics: vec![],
                source_revisions: Default::default(),
                created_at: now(),
            };
            store
                .save_build(&id, &b, Some(b"%PDF".to_vec()))
                .await
                .unwrap();
        }
        let store = ManuscriptStore::open(path).await.unwrap();
        assert_eq!(store.detail(&id).await.unwrap().project.title, "Persistent");
        assert_eq!(store.build_pdf(&id, "b1").await.unwrap(), b"%PDF");
        assert!(std::path::Path::new(path).is_dir());
        assert!(
            std::path::Path::new(path)
                .join("manuscripts")
                .join(&id)
                .join("project.json")
                .is_file()
        );
        assert!(!std::path::Path::new(&format!("{path}.vfs")).exists());
    }
    #[tokio::test]
    async fn binary_assets_roundtrip_and_cas_are_in_compile_snapshot() {
        let store = ManuscriptStore::open_in_memory().await.unwrap();
        let p = store.create_project("Figures").await.unwrap();
        let bytes = vec![137, 80, 78, 71, 0, 255];
        let a = store
            .save_asset(&p.id, "figures/chart.png", bytes.clone(), 0)
            .await
            .unwrap();
        assert_eq!(a.revision, 1);
        assert_eq!(store.asset_bytes(&p.id, &a.path).await.unwrap(), bytes);
        assert!(matches!(
            store.save_asset(&p.id, &a.path, vec![1], 0).await,
            Err(Error::Conflict)
        ));
        assert!(
            store
                .save_file(&p.id, &a.path, "overwrite", 0, "human", "edit")
                .await
                .is_err()
        );
        assert!(
            store
                .save_asset(&p.id, "main.tex", vec![1], 0)
                .await
                .is_err()
        );
        let snapshot = store.compile_snapshot(&p.id).await.unwrap();
        assert_eq!(snapshot.files[&a.path], bytes);
        assert_eq!(snapshot.source_revisions[&a.path], 1);
        store.delete_asset(&p.id, &a.path, 1).await.unwrap();
        assert!(store.asset_bytes(&p.id, &a.path).await.is_err());
        assert_eq!(
            store
                .save_asset(&p.id, &a.path, vec![1], 0)
                .await
                .unwrap()
                .revision,
            2
        );
    }
    #[tokio::test]
    async fn project_title_is_escaped_in_initial_latex() {
        let store = ManuscriptStore::open_in_memory().await.unwrap();
        let p = store
            .create_project(r"A&B_50% \ {test} ~ ^ $ #")
            .await
            .unwrap();
        let source = store
            .detail(&p.id)
            .await
            .unwrap()
            .files
            .into_iter()
            .find(|f| f.path == "main.tex")
            .unwrap()
            .content;
        assert!(source.contains(r"\title{A\&B\_50\% \textbackslash{} \{test\} \textasciitilde{} \textasciicircum{} \$ \#}"));
    }
    #[test]
    fn rejects_unsafe_project_paths() {
        for path in [
            "../secret",
            "/tmp/main.tex",
            "a/../b",
            "a\\b",
            "C:main.tex",
            "a//b",
            "./x",
            "a\0b",
        ] {
            assert!(validate_path(path).is_err(), "{path:?}");
        }
        assert!(validate_path("sections/introduction.tex").is_ok());
    }
    #[tokio::test]
    async fn concurrent_saves_have_one_winner() {
        let store = ManuscriptStore::open_in_memory().await.unwrap();
        let p = store.create_project("Paper").await.unwrap();
        let (a, b) = tokio::join!(
            store.save_file(&p.id, "main.tex", "A", 1, "human", "edit"),
            store.save_file(&p.id, "main.tex", "B", 1, "human", "edit")
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    }
}

fn paths_overlap(a: &str, b: &str) -> bool {
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}
pub struct CompileSnapshot {
    pub main_file: String,
    pub files: std::collections::BTreeMap<String, Vec<u8>>,
    pub source_revisions: std::collections::BTreeMap<String, u64>,
}
impl ManuscriptStore {
    pub async fn save_asset(
        &self,
        id: &str,
        path: &str,
        data: Vec<u8>,
        expected_revision: u64,
    ) -> Result<Asset> {
        validate_path(path)?;
        if data.is_empty() || data.len() > 16 * 1024 * 1024 {
            return Err(Error::Invalid("asset must contain 1 byte to 16 MiB".into()));
        }
        let _guard = self.lock.lock().await;
        let (v, mut state) = self.load(id).await?;
        if state
            .detail
            .files
            .iter()
            .any(|f| paths_overlap(&f.path, path))
            || state
                .detail
                .assets
                .iter()
                .any(|a| a.path != path && paths_overlap(&a.path, path))
        {
            return Err(Error::Invalid(
                "asset path conflicts with an existing project path".into(),
            ));
        }
        let current = state
            .detail
            .assets
            .iter()
            .find(|a| a.path == path)
            .map(|a| a.revision)
            .unwrap_or(0);
        if current != expected_revision {
            return Err(Error::Conflict);
        }
        let total = state
            .detail
            .assets
            .iter()
            .filter(|a| a.path != path)
            .map(|a| a.byte_size)
            .sum::<usize>()
            + data.len()
            + state
                .detail
                .files
                .iter()
                .map(|f| f.content.len())
                .sum::<usize>();
        if total > 64 * 1024 * 1024
            || (current == 0
                && (state.detail.assets.len() >= 128
                    || state.detail.assets.len() + state.detail.files.len() >= 512))
        {
            return Err(Error::Invalid(
                "project asset limit is 64 MiB / 128 files".into(),
            ));
        }
        let revision = state.asset_versions.get(path).copied().unwrap_or(0).max(
            state
                .history
                .iter()
                .filter(|r| r.path == path)
                .map(|r| r.revision)
                .max()
                .unwrap_or(0),
        ) + 1;
        let asset = Asset {
            path: path.into(),
            revision,
            byte_size: data.len(),
            updated_at: now(),
        };
        // Blobs are immutable. Publishing a new reference is the same atomic
        // project CAS as source edits; compile snapshots can safely retain old IDs.
        let blob_id = uid();
        self.write_blob(&blob_id, data).await?;
        state.asset_data.insert(path.into(), blob_id);
        state.asset_versions.insert(path.into(), revision);
        state.detail.assets.retain(|a| a.path != path);
        state.detail.assets.push(asset.clone());
        self.persist(v, &mut state).await?;
        Ok(asset)
    }
    pub async fn delete_asset(&self, id: &str, path: &str, expected_revision: u64) -> Result<()> {
        let _guard = self.lock.lock().await;
        let (v, mut state) = self.load(id).await?;
        let asset = state
            .detail
            .assets
            .iter()
            .find(|a| a.path == path)
            .ok_or(Error::NotFound)?;
        if asset.revision != expected_revision {
            return Err(Error::Conflict);
        }
        state.detail.assets.retain(|a| a.path != path);
        state.asset_data.remove(path);
        self.persist(v, &mut state).await
    }
    pub async fn asset_bytes(&self, id: &str, path: &str) -> Result<Vec<u8>> {
        let state = self.load(id).await?.1;
        let data = state.asset_data.get(path).ok_or(Error::NotFound)?;
        self.read_blob(data).await
    }
    pub async fn compile_snapshot(&self, id: &str) -> Result<CompileSnapshot> {
        let state = self.load(id).await?.1;
        let mut files: std::collections::BTreeMap<_, _> = state
            .detail
            .files
            .iter()
            .map(|f| (f.path.clone(), f.content.as_bytes().to_vec()))
            .collect();
        let mut source_revisions: std::collections::BTreeMap<_, _> = state
            .detail
            .files
            .iter()
            .map(|f| (f.path.clone(), f.revision))
            .collect();
        for asset in state.detail.assets {
            let data = state.asset_data.get(&asset.path).ok_or(Error::NotFound)?;
            files.insert(asset.path.clone(), self.read_blob(data).await?);
            source_revisions.insert(asset.path, asset.revision);
        }
        Ok(CompileSnapshot {
            main_file: state.detail.project.main_file,
            files,
            source_revisions,
        })
    }
}

fn escape_latex(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\textbackslash{}"),
            '{' | '}' | '$' | '&' | '#' | '_' | '%' => {
                out.push('\\');
                out.push(ch);
            }
            '^' => out.push_str("\\textasciicircum{}"),
            '~' => out.push_str("\\textasciitilde{}"),
            _ => out.push(ch),
        }
    }
    out
}
