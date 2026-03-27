use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ========================================
        // EMAIL_EVENTS TABLE
        // ========================================
        manager
            .create_table(
                Table::create()
                    .table(EmailEvents::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(EmailEvents::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    // Soft reference to emails.id — NO foreign key constraint.
                    // Pixel/click events can arrive for deleted or nonexistent email IDs.
                    .col(ColumnDef::new(EmailEvents::EmailId).uuid().not_null())
                    .col(
                        ColumnDef::new(EmailEvents::EventType)
                            .string_len(50)
                            .not_null(),
                    )
                    // SNS MessageId for dedup via UNIQUE partial index
                    .col(
                        ColumnDef::new(EmailEvents::ProviderMessageId)
                            .string_len(255)
                            .null(),
                    )
                    .col(
                        ColumnDef::new(EmailEvents::Recipient)
                            .string_len(255)
                            .null(),
                    )
                    // Click URL, bounce type/subtype, complaint type
                    .col(ColumnDef::new(EmailEvents::Metadata).json_binary().null())
                    .col(ColumnDef::new(EmailEvents::IpAddress).string_len(45).null())
                    .col(ColumnDef::new(EmailEvents::UserAgent).text().null())
                    .col(
                        ColumnDef::new(EmailEvents::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        // Index: fast lookup for events per email
        manager
            .create_index(
                Index::create()
                    .name("idx_email_events_email_created")
                    .table(EmailEvents::Table)
                    .col(EmailEvents::EmailId)
                    .col((EmailEvents::CreatedAt, IndexOrder::Desc))
                    .to_owned(),
            )
            .await?;

        // Index: aggregate stats by event type
        manager
            .create_index(
                Index::create()
                    .name("idx_email_events_type_created")
                    .table(EmailEvents::Table)
                    .col(EmailEvents::EventType)
                    .col((EmailEvents::CreatedAt, IndexOrder::Desc))
                    .to_owned(),
            )
            .await?;

        // Unique partial index for SNS dedup: INSERT ON CONFLICT DO NOTHING
        // Uses raw SQL because sea-query doesn't support WHERE clauses on indexes
        let db = manager.get_connection();
        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_email_events_provider_msg_id ON email_events (provider_message_id) WHERE provider_message_id IS NOT NULL"
        ).await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(EmailEvents::Table).to_owned())
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum EmailEvents {
    Table,
    Id,
    EmailId,
    EventType,
    ProviderMessageId,
    Recipient,
    Metadata,
    IpAddress,
    UserAgent,
    CreatedAt,
}
