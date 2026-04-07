use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Create workspace_sessions table
        db.execute_unprepared(
            r#"
            CREATE TABLE workspace_sessions (
                id SERIAL PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                status VARCHAR(20) NOT NULL DEFAULT 'active',
                sandbox_container_id VARCHAR(255),
                work_dir TEXT,
                branch_name VARCHAR(255),
                ai_provider VARCHAR(50) NOT NULL DEFAULT 'claude_cli',
                ai_model VARCHAR(100),
                tokens_input INTEGER NOT NULL DEFAULT 0,
                tokens_output INTEGER NOT NULL DEFAULT 0,
                estimated_cost_cents INTEGER NOT NULL DEFAULT 0,
                files_changed INTEGER NOT NULL DEFAULT 0,
                metadata JSONB,
                last_activity_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                closed_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .await?;

        // Create workspace_messages table
        db.execute_unprepared(
            r#"
            CREATE TABLE workspace_messages (
                id BIGSERIAL PRIMARY KEY,
                session_id INTEGER NOT NULL REFERENCES workspace_sessions(id) ON DELETE CASCADE,
                role VARCHAR(20) NOT NULL,
                content TEXT NOT NULL,
                metadata JSONB,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .await?;

        // Indexes
        db.execute_unprepared(
            "CREATE INDEX idx_workspace_sessions_project_id ON workspace_sessions(project_id)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX idx_workspace_sessions_user_id ON workspace_sessions(user_id)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX idx_workspace_sessions_status ON workspace_sessions(status)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX idx_workspace_messages_session_id ON workspace_messages(session_id)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX idx_workspace_messages_session_created ON workspace_messages(session_id, created_at)",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS workspace_messages")
            .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS workspace_sessions")
            .await?;
        Ok(())
    }
}
