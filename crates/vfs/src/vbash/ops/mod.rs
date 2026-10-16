//! Operation implementations for the VFS bash tool.
//!
//! Every function takes an [`crate::storage::OpendalFileStorage`] reference
//! and returns a [`ToolResult`]. No subprocess is spawned anywhere in this
//! module tree.
//!
//! Submodules are grouped by operation family:
//! - [`common`] — shared helpers, constants, binary detection
//! - [`read`] — `read`, `cat`, `head`, `tail`
//! - [`write`] — `write`, `edit`, `patch`, `touch`
//! - [`list`] — `ls`, `tree`, `wc`, `stat`, `mount_list`
//! - [`file`] — `mkdir`, `rm`, `cp`, `mv`

mod common;
mod file;
mod list;
mod read;
mod write;

pub use file::{op_cp, op_mkdir, op_mv, op_rm};
pub(crate) use common::is_binary;
pub use list::{op_ls, op_mount_list, op_stat, op_tree, op_wc};
pub use read::{op_cat, op_head, op_read, op_tail};
pub use write::{op_edit, op_patch, op_touch, op_write};
