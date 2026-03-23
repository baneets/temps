use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // port_overrides: JSON mapping of original_port -> new_port
        // e.g. {"8080": 9090, "3000": 3001}
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("compose_stacks"))
                    .add_column(
                        ColumnDef::new(Alias::new("port_overrides"))
                            .json_binary()
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
                    .table(Alias::new("compose_stacks"))
                    .drop_column(Alias::new("port_overrides"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
