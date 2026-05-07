//! Add `preview_password_encrypted` to `workspace_sessions`.
//!
//! Previously, the per-session preview password was only stored as an
//! argon2 PHC hash plus a 4-char hint. The plaintext was returned exactly
//! once at create/regenerate and was unrecoverable afterwards.
//!
//! This column adds an AES-256-GCM ciphertext of the plaintext (using the
//! platform `EncryptionService`) so the password can be returned by
//! subsequent `GET /sessions` reads without forcing the user to regenerate.
//! The argon2 hash is kept around so existing sessions whose plaintext was
//! never persisted continue to validate at the preview gateway until the
//! user regenerates them — at which point the new code populates both
//! columns.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(WorkspaceSessions::Table)
                    .add_column(
                        ColumnDef::new(WorkspaceSessions::PreviewPasswordEncrypted)
                            .text()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(WorkspaceSessions::Table)
                    .drop_column(WorkspaceSessions::PreviewPasswordEncrypted)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum WorkspaceSessions {
    Table,
    PreviewPasswordEncrypted,
}
