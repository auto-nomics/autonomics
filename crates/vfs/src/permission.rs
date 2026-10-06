//! Unix-style identities, modes, and mount policies for the VFS.
//!
//! Permissions are evaluated by the VFS layer before a virtual path is
//! dispatched to a backend operator. Mount-level permissions provide the
//! first stable boundary; per-inode metadata can build on these types later.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub const VFS_ROOT_UID: u32 = 0;
pub const VFS_ROOT_GID: u32 = 0;
pub const VFS_PLUGIN_DEVELOPER_GID: u32 = 100;

/// The process identity attached to a VFS view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VfsPrincipal {
    pub uid: u32,
    pub gid: u32,
    pub groups: BTreeSet<u32>,
    pub superuser: bool,
}

impl VfsPrincipal {
    pub fn root() -> Self {
        Self {
            uid: VFS_ROOT_UID,
            gid: VFS_ROOT_GID,
            groups: BTreeSet::new(),
            superuser: true,
        }
    }

    pub fn new(uid: u32, gid: u32) -> Self {
        Self::with_groups(uid, gid, [])
    }

    pub fn with_groups(uid: u32, gid: u32, groups: impl IntoIterator<Item = u32>) -> Self {
        let mut group_set: BTreeSet<_> = groups.into_iter().collect();
        group_set.insert(gid);
        Self {
            uid,
            gid,
            groups: group_set,
            superuser: false,
        }
    }

    pub fn plugin_developer(uid: u32) -> Self {
        Self::with_groups(uid, VFS_PLUGIN_DEVELOPER_GID, [])
    }

    fn in_group(&self, gid: u32) -> bool {
        self.gid == gid || self.groups.contains(&gid)
    }
}

impl Default for VfsPrincipal {
    fn default() -> Self {
        Self::root()
    }
}

impl fmt::Display for VfsPrincipal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "uid={},gid={}", self.uid, self.gid)?;
        if self.superuser {
            f.write_str(",superuser")?;
        }
        Ok(())
    }
}

/// A nine-bit Unix permission mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VfsMode(u16);

impl VfsMode {
    pub const fn from_bits(bits: u16) -> Self {
        Self(bits & 0o777)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub fn octal(self) -> String {
        format!("{:04o}", self.0)
    }

    fn class_bits(&self, owner: bool, group: bool) -> u16 {
        if owner {
            (self.0 >> 6) & 0o7
        } else if group {
            (self.0 >> 3) & 0o7
        } else {
            self.0 & 0o7
        }
    }

    pub fn allows(
        self,
        principal: &VfsPrincipal,
        ownership: VfsOwnership,
        access: VfsAccess,
    ) -> bool {
        if principal.superuser {
            return true;
        }
        let bits = self.class_bits(
            principal.uid == ownership.uid,
            principal.in_group(ownership.gid),
        );
        let bit = match access {
            VfsAccess::Read => 0o4,
            VfsAccess::Write => 0o2,
            VfsAccess::Execute => 0o1,
        };
        bits & bit == bit
    }
}

impl FromStr for VfsMode {
    type Err = VfsPermissionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim();
        if !trimmed.starts_with("0") && trimmed.len() != 3 && trimmed.len() != 4 {
            return Err(VfsPermissionError::InvalidMode(value.to_string()));
        }
        let digits = trimmed.strip_prefix("0").unwrap_or(trimmed);
        if digits.len() != 3 || !digits.bytes().all(|b| b.is_ascii_digit() && b <= b'7') {
            return Err(VfsPermissionError::InvalidMode(value.to_string()));
        }
        u16::from_str_radix(digits, 8)
            .map(Self::from_bits)
            .map_err(|_| VfsPermissionError::InvalidMode(value.to_string()))
    }
}

impl TryFrom<&str> for VfsMode {
    type Error = VfsPermissionError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl TryFrom<String> for VfsMode {
    type Error = VfsPermissionError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.as_str().try_into()
    }
}

