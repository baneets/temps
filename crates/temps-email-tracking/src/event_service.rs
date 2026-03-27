//! Email event service — queries and stats for email tracking events

use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
};
use std::sync::Arc;
use uuid::Uuid;

use temps_entities::email_events;

use crate::errors::EmailTrackingError;

/// Service for querying email tracking events
pub struct EmailEventService {
    db: Arc<DatabaseConnection>,
}

/// Query options for listing email events
#[derive(Debug, Clone, Default)]
pub struct ListEmailEventsOptions {
    pub email_id: Option<Uuid>,
    pub event_type: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

/// Aggregate email event statistics
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct EmailEventStats {
    pub delivered: u64,
    pub opened: u64,
    pub clicked: u64,
    pub bounced: u64,
    pub complained: u64,
    pub open_rate: Option<f64>,
    pub click_rate: Option<f64>,
    pub bounce_rate: Option<f64>,
}

impl EmailEventService {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    /// Record a tracking event (open, click, bounce, complaint, delivery)
    #[allow(clippy::too_many_arguments)]
    pub async fn record_event(
        &self,
        email_id: Uuid,
        event_type: &str,
        provider_message_id: Option<String>,
        recipient: Option<String>,
        metadata: Option<serde_json::Value>,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), EmailTrackingError> {
        use sea_orm::ActiveValue::Set;

        let event = email_events::ActiveModel {
            email_id: Set(email_id),
            event_type: Set(event_type.to_string()),
            provider_message_id: Set(provider_message_id),
            recipient: Set(recipient),
            metadata: Set(metadata),
            ip_address: Set(ip_address),
            user_agent: Set(user_agent),
            ..Default::default()
        };

        // Use insert — for SNS dedup, the unique partial index on provider_message_id
        // means duplicates will cause a constraint violation. We handle that gracefully.
        match email_events::Entity::insert(event)
            .exec(self.db.as_ref())
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                // Check for duplicate key / unique constraint violation (SNS dedup)
                let err_str = format!("{:?}", e);
                if err_str.contains("duplicate key") || err_str.contains("unique constraint") {
                    tracing::debug!("Duplicate event ignored (SNS dedup)");
                    Ok(())
                } else {
                    Err(EmailTrackingError::Database(e))
                }
            }
        }
    }

    /// List events for an email with optional filtering
    pub async fn list_events(
        &self,
        options: ListEmailEventsOptions,
    ) -> Result<(Vec<email_events::Model>, u64), EmailTrackingError> {
        let page = options.page.unwrap_or(1);
        let page_size = std::cmp::min(options.page_size.unwrap_or(20), 100);

        let mut query = email_events::Entity::find().order_by_desc(email_events::Column::CreatedAt);

        if let Some(email_id) = options.email_id {
            query = query.filter(email_events::Column::EmailId.eq(email_id));
        }

        if let Some(event_type) = options.event_type {
            query = query.filter(email_events::Column::EventType.eq(event_type));
        }

        let paginator = query.paginate(self.db.as_ref(), page_size);
        let total = paginator.num_items().await?;
        let items = paginator.fetch_page(page - 1).await?;

        Ok((items, total))
    }

    /// Get aggregate email event statistics
    pub async fn get_stats(
        &self,
        email_id: Option<Uuid>,
    ) -> Result<EmailEventStats, EmailTrackingError> {
        let mut base_query = email_events::Entity::find();

        if let Some(email_id) = email_id {
            base_query = base_query.filter(email_events::Column::EmailId.eq(email_id));
        }

        let delivered = base_query
            .clone()
            .filter(email_events::Column::EventType.eq("delivered"))
            .count(self.db.as_ref())
            .await?;

        let opened = base_query
            .clone()
            .filter(email_events::Column::EventType.eq("opened"))
            .count(self.db.as_ref())
            .await?;

        let clicked = base_query
            .clone()
            .filter(email_events::Column::EventType.eq("clicked"))
            .count(self.db.as_ref())
            .await?;

        let bounced = base_query
            .clone()
            .filter(email_events::Column::EventType.eq("bounced"))
            .count(self.db.as_ref())
            .await?;

        let complained = base_query
            .filter(email_events::Column::EventType.eq("complained"))
            .count(self.db.as_ref())
            .await?;

        let open_rate = if delivered > 0 {
            Some(opened as f64 / delivered as f64)
        } else {
            None
        };

        let click_rate = if delivered > 0 {
            Some(clicked as f64 / delivered as f64)
        } else {
            None
        };

        let bounce_rate = if delivered > 0 {
            Some(bounced as f64 / delivered as f64)
        } else {
            None
        };

        Ok(EmailEventStats {
            delivered,
            opened,
            clicked,
            bounced,
            complained,
            open_rate,
            click_rate,
            bounce_rate,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_options_default() {
        let options = ListEmailEventsOptions::default();
        assert!(options.email_id.is_none());
        assert!(options.event_type.is_none());
        assert!(options.page.is_none());
        assert!(options.page_size.is_none());
    }

    #[test]
    fn test_email_event_stats_rates() {
        let stats = EmailEventStats {
            delivered: 100,
            opened: 40,
            clicked: 10,
            bounced: 5,
            complained: 1,
            open_rate: Some(0.4),
            click_rate: Some(0.1),
            bounce_rate: Some(0.05),
        };

        assert_eq!(stats.delivered, 100);
        assert_eq!(stats.open_rate, Some(0.4));
        assert_eq!(stats.click_rate, Some(0.1));
        assert_eq!(stats.bounce_rate, Some(0.05));
    }

    #[test]
    fn test_email_event_stats_zero_delivered() {
        let stats = EmailEventStats {
            delivered: 0,
            opened: 0,
            clicked: 0,
            bounced: 0,
            complained: 0,
            open_rate: None,
            click_rate: None,
            bounce_rate: None,
        };

        assert!(stats.open_rate.is_none());
        assert!(stats.click_rate.is_none());
        assert!(stats.bounce_rate.is_none());
    }
}
