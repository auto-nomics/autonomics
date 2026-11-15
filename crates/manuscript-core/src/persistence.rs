//! Protocol v1: immutable snapshots plus an atomically replaced project.json head.
use super::*;

#[derive(Serialize, Deserialize)]
struct Head {
    protocol: u32,
    generation: i64,
    snapshot: String,
}
impl ManuscriptStore {
    /// Open a standalone local VFS rooted at the supplied directory.
    pub async fn open(path: impl AsRef<str>) -> Result<Self> {
        Self::from_storage(
            Arc::new(OpendalFileStorage::new(path.as_ref())),
            "/manuscripts",
        )
    }
    pub async fn open_in_memory() -> Result<Self> {
        Self::from_storage(Arc::new(OpendalFileStorage::new_temp()), "/manuscripts")
    }
    pub fn from_storage(storage: Arc<OpendalFileStorage>, root: &str) -> Result<Self> {
        validate_path(root.trim_matches('/'))?;
        Ok(Self {
            storage,
            root: root.trim_end_matches('/').into(),
            lock: Default::default(),
        })
    }
    fn project_dir(&self, id: &str) -> Result<String> {
        validate_path(id)?;
        if id.contains('/') {
            return Err(Error::Invalid(
                "project ID must be one path component".into(),
            ));
        }
        Ok(format!("{}/{}", self.root, id))
    }
    async fn read(&self, path: &str) -> Result<Vec<u8>> {
        match self.storage.read_bytes(path).await {
            Ok(bytes) => Ok(bytes),
            Err(error) if error.kind() == opendal::ErrorKind::NotFound => Err(Error::NotFound),
            Err(error) => Err(error.into()),
        }
    }
    async fn write(&self, path: &str, bytes: Vec<u8>) -> Result<()> {
        self.storage.write_bytes(path, bytes).await?;
        Ok(())
    }
    async fn head(&self, id: &str) -> Result<(Vec<u8>, Head)> {
        let bytes = self
            .read(&format!("{}/project.json", self.project_dir(id)?))
            .await?;
        let head: Head = serde_json::from_slice(&bytes)?;
        if head.protocol != 1 || head.snapshot.contains('/') {
            return Err(Error::Invalid("unsupported manuscript protocol".into()));
        }
        validate_path(&head.snapshot)?;
        Ok((bytes, head))
    }
    pub(super) async fn load(&self, id: &str) -> Result<(i64, Stored)> {
        let (_, head) = self.head(id).await?;
        let dir = format!("{}/snapshots/{}", self.project_dir(id)?, head.snapshot);
        let mut state: Stored =
            serde_json::from_slice(&self.read(&format!("{dir}/metadata.json")).await?)?;
        if state.detail.project.id != id {
            return Err(Error::Invalid("project ID mismatch".into()));
        }
        state.history = serde_json::from_slice(&self.read(&format!("{dir}/history.json")).await?)?;
        for file in &mut state.detail.files {
            validate_path(&file.path)?;
            file.content =
                String::from_utf8(self.read(&format!("{dir}/files/{}", file.path)).await?)
                    .map_err(|_| Error::Invalid("source is not UTF-8".into()))?;
        }
        Ok((head.generation, state))
    }
    async fn commit(
        &self,
        expected: Option<Vec<u8>>,
        generation: i64,
        state: &Stored,
    ) -> Result<()> {
        let project = self.project_dir(&state.detail.project.id)?;
        let snapshot = uid();
        let dir = format!("{project}/snapshots/{snapshot}");
        let mut metadata = state.clone();
        for file in &mut metadata.detail.files {
            validate_path(&file.path)?;
            self.write(
                &format!("{dir}/files/{}", file.path),
                file.content.as_bytes().to_vec(),
            )
            .await?;
            file.content.clear();
        }
        // Each published files/ directory is a complete, compilable source tree.
        // Immutable asset objects also retain old revisions during snapshot publication.
        for asset in &state.detail.assets {
            validate_path(&asset.path)?;
            let blob = state.asset_data.get(&asset.path).ok_or(Error::NotFound)?;
            self.write(
                &format!("{dir}/files/{}", asset.path),
                self.read_blob(blob).await?,
            )
            .await?;
        }
        metadata.history.clear();
        self.write(
            &format!("{dir}/history.json"),
            serde_json::to_vec(&state.history)?,
        )
        .await?;
        self.write(
            &format!("{dir}/metadata.json"),
            serde_json::to_vec_pretty(&metadata)?,
        )
        .await?;
        let head = Head {
            protocol: 1,
            generation,
            snapshot,
        };
        if !self
            .storage
            .compare_exchange_bytes(
                &format!("{project}/project.json"),
                expected.as_deref(),
                serde_json::to_vec_pretty(&head)?,
            )
            .await?
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub(super) async fn insert_project(&self, state: &Stored) -> Result<()> {
        self.commit(None, 1, state).await
    }
    pub(super) async fn persist(&self, generation: i64, state: &mut Stored) -> Result<()> {
        let (bytes, head) = self.head(&state.detail.project.id).await?;
        if head.generation != generation {
            return Err(Error::Conflict);
        }
        state.detail.project.updated_at = now();
        self.commit(Some(bytes), generation + 1, state).await
    }
    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        let dir = format!("{}/", self.root);
        self.storage.check_listable(&dir)?;
        let entries = match self
            .storage
            .resolve(&dir)
            .list(&format!(
                "{}/",
                self.storage.resolve_path(&dir).trim_end_matches('/')
            ))
            .await
        {
            Ok(entries) => entries,
            Err(error) if error.kind() == opendal::ErrorKind::NotFound => return Ok(vec![]),
            Err(error) => return Err(error.into()),
        };
        let mut projects = vec![];
        for entry in entries {
            if !entry.metadata().is_dir() {
                continue;
            }
            let id = entry
                .path()
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("");
            match self.head(id).await {
                Ok(_) => projects.push(self.load(id).await?.1.detail.project),
                Err(Error::NotFound) => (), // An uncommitted staging directory.
                Err(error) => return Err(error),
            }
        }
        projects.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(projects)
    }
    pub(super) async fn write_blob(&self, id: &str, bytes: Vec<u8>) -> Result<()> {
        validate_path(id)?;
        if id.contains('/') {
            return Err(Error::Invalid("invalid asset ID".into()));
        }
        self.write(&format!("{}/assets/{id}", self.root), bytes)
            .await
    }
    pub(super) async fn read_blob(&self, id: &str) -> Result<Vec<u8>> {
        validate_path(id)?;
        if id.contains('/') {
            return Err(Error::Invalid("invalid asset ID".into()));
        }
        self.read(&format!("{}/assets/{id}", self.root)).await
    }
    pub async fn save_build(
        &self,
        project_id: &str,
        build: &Build,
        pdf: Option<Vec<u8>>,
    ) -> Result<()> {
        self.head(project_id).await?;
        validate_path(&build.id)?;
        if build.id.contains('/') {
            return Err(Error::Invalid("invalid build ID".into()));
        }
        let project = self.project_dir(project_id)?;
        let dir = format!("{project}/builds/{}", build.id);
        if let Some(pdf) = pdf {
            self.write(&format!("{dir}/output.pdf"), pdf).await?;
        }
        self.write(&format!("{dir}/compile.log"), build.log.as_bytes().to_vec())
            .await?;
        self.write(
            &format!("{dir}/build.json"),
            serde_json::to_vec_pretty(build)?,
        )
        .await?;
        self.write(
            &format!("{project}/latest-build.json"),
            serde_json::to_vec(&build.id)?,
        )
        .await
    }
    pub async fn latest_build(&self, project_id: &str) -> Result<Build> {
        self.head(project_id).await?;
        let project = self.project_dir(project_id)?;
        let id: String =
            serde_json::from_slice(&self.read(&format!("{project}/latest-build.json")).await?)?;
        validate_path(&id)?;
        if id.contains('/') {
            return Err(Error::Invalid("invalid build ID".into()));
        }
        Ok(serde_json::from_slice(
            &self
                .read(&format!("{project}/builds/{id}/build.json"))
                .await?,
        )?)
    }
    pub async fn build_pdf(&self, project_id: &str, build_id: &str) -> Result<Vec<u8>> {
        self.head(project_id).await?;
        validate_path(build_id)?;
        if build_id.contains('/') {
            return Err(Error::Invalid("invalid build ID".into()));
        }
        self.read(&format!(
            "{}/builds/{build_id}/output.pdf",
            self.project_dir(project_id)?
        ))
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_vfs_stores_publish_only_one_snapshot_for_observed_generation() {
        let storage = Arc::new(OpendalFileStorage::new_temp());
        let a = ManuscriptStore::from_storage(storage.clone(), "/papers").unwrap();
        let b = ManuscriptStore::from_storage(storage.clone(), "/papers").unwrap();
        let project = a.create_project("VFS protocol").await.unwrap();
        let (version, mut left) = a.load(&project.id).await.unwrap();
        let (_, mut right) = b.load(&project.id).await.unwrap();
        put(&mut left, "main.tex", "left", 1, "human", "edit").unwrap();
        put(&mut right, "main.tex", "right", 1, "human", "edit").unwrap();
        let (x, y) = tokio::join!(
            a.persist(version, &mut left),
            b.persist(version, &mut right)
        );
        assert_eq!(usize::from(x.is_ok()) + usize::from(y.is_ok()), 1);
        assert!(matches!(x.err().or(y.err()), Some(Error::Conflict)));
        let detail = a.detail(&project.id).await.unwrap();
        assert_eq!(
            detail
                .files
                .iter()
                .find(|f| f.path == "main.tex")
                .unwrap()
                .revision,
            2
        );
        assert_eq!(b.list_projects().await.unwrap().len(), 1);
        let (_, head) = a.head(&project.id).await.unwrap();
        let path = format!(
            "/papers/{}/snapshots/{}/files/main.tex",
            project.id, head.snapshot
        );
        assert!(!storage.read_bytes(&path).await.unwrap().is_empty());
        assert!(matches!(
            a.detail("../escape").await,
            Err(Error::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn ignores_unpublished_snapshots_but_reports_missing_committed_sources() {
        let storage = Arc::new(OpendalFileStorage::new_temp());
        let store = ManuscriptStore::from_storage(storage.clone(), "/papers").unwrap();
        storage
            .write_bytes(
                "/papers/interrupted/snapshots/staging/files/main.tex",
                b"partial".to_vec(),
            )
            .await
            .unwrap();
        assert!(store.list_projects().await.unwrap().is_empty());
        let project = store.create_project("Committed").await.unwrap();
        let (_, head) = store.head(&project.id).await.unwrap();
        storage
            .delete_object(&format!(
                "/papers/{}/snapshots/{}/files/main.tex",
                project.id, head.snapshot
            ))
            .await
            .unwrap();
        assert!(matches!(
            store.compile_snapshot(&project.id).await,
            Err(Error::NotFound)
        ));
        assert!(matches!(store.list_projects().await, Err(Error::NotFound)));
    }
}
