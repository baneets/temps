use bollard::query_parameters::ListContainersOptions;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::net::TcpListener;

/// A host port binding extracted from a compose file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PortBinding {
    /// Host port number
    pub host_port: u16,
    /// Protocol (tcp or udp)
    pub protocol: String,
    /// The service name from the compose file
    pub service: String,
}

/// A port conflict detected before deploy.
#[derive(Debug, Clone)]
pub struct PortConflict {
    pub host_port: u16,
    pub protocol: String,
    /// Service in the compose file that wants this port
    pub requesting_service: String,
    /// What currently owns the port
    pub owner: PortOwner,
}

#[derive(Debug, Clone)]
pub enum PortOwner {
    /// Another compose stack owns this port
    Stack { stack_id: i32, stack_name: String },
    /// A system process is listening on this port
    System,
    /// A stack route is using this port for domain routing
    Route {
        stack_id: i32,
        stack_name: String,
        domain: String,
    },
}

impl std::fmt::Display for PortConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.owner {
            PortOwner::Stack {
                stack_name,
                stack_id,
            } => write!(
                f,
                "Port {}/{} (requested by service '{}') is already used by stack '{}' (id: {})",
                self.host_port, self.protocol, self.requesting_service, stack_name, stack_id
            ),
            PortOwner::System => write!(
                f,
                "Port {}/{} (requested by service '{}') is already in use by another process on this host",
                self.host_port, self.protocol, self.requesting_service
            ),
            PortOwner::Route {
                stack_name,
                stack_id,
                domain,
            } => write!(
                f,
                "Port {}/{} (requested by service '{}') is already routed to stack '{}' (id: {}) via domain '{}'",
                self.host_port, self.protocol, self.requesting_service, stack_name, stack_id, domain
            ),
        }
    }
}

/// Minimal compose file structure for port extraction.
#[derive(Deserialize, Default)]
struct ComposeFile {
    services: Option<HashMap<String, ComposeService>>,
}

#[derive(Deserialize, Default)]
struct ComposeService {
    ports: Option<Vec<serde_yaml::Value>>,
}

