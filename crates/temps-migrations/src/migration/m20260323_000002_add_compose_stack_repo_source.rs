use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // repo_url: e.g. "https://github.com/user/repo.git"
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("compose_stacks"))
                    .add_column(
                        ColumnDef::new(Alias::new("repo_url"))
                            .string_len(512)
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // repo_branch: e.g. "main"
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("compose_stacks"))
                    .add_column(
                        ColumnDef::new(Alias::new("repo_branch"))
                            .string_len(255)
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // repo_compose_path: e.g. "docker-compose.yml" or "infra/compose.yml"
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("compose_stacks"))
                    .add_column(
                        ColumnDef::new(Alias::new("repo_compose_path"))
                            .string_len(512)
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // repo_access_token: optional, for private repos (encrypted)
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("compose_stacks"))
                    .add_column(
                        ColumnDef::new(Alias::new("repo_access_token"))
                            .text()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // last_synced_at: when the compose file was last fetched from the repo
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("compose_stacks"))
                    .add_column(
                        ColumnDef::new(Alias::new("last_synced_at"))
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for col in [
            "repo_url",
            "repo_branch",
            "repo_compose_path",
            "repo_access_token",
            "last_synced_at",
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("compose_stacks"))
                        .drop_column(Alias::new(col))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}
