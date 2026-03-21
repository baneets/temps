use anyhow::Result;
use serde::Serialize;
pub use temps_core::AuditContext;
use temps_core::AuditOperation;

#[derive(Debug, Clone, Serialize)]
pub struct StackCreatedAudit {
    pub context: AuditContext,
    pub stack_id: i32,
    pub name: String,
}

impl AuditOperation for StackCreatedAudit {
    fn operation_type(&self) -> String {
        "COMPOSE_STACK_CREATED".to_string()
    }

    fn user_id(&self) -> i32 {
        self.context.user_id
    }

    fn ip_address(&self) -> Option<String> {
        self.context.ip_address.clone()
    }

    fn user_agent(&self) -> &str {
        &self.context.user_agent
    }

    fn serialize(&self) -> Result<String> {
        serde_json::to_string(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize audit operation {}", e))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StackUpdatedAudit {
    pub context: AuditContext,
    pub stack_id: i32,
    pub name: String,
}

impl AuditOperation for StackUpdatedAudit {
    fn operation_type(&self) -> String {
        "COMPOSE_STACK_UPDATED".to_string()
    }

    fn user_id(&self) -> i32 {
        self.context.user_id
    }

    fn ip_address(&self) -> Option<String> {
        self.context.ip_address.clone()
    }

    fn user_agent(&self) -> &str {
        &self.context.user_agent
    }

    fn serialize(&self) -> Result<String> {
        serde_json::to_string(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize audit operation {}", e))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StackDeletedAudit {
    pub context: AuditContext,
    pub stack_id: i32,
    pub name: String,
}

impl AuditOperation for StackDeletedAudit {
    fn operation_type(&self) -> String {
        "COMPOSE_STACK_DELETED".to_string()
    }

    fn user_id(&self) -> i32 {
        self.context.user_id
    }

    fn ip_address(&self) -> Option<String> {
        self.context.ip_address.clone()
    }

    fn user_agent(&self) -> &str {
        &self.context.user_agent
    }

    fn serialize(&self) -> Result<String> {
        serde_json::to_string(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize audit operation {}", e))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StackStateChangedAudit {
    pub context: AuditContext,
    pub stack_id: i32,
    pub name: String,
    pub new_state: String,
}

impl AuditOperation for StackStateChangedAudit {
    fn operation_type(&self) -> String {
        "COMPOSE_STACK_STATE_CHANGED".to_string()
    }

    fn user_id(&self) -> i32 {
        self.context.user_id
    }

    fn ip_address(&self) -> Option<String> {
        self.context.ip_address.clone()
    }

    fn user_agent(&self) -> &str {
        &self.context.user_agent
    }

    fn serialize(&self) -> Result<String> {
        serde_json::to_string(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize audit operation {}", e))
    }
}
