//! In-memory map from sandbox internal ID → `SandboxHandle`. Keeps live
//! container handles for the lifetime of the server process.
//!
//! Separate from `temps-agents::SandboxRegistry` because (a) we own
//! lifecycle differently (standalone sandboxes don't tie to agent runs)
//! and (b) `release` here marks the DB row "destroyed" rather than
//! deleting per-run data.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use temps_agents::error::AgentError;
use temps_agents::sandbox::{SandboxCreateConfig, SandboxHandle, SandboxProvider};

pub struct StandaloneSandboxRegistry {
    provider: Arc<dyn SandboxProvider>,
    handles: RwLock<HashMap<i32, SandboxHandle>>,
}

impl StandaloneSandboxRegistry {
    pub fn new(provider: Arc<dyn SandboxProvider>) -> Self {
        Self {
            provider,
            handles: RwLock::new(HashMap::new()),
        }
    }

    pub fn provider(&self) -> &dyn SandboxProvider {
        self.provider.as_ref()
    }

    pub fn provider_arc(&self) -> Arc<dyn SandboxProvider> {
        self.provider.clone()
    }

    /// Create and register a new sandbox for the given internal ID.
    pub async fn create(&self, config: SandboxCreateConfig) -> Result<SandboxHandle, AgentError> {
        let id = config.run_id;
        let handle = self.provider.create(config).await?;
        self.handles.write().await.insert(id, handle.clone());
        Ok(handle)
    }

    /// Look up an existing handle. Verifies liveness — returns
    /// `SandboxNotFound` if the provider reports the container is gone
    /// so callers see a consistent "it's missing" story whether the
    /// registry forgot it or the container died externally.
    ///
    /// `public_id` is the full `sbx_<hex>` identifier. The registry
    /// derives the container label from it so recovery after a server
    /// restart finds the container by name.
    pub async fn get(&self, id: i32, public_id: &str) -> Result<SandboxHandle, AgentError> {
        let handle = {
            let g = self.handles.read().await;
            g.get(&id).cloned()
        };
        let handle = match handle {
            Some(h) => h,
            None => {
                // Server may have restarted — try recovery by container name.
                let label = public_id.strip_prefix("sbx_").unwrap_or(public_id);
                if let Some(recovered) = self.provider.recover_by_name(label).await? {
                    self.handles.write().await.insert(id, recovered.clone());
                    return Ok(recovered);
                }
                return Err(AgentError::SandboxNotFound { run_id: id });
            }
        };
        if !self.provider.is_alive(&handle).await.unwrap_or(false) {
            return Err(AgentError::SandboxNotFound { run_id: id });
        }
        Ok(handle)
    }

    /// Remove the handle from the registry and destroy the underlying
    /// container + volumes. Safe to call twice; missing handles are a
    /// no-op.
    pub async fn destroy(&self, id: i32) -> Result<(), AgentError> {
        let handle = self.handles.write().await.remove(&id);
        if let Some(handle) = handle {
            if let Err(e) = self.provider.destroy(&handle, true).await {
                tracing::warn!(
                    "Failed to destroy standalone sandbox {} (internal id {}): {}",
                    handle.sandbox_name,
                    id,
                    e
                );
                return Err(e);
            }
        }
        Ok(())
    }

    /// Stop the container without destroying it. For lifecycle transitions
    /// that pause a sandbox. Callers own the DB state.
    pub async fn stop(&self, id: i32) -> Result<(), AgentError> {
        let handle = {
            let g = self.handles.read().await;
            g.get(&id).cloned()
        };
        if let Some(handle) = handle {
            self.provider.stop(&handle).await?;
        }
        Ok(())
    }

    /// Start a previously-stopped container without re-creating it.
    pub async fn start(&self, id: i32) -> Result<(), AgentError> {
        let handle = {
            let g = self.handles.read().await;
            g.get(&id).cloned()
        };
        if let Some(handle) = handle {
            self.provider.start(&handle).await?;
        }
        Ok(())
    }

    /// Restart a container (stop + start) preserving its filesystem.
    pub async fn restart(&self, id: i32) -> Result<(), AgentError> {
        let handle = {
            let g = self.handles.read().await;
            g.get(&id).cloned()
        };
        if let Some(handle) = handle {
            self.provider.restart(&handle).await?;
        }
        Ok(())
    }

    /// Recover handles for sandboxes that were running when the server
    /// last stopped. Called on startup. Unlike `StandaloneSandboxRegistry::get`,
    /// this doesn't error on missing containers — it just skips them and
    /// returns the count of successfully recovered handles so the plugin
    /// can log a summary.
    ///
    /// `entries` pairs each sandbox's internal numeric id with its
    /// container label (hex suffix of `public_id`). The provider looks
    /// up by label; the registry keys by numeric id for in-memory lookup.
    pub async fn recover_active(&self, entries: &[(i32, String)]) -> usize {
        let mut recovered = 0;
        for (id, label) in entries {
            match self.provider.recover_by_name(label).await {
                Ok(Some(handle)) => {
                    self.handles.write().await.insert(*id, handle);
                    recovered += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        "Failed to recover standalone sandbox for id {} ({}): {}",
                        id,
                        label,
                        e
                    );
                }
            }
        }
        recovered
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }
}
