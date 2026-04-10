use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Project-level skill definitions — shared skills referenced by workflows via slug
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS project_skill_definitions (
                id SERIAL PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                slug VARCHAR NOT NULL,
                name VARCHAR NOT NULL,
                description TEXT DEFAULT NULL,
                content TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                UNIQUE(project_id, slug)
            )",
        )
        .await?;

        // Project-level MCP server definitions — shared MCP configs referenced by workflows via slug
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS project_mcp_definitions (
                id SERIAL PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                slug VARCHAR NOT NULL,
                name VARCHAR NOT NULL,
                description TEXT DEFAULT NULL,
                config JSONB NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                UNIQUE(project_id, slug)
            )",
        )
        .await?;

        // Change project_agents: skills_config and mcp_servers_config now store slug arrays
        // e.g. ["blog-writer", "seo-optimizer"] instead of inline content
        // Existing JSONB columns are reused — old inline data will be ignored by the new code

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS project_mcp_definitions")
            .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS project_skill_definitions")
            .await?;
        Ok(())
    }
}
