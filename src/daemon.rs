//! Daemon process ownership and storage lifecycle.
//!
//! The daemon entry point lives in the crate root (`run`); this module holds the
//! pieces that only the daemon needs: instance locking and identity, plus the
//! on-disk storage migration used by the `migrate-storage` command.

pub mod process;
pub mod storage_migration;