impl From<VfsMode> for String {
    fn from(value: VfsMode) -> Self {
        value.octal()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VfsAccess {
    Read,
    Write,
    Execute,
}

impl fmt::Display for VfsAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Execute => "execute",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VfsOwnership {
    pub uid: u32,
    pub gid: u32,
}

impl VfsOwnership {
    pub const fn root() -> Self {
        Self {
            uid: VFS_ROOT_UID,
            gid: VFS_ROOT_GID,
        }
    }
}

impl Default for VfsOwnership {
    fn default() -> Self {
        Self::root()
    }
}

/// An explicit deny policy attached to a mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum VfsPathRule {
    Deny {
        /// Glob relative to the mount's virtual path.
        path: String,
        /// Access classes denied by this rule. Defaults to read, write, and execute.
        #[serde(default = "deny_all_access")]
        access: Vec<VfsAccess>,
    },
}

fn deny_all_access() -> Vec<VfsAccess> {
    vec![VfsAccess::Read, VfsAccess::Write, VfsAccess::Execute]
}

/// Mount-level Unix defaults and path policies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MountPermissions {
    pub owner: VfsOwnership,
    /// Mode used when no per-inode metadata exists.
    pub mode: VfsMode,
    pub file_mode: VfsMode,
    pub directory_mode: VfsMode,
    pub rules: Vec<VfsPathRule>,
}

impl MountPermissions {
    pub const fn unix(
        owner: VfsOwnership,
        mode: VfsMode,
        file_mode: VfsMode,
        directory_mode: VfsMode,
    ) -> Self {
        Self {
            owner,
            mode,
            file_mode,
            directory_mode,
            rules: Vec::new(),
        }
    }

    pub fn with_rules(mut self, rules: Vec<VfsPathRule>) -> Self {
        self.rules = rules;
        self
    }

    pub fn mode_for(&self, is_directory: bool) -> VfsMode {
        if is_directory {
            self.directory_mode
        } else {
            self.file_mode
        }
    }

    /// Evaluate a path relative to the mount point.
    pub fn authorize(
        &self,
        principal: &VfsPrincipal,
        relative_path: &str,
        is_directory: bool,
        access: VfsAccess,
    ) -> Result<(), VfsPermissionError> {
        let normalized = normalize_policy_path(relative_path);
        for rule in &self.rules {
            let VfsPathRule::Deny {
                path,
                access: denied_access,
            } = rule;
            if denied_access.contains(&access)
                && glob::Pattern::new(path)
                    .map(|pattern| pattern.matches(&normalized))
                    .unwrap_or(false)
            {
                return Err(VfsPermissionError::Denied {
                    principal: principal.clone(),
                    path: normalized,
                    access,
                });
            }
        }
        self.mode_for(is_directory)
            .allows(principal, self.owner, access)
            .then_some(())
            .ok_or(VfsPermissionError::Denied {
                principal: principal.clone(),
                path: normalized,
                access,
            })
    }
}

impl Default for MountPermissions {
    fn default() -> Self {
        Self::unix(
            VfsOwnership::root(),
            VfsMode::from_bits(0o777),
            VfsMode::from_bits(0o666),
            VfsMode::from_bits(0o777),
        )
    }
}

pub(crate) fn normalize_policy_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VfsPermissionError {
    #[error("invalid Unix mode '{0}': expected three octal digits such as 755")]
    InvalidMode(String),
    #[error("{access} denied for {principal} on '{path}'")]
    Denied {
        principal: VfsPrincipal,
        path: String,
        access: VfsAccess,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_uses_unix_class_precedence() {
        let mode = VfsMode::from_bits(0o640);
        let owner = VfsOwnership {
            uid: 1000,
            gid: 100,
        };
        let owner_principal = VfsPrincipal::new(1000, 999);
        assert!(mode.allows(&owner_principal, owner, VfsAccess::Read));
        assert!(mode.allows(&owner_principal, owner, VfsAccess::Write));

        let group_principal = VfsPrincipal::new(2000, 100);
        assert!(group_principal.in_group(100));
        assert!(mode.allows(&group_principal, owner, VfsAccess::Read));
        assert!(!mode.allows(&group_principal, owner, VfsAccess::Write));

        let other_principal = VfsPrincipal::new(3000, 300);
        assert!(!mode.allows(&other_principal, owner, VfsAccess::Read));
    }

    #[test]
    fn superuser_bypasses_mode_but_not_policy() {
        let permissions = MountPermissions::default().with_rules(vec![VfsPathRule::Deny {
            path: ".git/**".into(),
            access: deny_all_access(),
        }]);
        let root = VfsPrincipal::root();
        assert!(
            permissions
                .authorize(&root, ".git/config", false, VfsAccess::Read)
                .is_err()
        );
    }
}
