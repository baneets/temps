use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("compose_stack_routes"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("stack_id")).integer().not_null())
                    .col(
                        ColumnDef::new(Alias::new("domain"))
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("target_port"))
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("service_name"))
                            .string_len(255)
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("enabled"))
                            .boolean()
                            .not_null()
                            .default(true),
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
                    .foreign_key(
                        ForeignKey::create()
                            .from(Alias::new("compose_stack_routes"), Alias::new("stack_id"))
                            .to(Alias::new("compose_stacks"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Unique index on domain for fast lookup and uniqueness
        manager
            .create_index(
                Index::create()
                    .name("idx_compose_stack_routes_domain")
                    .table(Alias::new("compose_stack_routes"))
                    .col(Alias::new("domain"))
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Index on stack_id for listing routes per stack
        manager
            .create_index(
                Index::create()
                    .name("idx_compose_stack_routes_stack_id")
                    .table(Alias::new("compose_stack_routes"))
                    .col(Alias::new("stack_id"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("compose_stack_routes"))
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
