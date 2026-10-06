//! An OpenDAL access layer that enforces VFS permissions.
//!
//! `OpendalFileStorage` sometimes must return an `Operator` for compatibility
//! with existing nodes. This layer keeps such operators inside the VFS
//! permission boundary: every path presented to the backend is authorized
//! again, so callers cannot use a different backend key to bypass VFS policy.

use std::sync::Arc;

use opendal::raw::{
    Access, Accessor, OpCopy, OpCreateDir, OpDelete, OpList, OpPresign, OpRead, OpRename, OpStat,
    OpWrite, PresignOperation, RpCreateDir, RpDelete, RpList, RpRead, RpWrite, oio,
};
use opendal::{EntryMode, Error, ErrorKind, Operator, Result};

use crate::permission::{MountPermissions, VfsAccess, VfsPrincipal, normalize_policy_path};

#[derive(Clone, Debug)]
pub(crate) struct AuthorizedOperatorConfig {
    pub principal: VfsPrincipal,
    pub policies: Vec<AuthorizedMountPolicy>,
}

#[derive(Clone, Debug)]
pub(crate) struct AuthorizedMountPolicy {
    pub virtual_prefix: String,
    pub source_prefix: String,
    pub permissions: MountPermissions,
    pub read_only: bool,
}

impl AuthorizedOperatorConfig {
    pub fn default_backend(principal: VfsPrincipal) -> Self {
        Self {
            principal,
            policies: vec![AuthorizedMountPolicy {
                virtual_prefix: "/".into(),
                source_prefix: String::new(),
                permissions: MountPermissions::default(),
                read_only: false,
            }],
        }
    }

    fn authorize(&self, path: &str, access: VfsAccess, is_directory: bool) -> Result<()> {
        let key = normalize_policy_path(path);
        let mut selected: Option<(&AuthorizedMountPolicy, String)> = None;
        for policy in &self.policies {
            let source = normalize_policy_path(&policy.source_prefix);
            let relative = if source.is_empty() {
                key.clone()
            } else if key == source {
                String::new()
            } else {
                match key.strip_prefix(&format!("{source}/")) {
                    Some(relative) => relative.to_string(),
                    None => continue,
                }
            };
            let replace = match &selected {
                None => true,
                Some((current, _)) => {
                    policy.source_prefix.len() > current.source_prefix.len()
                        || (policy.source_prefix.len() == current.source_prefix.len()
                            && policy.virtual_prefix.len() > current.virtual_prefix.len())
                }
            };
            if replace {
                selected = Some((policy, relative));
            }
        }

        let Some((policy, relative)) = selected else {
            return Err(permission_denied(path, access));
        };
        if policy.read_only && access == VfsAccess::Write {
            return Err(permission_denied(path, access));
        }

        let parts: Vec<&str> = relative
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        for index in 0..parts.len().saturating_sub(1) {
            let prefix = parts[..=index].join("/");
            policy
                .permissions
                .authorize(&self.principal, &prefix, true, VfsAccess::Execute)
                .map_err(|_| permission_denied(path, access))?;
        }
        policy
            .permissions
            .authorize(&self.principal, &relative, is_directory, access)
            .map_err(|_| permission_denied(path, access))
    }
}

fn permission_denied(path: &str, access: VfsAccess) -> Error {
    Error::new(
        ErrorKind::PermissionDenied,
        format!("VFS {access} denied for '{path}'"),
    )
}

#[derive(Debug)]
struct AuthorizedAccess {
    inner: Accessor,
    config: AuthorizedOperatorConfig,
}

impl Access for AuthorizedAccess {
    type Reader = oio::Reader;
    type Writer = oio::Writer;
    type Lister = oio::Lister;
    type Deleter = oio::Deleter;
    type Copier = oio::Copier;

    fn info(&self) -> Arc<opendal::raw::AccessorInfo> {
        self.inner.info()
    }

