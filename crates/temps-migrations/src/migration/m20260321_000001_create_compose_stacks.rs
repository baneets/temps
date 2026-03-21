use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("compose_stacks"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("name"))
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("description"))
                            .string_len(1024)
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("compose_content"))
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("env_content")).text().null())
                    .col(ColumnDef::new(Alias::new("node_id")).integer().null())
                    .col(
                        ColumnDef::new(Alias::new("state"))
                            .string_len(32)
                            .not_null()
                            .default("stopped"),
                    )
                    .col(
                        ColumnDef::new(Alias::new("created_at"))
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Alias::new("updated_at"))
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_compose_stacks_state")
                    .table(Alias::new("compose_stacks"))
                    .col(Alias::new("state"))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_compose_stacks_node_id")
                    .table(Alias::new("compose_stacks"))
                    .col(Alias::new("node_id"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Alias::new("compose_stacks")).to_owned())
            .await?;
        Ok(())
    }
}
