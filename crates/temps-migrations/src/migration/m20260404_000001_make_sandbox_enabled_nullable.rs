use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Make sandbox_enabled nullable so NULL = "use global default"
        // true = force sandbox on, false = force sandbox off
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE project_agents ALTER COLUMN sandbox_enabled DROP NOT NULL, ALTER COLUMN sandbox_enabled SET DEFAULT NULL",
        )
        .await?;

        // Set all existing false values to NULL (= use global default)
        db.execute_unprepared(
            "UPDATE project_agents SET sandbox_enabled = NULL WHERE sandbox_enabled = false",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "UPDATE project_agents SET sandbox_enabled = false WHERE sandbox_enabled IS NULL",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE project_agents ALTER COLUMN sandbox_enabled SET NOT NULL, ALTER COLUMN sandbox_enabled SET DEFAULT false",
        )
        .await?;
        Ok(())
    }
}