    async fn create_dir(&self, path: &str, args: OpCreateDir) -> Result<RpCreateDir> {
        self.config.authorize(path, VfsAccess::Write, true)?;
        self.inner.create_dir(path, args).await
    }

    async fn stat(&self, path: &str, args: OpStat) -> Result<opendal::raw::RpStat> {
        self.config.authorize(
            path,
            VfsAccess::Read,
            path.is_empty() || path.ends_with('/'),
        )?;
        self.inner.stat(path, args).await
    }

    async fn read(&self, path: &str, args: OpRead) -> Result<(RpRead, Self::Reader)> {
        self.config.authorize(path, VfsAccess::Read, false)?;
        self.inner.read(path, args).await
    }

    async fn write(&self, path: &str, args: OpWrite) -> Result<(RpWrite, Self::Writer)> {
        self.config.authorize(path, VfsAccess::Write, false)?;
        self.inner.write(path, args).await
    }

    async fn delete(&self) -> Result<(RpDelete, Self::Deleter)> {
        let (reply, deleter) = self.inner.delete().await?;
        Ok((
            reply,
            Box::new(AuthorizedDeleter {
                inner: deleter,
                config: self.config.clone(),
            }),
        ))
    }

    async fn list(&self, path: &str, args: OpList) -> Result<(RpList, Self::Lister)> {
        self.config.authorize(path, VfsAccess::Read, true)?;
        let (reply, lister) = self.inner.list(path, args).await?;
        Ok((
            reply,
            Box::new(AuthorizedLister {
                inner: lister,
                config: self.config.clone(),
            }),
        ))
    }

    async fn copy(
        &self,
        from: &str,
        to: &str,
        args: OpCopy,
        opts: opendal::raw::OpCopier,
    ) -> Result<(opendal::raw::RpCopy, Self::Copier)> {
        self.config.authorize(from, VfsAccess::Read, false)?;
        self.config.authorize(to, VfsAccess::Write, false)?;
        self.inner.copy(from, to, args, opts).await
    }

    async fn rename(&self, from: &str, to: &str, args: OpRename) -> Result<opendal::raw::RpRename> {
        self.config.authorize(from, VfsAccess::Write, false)?;
        self.config.authorize(to, VfsAccess::Write, false)?;
        self.inner.rename(from, to, args).await
    }

    async fn presign(&self, path: &str, args: OpPresign) -> Result<opendal::raw::RpPresign> {
        let access = match args.operation() {
            PresignOperation::Write(_) | PresignOperation::Delete(_) => VfsAccess::Write,
            PresignOperation::Stat(_) | PresignOperation::Read(_) => VfsAccess::Read,
            _ => VfsAccess::Read,
        };
        self.config.authorize(path, access, false)?;
        self.inner.presign(path, args).await
    }
}

struct AuthorizedDeleter {
    inner: oio::Deleter,
    config: AuthorizedOperatorConfig,
}

impl oio::Delete for AuthorizedDeleter {
    async fn delete(&mut self, path: &str, args: OpDelete) -> Result<()> {
        self.config.authorize(path, VfsAccess::Write, false)?;
        self.inner.delete(path, args).await
    }

    async fn close(&mut self) -> Result<()> {
        self.inner.close().await
    }
}

struct AuthorizedLister {
    inner: oio::Lister,
    config: AuthorizedOperatorConfig,
}

impl oio::List for AuthorizedLister {
    async fn next(&mut self) -> Result<Option<opendal::raw::oio::Entry>> {
        loop {
            let Some(entry) = self.inner.next().await? else {
                return Ok(None);
            };
            let is_directory = entry.mode() == EntryMode::DIR;
            if self
                .config
                .authorize(entry.path(), VfsAccess::Read, is_directory)
                .is_ok()
            {
                return Ok(Some(entry));
            }
        }
    }
}

pub(crate) fn authorize_operator(operator: Operator, config: AuthorizedOperatorConfig) -> Operator {
    Operator::from_inner(Arc::new(AuthorizedAccess {
        inner: operator.into_inner(),
        config,
    }))
}