/// Extract host port bindings from a docker-compose YAML string.
pub fn extract_ports(compose_content: &str) -> Result<Vec<PortBinding>, String> {
    let compose: ComposeFile = serde_yaml::from_str(compose_content)
        .map_err(|e| format!("Invalid compose YAML: {}", e))?;

    let services = match compose.services {
        Some(s) => s,
        None => return Ok(Vec::new()),
    };

    let mut bindings = Vec::new();

    for (service_name, service) in &services {
        let ports = match &service.ports {
            Some(p) => p,
            None => continue,
        };

        for port_val in ports {
            match port_val {
                serde_yaml::Value::String(s) => {
                    if let Some(binding) = parse_port_string(s, service_name) {
                        bindings.push(binding);
                    }
                }
                serde_yaml::Value::Number(n) => {
                    // Simple number like `8080` means container port only (no host binding)
                    // unless it's a short form like 8080:8080
                    if let Some(port) = n.as_u64().and_then(|p| u16::try_from(p).ok()) {
                        bindings.push(PortBinding {
                            host_port: port,
                            protocol: "tcp".to_string(),
                            service: service_name.to_string(),
                        });
                    }
                }
                serde_yaml::Value::Mapping(m) => {
                    // Long syntax: { target: 80, published: 8080, protocol: tcp }
                    let published = m
                        .get(serde_yaml::Value::String("published".into()))
                        .and_then(|v| match v {
                            serde_yaml::Value::Number(n) => {
                                n.as_u64().and_then(|p| u16::try_from(p).ok())
                            }
                            serde_yaml::Value::String(s) => s.parse().ok(),
                            _ => None,
                        });

                    let protocol = m
                        .get(serde_yaml::Value::String("protocol".into()))
                        .and_then(|v| v.as_str())
                        .unwrap_or("tcp")
                        .to_string();

                    if let Some(port) = published {
                        bindings.push(PortBinding {
                            host_port: port,
                            protocol,
                            service: service_name.clone(),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    Ok(bindings)
}

/// Parse a port string like "8080:80", "127.0.0.1:8080:80/udp", "8080:80/tcp"
fn parse_port_string(s: &str, service_name: &str) -> Option<PortBinding> {
    // Split off protocol
    let (port_part, protocol) = if let Some(idx) = s.rfind('/') {
        (&s[..idx], s[idx + 1..].to_string())
    } else {
        (s, "tcp".to_string())
    };

    let parts: Vec<&str> = port_part.split(':').collect();

    let host_port = match parts.len() {
        1 => {
            // Just container port, no host binding
            // Actually in compose, a single port means expose only, not bound to host
            return None;
        }
        2 => {
            // host_port:container_port
            parse_port_or_range(parts[0])?
        }
        3 => {
            // ip:host_port:container_port
            parse_port_or_range(parts[1])?
        }
        _ => return None,
    };

    Some(PortBinding {
        host_port,
        protocol,
        service: service_name.to_string(),
    })
}

fn parse_port_or_range(s: &str) -> Option<u16> {
    // Handle port ranges like "8080-8090" by taking the start
    if let Some(idx) = s.find('-') {
        s[..idx].parse().ok()
    } else {
        s.parse().ok()
    }
}

/// Check if a port is available on the host by attempting to bind.
fn is_port_available(port: u16) -> bool {
    TcpListener::bind(("0.0.0.0", port)).is_ok()
}

/// Validate ports against running Docker containers and system ports.
/// Returns a list of conflicts (empty = all clear).
pub async fn validate_ports(bindings: &[PortBinding], current_stack_id: i32) -> Vec<PortConflict> {
    if bindings.is_empty() {
        return Vec::new();
    }

    let mut conflicts = Vec::new();

    // 1. Check against other Docker containers
    let docker_conflicts = check_docker_ports(bindings, current_stack_id).await;
    conflicts.extend(docker_conflicts);

    // 2. Check system ports (only for ports not already flagged)
    let flagged: HashSet<u16> = conflicts.iter().map(|c| c.host_port).collect();
    for binding in bindings {
        if flagged.contains(&binding.host_port) {
            continue;
        }
        if !is_port_available(binding.host_port) {
            conflicts.push(PortConflict {
                host_port: binding.host_port,
                protocol: binding.protocol.clone(),
                requesting_service: binding.service.clone(),
                owner: PortOwner::System,
            });
        }
    }

    conflicts
}

/// Check ports against running Docker containers managed by temps.
async fn check_docker_ports(bindings: &[PortBinding], current_stack_id: i32) -> Vec<PortConflict> {
    let docker = match bollard::Docker::connect_with_defaults() {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    // List all running containers with temps compose labels
    let containers = match docker
        .list_containers(Some(ListContainersOptions {
            all: false,
            filters: Some(HashMap::from([(
                "label".to_string(),
                vec!["com.docker.compose.project".to_string()],
            )])),
            ..Default::default()
        }))
        .await
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut conflicts = Vec::new();
    let wanted_ports: HashSet<u16> = bindings.iter().map(|b| b.host_port).collect();

    for container in &containers {
        let project = container
            .labels
            .as_ref()
            .and_then(|l| l.get("com.docker.compose.project"))
            .cloned()
            .unwrap_or_default();

        // Extract stack_id from project name "temps-stack-{id}"
        let container_stack_id = project
            .strip_prefix("temps-stack-")
            .and_then(|s| s.parse::<i32>().ok());

        // Skip containers belonging to the current stack (they'll be recreated)
        if container_stack_id == Some(current_stack_id) {
            continue;
        }

        // Check published ports
        if let Some(ports) = &container.ports {
            for port in ports {
                let public_port = port.public_port;
                if let Some(pp) = public_port {
                    if wanted_ports.contains(&pp) {
                        let service = container
                            .labels
                            .as_ref()
                            .and_then(|l| l.get("com.docker.compose.service"))
                            .cloned()
                            .unwrap_or_default();

                        // Find which of our bindings wants this port
                        for binding in bindings {
                            if binding.host_port == pp {
                                conflicts.push(PortConflict {
                                    host_port: pp,
                                    protocol: binding.protocol.clone(),
                                    requesting_service: binding.service.clone(),
                                    owner: PortOwner::Stack {
                                        stack_id: container_stack_id.unwrap_or(0),
                                        stack_name: format!("{} (service: {})", project, service),
                                    },
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    conflicts
}

/// Apply port overrides to a compose YAML string.
/// `overrides` maps original host port (as string) to new host port.
/// Returns the modified YAML.
pub fn apply_port_overrides(
    compose_content: &str,
    overrides: &HashMap<String, u16>,
) -> Result<String, String> {
    if overrides.is_empty() {
        return Ok(compose_content.to_string());
    }

    let mut doc: serde_yaml::Value = serde_yaml::from_str(compose_content)
        .map_err(|e| format!("Invalid compose YAML: {}", e))?;

    let services = match doc.get_mut("services").and_then(|s| s.as_mapping_mut()) {
        Some(s) => s,
        None => return Ok(compose_content.to_string()),
    };

    for (_service_name, service) in services.iter_mut() {
        let ports = match service.get_mut("ports").and_then(|p| p.as_sequence_mut()) {
            Some(p) => p,
            None => continue,
        };

        for port_val in ports.iter_mut() {
            match port_val {
                serde_yaml::Value::String(s) => {
                    if let Some(new_s) = rewrite_port_string(s, overrides) {
                        *s = new_s;
                    }
                }
                serde_yaml::Value::Mapping(m) => {
                    // Long syntax: { target: 80, published: 8080 }
                    let published_key = serde_yaml::Value::String("published".into());
                    if let Some(published) = m.get(&published_key).cloned() {
                        let port_str = match &published {
                            serde_yaml::Value::Number(n) => n.to_string(),
                            serde_yaml::Value::String(s) => s.clone(),
                            _ => continue,
                        };
                        if let Some(&new_port) = overrides.get(&port_str) {
                            m.insert(
                                published_key,
                                serde_yaml::Value::Number(serde_yaml::Number::from(new_port)),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }

    serde_yaml::to_string(&doc).map_err(|e| format!("Failed to serialize compose YAML: {}", e))
}

/// Rewrite a port string like "8080:80" to "9090:80" based on overrides.
fn rewrite_port_string(s: &str, overrides: &HashMap<String, u16>) -> Option<String> {
    // Split off protocol
    let (port_part, protocol_suffix) = if let Some(idx) = s.rfind('/') {
        (&s[..idx], &s[idx..])
    } else {
        (s, "")
    };

    let parts: Vec<&str> = port_part.split(':').collect();

    match parts.len() {
        2 => {
            // host_port:container_port
            if let Some(&new_port) = overrides.get(parts[0]) {
                Some(format!("{}:{}{}", new_port, parts[1], protocol_suffix))
            } else {
                None
            }
        }
        3 => {
            // ip:host_port:container_port
            if let Some(&new_port) = overrides.get(parts[1]) {
                Some(format!(
                    "{}:{}:{}{}",
                    parts[0], new_port, parts[2], protocol_suffix
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Find the next available port starting from `start`.
pub fn find_available_port(start: u16) -> Option<u16> {
    (start..=65535).find(|&port| is_port_available(port))
}

/// Suggest port overrides to resolve conflicts.
/// Returns a map of conflicting_port -> suggested_available_port.
pub fn suggest_overrides(conflicts: &[PortConflict]) -> HashMap<String, u16> {
    let mut suggestions = HashMap::new();
    let mut next_candidate = 9000u16;

    for conflict in conflicts {
        let key = conflict.host_port.to_string();
        if suggestions.contains_key(&key) {
            continue;
        }
        // Start searching from max(conflict port + 1, next_candidate)
        let start = std::cmp::max(conflict.host_port + 1, next_candidate);
        if let Some(available) = find_available_port(start) {
            suggestions.insert(key, available);
            next_candidate = available + 1;
        }
    }

    suggestions
}

/// Deduplicate conflicts by (host_port, protocol) pair.
/// When the same port is flagged multiple times (e.g., by Docker and routes),
/// keep only the most specific conflict (Route > Stack > System).
pub fn deduplicate_conflicts(conflicts: Vec<PortConflict>) -> Vec<PortConflict> {
    let mut seen: HashMap<(u16, String), PortConflict> = HashMap::new();
    for conflict in conflicts {
        let key = (conflict.host_port, conflict.protocol.clone());
        let entry = seen.entry(key).or_insert(conflict.clone());
        // Prefer Route > Stack > System (more specific info)
        match (&entry.owner, &conflict.owner) {
            (PortOwner::System, PortOwner::Stack { .. } | PortOwner::Route { .. })
            | (PortOwner::Stack { .. }, PortOwner::Route { .. }) => {
                *entry = conflict;
            }
            _ => {}
        }
    }
    seen.into_values().collect()
}

/// Format multiple port conflicts into a user-friendly error message.
pub fn format_conflicts(conflicts: &[PortConflict]) -> String {
    if conflicts.len() == 1 {
        return conflicts[0].to_string();
    }

    let mut msg = format!("{} port conflicts detected:\n", conflicts.len());
    for (i, conflict) in conflicts.iter().enumerate() {
        msg.push_str(&format!("  {}. {}\n", i + 1, conflict));
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_simple_ports() {
        let yaml = r#"
services:
  web:
    image: nginx
    ports:
      - "8080:80"
      - "8443:443"
"#;
        let bindings = extract_ports(yaml).unwrap();
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].host_port, 8080);
        assert_eq!(bindings[0].service, "web");
        assert_eq!(bindings[1].host_port, 8443);
    }

    #[test]
    fn test_extract_ports_with_protocol() {
        let yaml = r#"
services:
  dns:
    image: coredns
    ports:
      - "53:53/udp"
      - "53:53/tcp"
"#;
        let bindings = extract_ports(yaml).unwrap();
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].protocol, "udp");
        assert_eq!(bindings[1].protocol, "tcp");
    }

    #[test]
    fn test_extract_ports_with_ip() {
        let yaml = r#"
services:
  web:
    image: nginx
    ports:
      - "127.0.0.1:9090:80"
"#;
        let bindings = extract_ports(yaml).unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].host_port, 9090);
    }

    #[test]
    fn test_extract_long_syntax() {
        let yaml = r#"
services:
  web:
    image: nginx
    ports:
      - target: 80
        published: 8080
        protocol: tcp
"#;
        let bindings = extract_ports(yaml).unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].host_port, 8080);
        assert_eq!(bindings[0].protocol, "tcp");
    }

    #[test]
    fn test_extract_no_ports() {
        let yaml = r#"
services:
  worker:
    image: myapp
"#;
        let bindings = extract_ports(yaml).unwrap();
        assert!(bindings.is_empty());
    }

    #[test]
    fn test_extract_no_services() {
        let yaml = "version: '3'\n";
        let bindings = extract_ports(yaml).unwrap();
        assert!(bindings.is_empty());
    }

    #[test]
    fn test_extract_container_only_port() {
        let yaml = r#"
services:
  web:
    image: nginx
    ports:
      - "80"
"#;
        let bindings = extract_ports(yaml).unwrap();
        // Single port string means expose only, no host binding
        assert!(bindings.is_empty());
    }

    #[test]
    fn test_invalid_yaml() {
        let result = extract_ports("not: valid: yaml: [[[");
        assert!(result.is_err());
    }

    #[test]
    fn test_conflict_display() {
        let conflict = PortConflict {
            host_port: 8080,
            protocol: "tcp".to_string(),
            requesting_service: "web".to_string(),
            owner: PortOwner::Stack {
                stack_id: 3,
                stack_name: "my-stack".to_string(),
            },
        };
        let msg = conflict.to_string();
        assert!(msg.contains("8080"));
        assert!(msg.contains("my-stack"));
        assert!(msg.contains("web"));
    }

    #[test]
    fn test_route_conflict_display() {
        let conflict = PortConflict {
            host_port: 3000,
            protocol: "tcp".to_string(),
            requesting_service: "app".to_string(),
            owner: PortOwner::Route {
                stack_id: 5,
                stack_name: "other-stack".to_string(),
                domain: "app.example.com".to_string(),
            },
        };
        let msg = conflict.to_string();
        assert!(msg.contains("3000"));
        assert!(msg.contains("other-stack"));
        assert!(msg.contains("app.example.com"));
    }

    #[test]
    fn test_apply_port_overrides_short_syntax() {
        let yaml = r#"
services:
  web:
    image: nginx
    ports:
      - "8080:80"
      - "8443:443"
"#;
        let mut overrides = HashMap::new();
        overrides.insert("8080".to_string(), 9090u16);

        let result = apply_port_overrides(yaml, &overrides).unwrap();
        assert!(result.contains("9090:80"));
        assert!(result.contains("8443:443"));
        assert!(!result.contains("8080:80"));
    }

    #[test]
    fn test_apply_port_overrides_with_ip() {
        let yaml = r#"
services:
  web:
    image: nginx
    ports:
      - "127.0.0.1:8080:80"
"#;
        let mut overrides = HashMap::new();
        overrides.insert("8080".to_string(), 9090u16);

        let result = apply_port_overrides(yaml, &overrides).unwrap();
        assert!(result.contains("127.0.0.1:9090:80"));
    }

    #[test]
    fn test_apply_port_overrides_with_protocol() {
        let yaml = r#"
services:
  dns:
    image: coredns
    ports:
      - "53:53/udp"
"#;
        let mut overrides = HashMap::new();
        overrides.insert("53".to_string(), 5353u16);

        let result = apply_port_overrides(yaml, &overrides).unwrap();
        assert!(result.contains("5353:53/udp"));
    }

    #[test]
    fn test_apply_port_overrides_empty() {
        let yaml = "services:\n  web:\n    image: nginx\n";
        let overrides = HashMap::new();
        let result = apply_port_overrides(yaml, &overrides).unwrap();
        assert!(result.contains("nginx"));
    }

    #[test]
    fn test_deduplicate_conflicts() {
        let conflicts = vec![
            PortConflict {
                host_port: 8080,
                protocol: "tcp".to_string(),
                requesting_service: "web".to_string(),
                owner: PortOwner::System,
            },
            PortConflict {
                host_port: 8080,
                protocol: "tcp".to_string(),
                requesting_service: "web".to_string(),
                owner: PortOwner::Stack {
                    stack_id: 1,
                    stack_name: "other".to_string(),
                },
            },
            PortConflict {
                host_port: 8080,
                protocol: "tcp".to_string(),
                requesting_service: "web".to_string(),
                owner: PortOwner::Route {
                    stack_id: 1,
                    stack_name: "other".to_string(),
                    domain: "example.com".to_string(),
                },
            },
        ];
        let deduped = deduplicate_conflicts(conflicts);
        assert_eq!(deduped.len(), 1);
        // Should keep the Route variant (most specific)
        assert!(matches!(deduped[0].owner, PortOwner::Route { .. }));
    }
}
