#!/usr/bin/env node

/**
 * Integration test suite for the Temps MCP Server
 *
 * Runs every tool against a live Temps development instance.
 * Tests are organized into groups with dependency chains:
 *   1. Read-only / safe tools first (no side effects)
 *   2. CRUD lifecycle tests (create -> read -> update -> delete)
 *   3. Dependent tools that need resources from previous steps
 *
 * Usage:
 *   TEMPS_API_URL=http://localhost:8081 TEMPS_API_KEY=<key> bun run test
 *   TEMPS_API_URL=http://localhost:8081 TEMPS_API_KEY=<key> bun run test -- --group projects
 *   TEMPS_API_URL=http://localhost:8081 TEMPS_API_KEY=<key> bun run test -- --only list_projects
 *
 * Environment:
 *   TEMPS_API_URL   - Required. Temps API URL
 *   TEMPS_API_KEY   - Required. API key with full permissions
 *   TEST_PROJECT_ID - Optional. Existing project ID to use for read tests (skips creation)
 */

import { callTool, listTools, toolCount } from '../tools/index.js';
import type { ToolResult } from '../types/index.js';

// ─── Test Infrastructure ─────────────────────────────────────────

interface TestResult {
  tool: string;
  group: string;
  status: 'pass' | 'fail' | 'skip';
  durationMs: number;
  error?: string;
  response?: string;
}

interface TestContext {
  /** IDs captured during test runs for use in subsequent tests */
  projectId?: number;
  projectSlug?: string;
  environmentId?: number;
  environmentName?: string;
  deploymentId?: number;
  /** Project ID that owns the deployment (may differ from test project) */
  _deploymentProjectId?: number;
  serviceId?: number;
  apiKeyId?: number;
  webhookId?: number;
  monitorId?: number;
  incidentId?: number;
  funnelId?: number;
  dsnId?: number;
  ipRuleId?: number;
  customDomainId?: number;
  envVarId?: number;
  tokenId?: number;
  scanId?: number;
  userId?: number;
  s3SourceId?: number;
  backupScheduleId?: number;
  notificationProviderId?: number;
  emailDomainId?: number;
  errorGroupId?: number;
}

const results: TestResult[] = [];
const ctx: TestContext = {};
const CLEANUP_IDS: Array<{ tool: string; args: Record<string, unknown> }> = [];

// ─── Helpers ─────────────────────────────────────────────────────

function extractId(result: ToolResult, pattern?: RegExp): number | undefined {
  const text = result.content[0]?.text ?? '';
  // Try common patterns
  const patterns = pattern
    ? [pattern]
    : [
        /\| ID \| (\d+) \|/,
        /\bid["\s:]*(\d+)/i,
        /"id"\s*:\s*(\d+)/,
        /ID:\s*(\d+)/,
        /created.*?(\d+)/i,
      ];
  for (const p of patterns) {
    const m = text.match(p);
    if (m) return Number(m[1]);
  }
  return undefined;
}

function extractSlug(result: ToolResult): string | undefined {
  const text = result.content[0]?.text ?? '';
  const m = text.match(/\| Slug \| ([^\s|]+)/);
  return m ? m[1] : undefined;
}

function extractFromJson<T = unknown>(result: ToolResult, key: string): T | undefined {
  const text = result.content[0]?.text ?? '';
  const jsonMatch = text.match(/```json\n([\s\S]*?)\n```/);
  if (jsonMatch) {
    try {
      const data = JSON.parse(jsonMatch[1]);
      return data[key] as T;
    } catch { /* ignore */ }
  }
  // Try raw JSON parsing
  try {
    const data = JSON.parse(text);
    return data[key] as T;
  } catch { /* ignore */ }
  return undefined;
}

async function runTest(
  group: string,
  tool: string,
  args: Record<string, unknown>,
  options?: {
    expectError?: boolean;
    skipIf?: () => boolean;
    skipReason?: string;
    extract?: (result: ToolResult) => void;
  }
): Promise<TestResult> {
  if (options?.skipIf?.()) {
    const r: TestResult = {
      tool,
      group,
      status: 'skip',
      durationMs: 0,
      error: options.skipReason ?? 'Skipped (missing dependency)',
    };
    results.push(r);
    return r;
  }

  const start = performance.now();
  let result: ToolResult;

  try {
    result = await callTool(tool, args);
  } catch (error) {
    const r: TestResult = {
      tool,
      group,
      status: 'fail',
      durationMs: performance.now() - start,
      error: `Exception: ${error instanceof Error ? error.message : String(error)}`,
    };
    results.push(r);
    return r;
  }

  const durationMs = performance.now() - start;
  const text = result.content[0]?.text ?? '';
  const isError = result.isError === true || text.startsWith('Error:');

  if (options?.expectError) {
    // We expected an error
    const r: TestResult = {
      tool,
      group,
      status: isError ? 'pass' : 'fail',
      durationMs,
      error: isError ? undefined : 'Expected error but got success',
      response: text.substring(0, 200),
    };
    results.push(r);
    return r;
  }

  if (isError) {
    const r: TestResult = {
      tool,
      group,
      status: 'fail',
      durationMs,
      error: text.substring(0, 500),
    };
    results.push(r);
    return r;
  }

  // Success — run extraction
  try {
    options?.extract?.(result);
  } catch (err) {
    // Extraction failure is not a test failure
    console.error(`  [warn] extraction failed for ${tool}: ${err}`);
  }

  const r: TestResult = {
    tool,
    group,
    status: 'pass',
    durationMs,
    response: text.substring(0, 150),
  };
  results.push(r);
  return r;
}

function registerCleanup(tool: string, args: Record<string, unknown>) {
  CLEANUP_IDS.push({ tool, args });
}

// ─── Test Groups ─────────────────────────────────────────────────

async function testPlatform() {
  const g = 'platform';
  await runTest(g, 'get_platform_info', {});
  await runTest(g, 'get_platform_access', {});
  await runTest(g, 'get_platform_public_ip', {});
  await runTest(g, 'get_platform_private_ip', {});
}

async function testSettings() {
  const g = 'settings';
  await runTest(g, 'get_settings', {});
  // Don't update settings in tests — too destructive
}

async function testUsers() {
  const g = 'users';
  await runTest(g, 'get_current_user', {});
  await runTest(g, 'list_users', {});
  await runTest(g, 'list_users', { include_deleted: true });
}

async function testPresets() {
  const g = 'presets';
  await runTest(g, 'list_presets', {});
  await runTest(g, 'get_preset', { slug: 'nixpacks' });
}

async function testApiKeysReadOnly() {
  const g = 'api-keys-read';
  await runTest(g, 'list_api_keys', {});
  await runTest(g, 'get_api_key_permissions', {});
}

async function testAuditReadOnly() {
  const g = 'audit-read';
  await runTest(g, 'list_audit_logs', { limit: 5 });
}

async function testProxyLogsReadOnly() {
  const g = 'proxy-logs-read';
  await runTest(g, 'list_proxy_logs', { page: 1, page_size: 5 });
  await runTest(g, 'get_proxy_log_today_stats', {});
}

async function testDomainsReadOnly() {
  const g = 'domains-read';
  await runTest(g, 'list_domains', {});
  await runTest(g, 'list_domain_orders', {});
}

async function testServicesReadOnly() {
  const g = 'services-read';
  await runTest(g, 'list_services', {});
  await runTest(g, 'get_service_types', {});
}

async function testDnsProvidersReadOnly() {
  const g = 'dns-providers-read';
  await runTest(g, 'list_dns_providers', {});
  await runTest(g, 'lookup_dns_a_records', { domain: 'example.com' });
}

async function testNotificationsReadOnly() {
  const g = 'notifications-read';
  await runTest(g, 'list_notification_providers', {});
  await runTest(g, 'get_notification_preferences', {});
}

async function testEmailReadOnly() {
  const g = 'email-read';
  await runTest(g, 'list_email_domains', {});
  await runTest(g, 'list_email_providers', {});
}

async function testLoadBalancerReadOnly() {
  const g = 'lb-read';
  await runTest(g, 'list_lb_routes', {});
}

async function testIpAccessReadOnly() {
  const g = 'ip-access-read';
  await runTest(g, 'list_ip_access_rules', {});
  await runTest(g, 'check_ip_blocked', { ip: '8.8.8.8' });
}

async function testBackupsReadOnly() {
  const g = 'backups-read';
  await runTest(g, 'list_backup_schedules', {});
  await runTest(g, 'list_s3_sources', {});
}

async function testWebhookEventTypes() {
  const g = 'webhooks-read';
  await runTest(g, 'list_webhook_event_types', {});
}

// ─── Project CRUD Lifecycle ──────────────────────────────────────

async function testProjectLifecycle() {
  const g = 'projects';
  const testProjectName = `mcp-test-${Date.now()}`;

  // List
  await runTest(g, 'list_projects', {});
  await runTest(g, 'get_project_statistics', {});

  // Create
  await runTest(g, 'create_project', { name: testProjectName, source_type: 'docker_image' }, {
    extract: (r) => {
      ctx.projectId = extractId(r);
      ctx.projectSlug = extractSlug(r);
      if (ctx.projectId) registerCleanup('delete_project', { id: ctx.projectId });
    },
  });

  // Read by ID
  await runTest(g, 'get_project', { id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
  });

  // Read by slug
  await runTest(g, 'get_project_by_slug', { slug: ctx.projectSlug }, {
    skipIf: () => !ctx.projectSlug,
  });

  // Update
  await runTest(g, 'update_project', { id: ctx.projectId, name: `${testProjectName}-updated` }, {
    skipIf: () => !ctx.projectId,
  });

  // Update settings
  await runTest(g, 'update_project_settings', {
    project_id: ctx.projectId,
    enable_preview_environments: false,
  }, {
    skipIf: () => !ctx.projectId,
  });
}

// ─── Environment Lifecycle ───────────────────────────────────────

async function testEnvironmentLifecycle() {
  const g = 'environments';

  // List environments (project was created in project lifecycle)
  await runTest(g, 'list_environments', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
    extract: (r) => {
      // The default "production" env should exist
      const text = r.content[0]?.text ?? '';
      const idMatch = text.match(/\| (\d+) \| production/);
      if (idMatch) {
        ctx.environmentId = Number(idMatch[1]);
        ctx.environmentName = 'production';
      } else {
        // Try to find any environment ID
        const anyId = text.match(/\| (\d+) \|/);
        if (anyId) ctx.environmentId = Number(anyId[1]);
      }
    },
  });

  // Create a staging environment
  const stagingResult = await runTest(g, 'create_environment', {
    project_id: ctx.projectId,
    name: 'mcp-test-staging',
    branch: 'develop',
  }, {
    skipIf: () => !ctx.projectId,
    extract: (r) => {
      const envId = extractId(r);
      if (envId && !ctx.environmentId) {
        ctx.environmentId = envId;
      }
    },
  });

  // Environment variables
  await runTest(g, 'list_environment_variables', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
  });

  // Set an env var
  await runTest(g, 'set_environment_variable', {
    project_id: ctx.projectId,
    key: 'MCP_TEST_VAR',
    value: 'hello-from-mcp-tests',
  }, {
    skipIf: () => !ctx.projectId,
    extract: (r) => {
      ctx.envVarId = extractId(r);
    },
  });

  // List env vars again (should include MCP_TEST_VAR)
  await runTest(g, 'list_environment_variables', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
  });

  // Update env var
  await runTest(g, 'update_environment_variable', {
    project_id: ctx.projectId,
    variable_id: ctx.envVarId,
    value: 'updated-value',
  }, {
    skipIf: () => !ctx.projectId || !ctx.envVarId,
  });

  // Delete env var
  await runTest(g, 'delete_environment_variable', {
    project_id: ctx.projectId,
    variable_id: ctx.envVarId,
  }, {
    skipIf: () => !ctx.projectId || !ctx.envVarId,
  });

  // Crons (will likely be empty but should not error)
  await runTest(g, 'list_environment_crons', {
    project_id: ctx.projectId,
    environment_id: ctx.environmentId,
  }, {
    skipIf: () => !ctx.projectId || !ctx.environmentId,
  });
}

// ─── Deployment Tools ────────────────────────────────────────────

async function testDeployments() {
  const g = 'deployments';

  // List deployments (may be empty for a fresh project)
  await runTest(g, 'list_deployments', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
    extract: (r) => {
      const text = r.content[0]?.text ?? '';
      const idMatch = text.match(/\| (\d+) \|/);
      if (idMatch) ctx.deploymentId = Number(idMatch[1]);
    },
  });

  // Get last deployment (may 404 if project has no deployments yet)
  await runTest(g, 'get_last_deployment', { id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
    expectError: true,
    extract: (r) => {
      if (!ctx.deploymentId) {
        ctx.deploymentId = extractId(r);
      }
    },
  });

  // If the test project has no deployments, find one from an existing project
  if (!ctx.deploymentId) {
    const result = await callTool('list_projects', {});
    const text = result.content[0]?.text ?? '';
    // Find a project ID from the table (skip the test project)
    const projectIds = [...text.matchAll(/\| (\d+) \|/g)]
      .map((m) => Number(m[1]))
      .filter((id) => id !== ctx.projectId);

    for (const pid of projectIds) {
      const lastDeploy = await callTool('get_last_deployment', { id: pid });
      const depText = lastDeploy.content[0]?.text ?? '';
      if (!lastDeploy.isError && !depText.startsWith('Error:')) {
        const depId = extractId(lastDeploy);
        if (depId) {
          ctx.deploymentId = depId;
          // Use this project for deployment-specific tests
          ctx._deploymentProjectId = pid;
          break;
        }
      }
    }
  }

  const deployProjectId = ctx._deploymentProjectId ?? ctx.projectId;

  // Get specific deployment
  await runTest(g, 'get_deployment', {
    project_id: deployProjectId,
    deployment_id: ctx.deploymentId,
  }, {
    skipIf: () => !deployProjectId || !ctx.deploymentId,
  });

  // Get deployment logs
  await runTest(g, 'get_deployment_logs', {
    project_id: deployProjectId,
    deployment_id: ctx.deploymentId,
  }, {
    skipIf: () => !deployProjectId || !ctx.deploymentId,
  });

  // Deployment tokens (disabled: API endpoints do not exist)
  // await runTest(g, 'list_deployment_tokens', ...);
  // await runTest(g, 'create_deployment_token', ...);
  // await runTest(g, 'get_deployment_token', ...);
  // await runTest(g, 'delete_deployment_token', ...);
}

// ─── Containers ──────────────────────────────────────────────────

async function testContainers() {
  const g = 'containers';

  await runTest(g, 'list_containers', {
    project_id: ctx.projectId,
    environment_id: ctx.environmentId,
  }, {
    skipIf: () => !ctx.projectId || !ctx.environmentId,
  });

  // Container logs (environment-level, no container_id — uses primary container)
  await runTest(g, 'get_container_logs', {
    project_id: ctx.projectId,
    environment_id: ctx.environmentId,
    tail: '50',
  }, {
    skipIf: () => !ctx.projectId || !ctx.environmentId,
    expectError: true, // May fail if no containers are running
  });

  // Also test against a project with actual containers if available
  if (ctx._deploymentProjectId) {
    await runTest(g, 'get_container_logs', {
      project_id: ctx._deploymentProjectId,
      environment_id: 1,
      tail: '50',
    }, {
      expectError: true, // WebSocket may reject if no running container
    });
  }
}

// ─── Monitors Lifecycle ──────────────────────────────────────────

async function testMonitors() {
  const g = 'monitors';

  await runTest(g, 'list_monitors', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
  });

  await runTest(g, 'create_monitor', {
    project_id: ctx.projectId,
    name: 'mcp-test-monitor',
    monitor_type: 'http',
    check_interval_seconds: 60,
    environment_id: ctx.environmentId,
  }, {
    skipIf: () => !ctx.projectId || !ctx.environmentId,
    extract: (r) => {
      ctx.monitorId = extractId(r);
      if (ctx.monitorId) registerCleanup('delete_monitor', { monitor_id: ctx.monitorId });
    },
  });

  await runTest(g, 'get_monitor', { monitor_id: ctx.monitorId }, {
    skipIf: () => !ctx.monitorId,
  });

  await runTest(g, 'get_monitor_status', { monitor_id: ctx.monitorId }, {
    skipIf: () => !ctx.monitorId,
  });

  await runTest(g, 'get_monitor_history', { monitor_id: ctx.monitorId, days: 1 }, {
    skipIf: () => !ctx.monitorId,
  });

  await runTest(g, 'delete_monitor', { monitor_id: ctx.monitorId }, {
    skipIf: () => !ctx.monitorId,
  });
}

// ─── Webhooks Lifecycle ──────────────────────────────────────────

async function testWebhooks() {
  const g = 'webhooks';

  await runTest(g, 'list_webhooks', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
  });

  await runTest(g, 'create_webhook', {
    project_id: ctx.projectId,
    url: 'https://httpbin.org/post',
    events: ['deployment.created'],
    enabled: false,
  }, {
    skipIf: () => !ctx.projectId,
    extract: (r) => {
      ctx.webhookId = extractId(r);
      if (ctx.webhookId) registerCleanup('delete_webhook', {
        project_id: ctx.projectId,
        webhook_id: ctx.webhookId,
      });
    },
  });

  await runTest(g, 'get_webhook', {
    project_id: ctx.projectId,
    webhook_id: ctx.webhookId,
  }, {
    skipIf: () => !ctx.projectId || !ctx.webhookId,
  });

  await runTest(g, 'update_webhook', {
    project_id: ctx.projectId,
    webhook_id: ctx.webhookId,
    url: 'https://httpbin.org/anything',
  }, {
    skipIf: () => !ctx.projectId || !ctx.webhookId,
  });

  await runTest(g, 'list_webhook_deliveries', {
    project_id: ctx.projectId,
    webhook_id: ctx.webhookId,
  }, {
    skipIf: () => !ctx.projectId || !ctx.webhookId,
  });

  await runTest(g, 'delete_webhook', {
    project_id: ctx.projectId,
    webhook_id: ctx.webhookId,
  }, {
    skipIf: () => !ctx.projectId || !ctx.webhookId,
  });
}

// ─── Incidents Lifecycle ─────────────────────────────────────────

async function testIncidents() {
  const g = 'incidents';

  await runTest(g, 'list_incidents', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
  });

  await runTest(g, 'create_incident', {
    project_id: ctx.projectId,
    title: 'MCP Test Incident',
    severity: 'minor',
    description: 'Created by automated MCP test suite',
  }, {
    skipIf: () => !ctx.projectId,
    extract: (r) => {
      ctx.incidentId = extractId(r);
    },
  });

  await runTest(g, 'get_incident', { incident_id: ctx.incidentId }, {
    skipIf: () => !ctx.incidentId,
  });

  await runTest(g, 'update_incident_status', {
    incident_id: ctx.incidentId,
    status: 'resolved',
    message: 'Auto-resolved by MCP test suite',
  }, {
    skipIf: () => !ctx.incidentId,
  });

  await runTest(g, 'get_incident_updates', { incident_id: ctx.incidentId }, {
    skipIf: () => !ctx.incidentId,
  });

  await runTest(g, 'get_bucketed_incidents', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
    expectError: true, // Backend returns 500 on new projects without incident data (TimescaleDB issue)
  });
}

// ─── Funnels Lifecycle ───────────────────────────────────────────

async function testFunnels() {
  const g = 'funnels';

  await runTest(g, 'list_funnels', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
  });

  await runTest(g, 'create_funnel', {
    project_id: ctx.projectId,
    name: 'MCP Test Funnel',
    steps: [
      { event_name: 'page_view' },
      { event_name: 'button_click' },
    ],
  }, {
    skipIf: () => !ctx.projectId,
    extract: (r) => {
      ctx.funnelId = extractId(r) ?? extractFromJson<number>(r, 'funnel_id');
    },
  });

  await runTest(g, 'get_funnel_metrics', {
    project_id: ctx.projectId,
    funnel_id: ctx.funnelId,
  }, {
    skipIf: () => !ctx.projectId || !ctx.funnelId,
  });

  await runTest(g, 'update_funnel', {
    project_id: ctx.projectId,
    funnel_id: ctx.funnelId,
    name: 'MCP Test Funnel Updated',
  }, {
    skipIf: () => !ctx.projectId || !ctx.funnelId,
  });

  await runTest(g, 'preview_funnel_metrics', {
    project_id: ctx.projectId,
    name: 'Preview Funnel',
    steps: [{ event_name: 'page_view' }],
  }, {
    skipIf: () => !ctx.projectId,
  });

  await runTest(g, 'delete_funnel', {
    project_id: ctx.projectId,
    funnel_id: ctx.funnelId,
  }, {
    skipIf: () => !ctx.projectId || !ctx.funnelId,
  });
}

// ─── DSN Lifecycle ───────────────────────────────────────────────

async function testDsn() {
  const g = 'dsn';

  await runTest(g, 'list_dsns', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
  });

  await runTest(g, 'create_dsn', {
    project_id: ctx.projectId,
    name: 'mcp-test-dsn',
  }, {
    skipIf: () => !ctx.projectId,
    extract: (r) => {
      ctx.dsnId = extractId(r);
    },
  });

  await runTest(g, 'get_or_create_dsn', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
  });

  await runTest(g, 'regenerate_dsn', {
    project_id: ctx.projectId,
    dsn_id: ctx.dsnId,
  }, {
    skipIf: () => !ctx.projectId || !ctx.dsnId,
  });

  await runTest(g, 'revoke_dsn', {
    project_id: ctx.projectId,
    dsn_id: ctx.dsnId,
  }, {
    skipIf: () => !ctx.projectId || !ctx.dsnId,
  });
}

// ─── Scans ───────────────────────────────────────────────────────

async function testScans() {
  const g = 'scans';

  await runTest(g, 'list_project_scans', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
  });

  await runTest(g, 'get_latest_scans_per_environment', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
  });
}

// ─── Custom Domains ──────────────────────────────────────────────

async function testCustomDomains() {
  const g = 'custom-domains';

  await runTest(g, 'list_custom_domains', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
  });
}

// ─── Errors ──────────────────────────────────────────────────────

async function testErrors() {
  const g = 'errors';

  await runTest(g, 'list_error_groups', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
    extract: (r) => {
      // Try to grab a group id for further tests
      const text = r.content[0]?.text ?? '';
      const m = text.match(/\| (\d+) \|/);
      if (m) ctx.errorGroupId = Number(m[1]);
    },
  });

  await runTest(g, 'get_error_stats', { project_id: ctx.projectId }, {
    skipIf: () => !ctx.projectId,
  });

  await runTest(g, 'get_error_dashboard', {
    project_id: ctx.projectId,
    start_time: new Date(Date.now() - 86400000).toISOString(),
    end_time: new Date().toISOString(),
  }, {
    skipIf: () => !ctx.projectId,
  });
}

// ─── API Key CRUD ────────────────────────────────────────────────

async function testApiKeyCrud() {
  const g = 'api-keys-crud';

  await runTest(g, 'create_api_key', {
    name: 'mcp-test-key',
    role_type: 'reader',
  }, {
    extract: (r) => {
      ctx.apiKeyId = extractId(r);
      if (ctx.apiKeyId) registerCleanup('delete_api_key', { id: ctx.apiKeyId });
    },
  });

  await runTest(g, 'get_api_key', { id: ctx.apiKeyId }, {
    skipIf: () => !ctx.apiKeyId,
  });

  await runTest(g, 'deactivate_api_key', { id: ctx.apiKeyId }, {
    skipIf: () => !ctx.apiKeyId,
  });

  await runTest(g, 'activate_api_key', { id: ctx.apiKeyId }, {
    skipIf: () => !ctx.apiKeyId,
  });

  await runTest(g, 'delete_api_key', { id: ctx.apiKeyId }, {
    skipIf: () => !ctx.apiKeyId,
  });
}

// ─── IP Access CRUD ──────────────────────────────────────────────

async function testIpAccessCrud() {
  const g = 'ip-access-crud';

  await runTest(g, 'create_ip_access_rule', {
    ip_address: '192.168.99.99',
    action: 'block',
    reason: 'MCP test rule',
  }, {
    extract: (r) => {
      ctx.ipRuleId = extractId(r);
      if (ctx.ipRuleId) registerCleanup('delete_ip_access_rule', { id: ctx.ipRuleId });
    },
  });

  await runTest(g, 'get_ip_access_rule', { id: ctx.ipRuleId }, {
    skipIf: () => !ctx.ipRuleId,
  });

  await runTest(g, 'update_ip_access_rule', {
    id: ctx.ipRuleId,
    reason: 'Updated by MCP tests',
  }, {
    skipIf: () => !ctx.ipRuleId,
  });

  await runTest(g, 'delete_ip_access_rule', { id: ctx.ipRuleId }, {
    skipIf: () => !ctx.ipRuleId,
  });
}

// ─── Notification Preferences ────────────────────────────────────

async function testNotificationPrefs() {
  const g = 'notification-prefs';

  await runTest(g, 'get_notification_preferences', {});

  await runTest(g, 'update_notification_preference', {
    key: 'weekly_digest_enabled',
    value: true,
  });
}

// ─── Tool Registry Validation ────────────────────────────────────

async function testToolRegistry() {
  const g = 'registry';

  // Verify listTools returns all tools
  const toolList = listTools();
  const count = toolList.tools.length;

  results.push({
    tool: 'registry_list_tools',
    group: g,
    status: count > 0 ? 'pass' : 'fail',
    durationMs: 0,
    response: `${count} tools registered`,
    error: count > 0 ? undefined : `Expected at least 1 tool, got ${count}`,
  });

  // Verify each tool has required fields
  let invalidCount = 0;
  for (const tool of toolList.tools) {
    if (!tool.name || !tool.description || !tool.inputSchema) {
      invalidCount++;
    }
  }

  results.push({
    tool: 'registry_tool_schemas',
    group: g,
    status: invalidCount === 0 ? 'pass' : 'fail',
    durationMs: 0,
    error: invalidCount > 0 ? `${invalidCount} tools with invalid schemas` : undefined,
  });

  // Verify unknown tool returns error
  const unknownResult = await callTool('nonexistent_tool', {});
  results.push({
    tool: 'registry_unknown_tool',
    group: g,
    status: unknownResult.isError ? 'pass' : 'fail',
    durationMs: 0,
    error: unknownResult.isError ? undefined : 'Expected error for unknown tool',
  });

  // Verify missing required params returns error
  const missingParamResult = await callTool('get_project', {});
  results.push({
    tool: 'registry_missing_params',
    group: g,
    status: missingParamResult.isError || missingParamResult.content[0]?.text.includes('Error') ? 'pass' : 'fail',
    durationMs: 0,
    error: 'Expected error for missing required param',
  });
}

// ─── Cleanup ─────────────────────────────────────────────────────

async function cleanup() {
  console.log('\n--- Cleanup ---');

  // Run cleanup in reverse order
  for (const { tool, args } of CLEANUP_IDS.reverse()) {
    try {
      console.log(`  Cleaning up: ${tool}(${JSON.stringify(args)})`);
      await callTool(tool, args);
    } catch (err) {
      console.error(`  [cleanup error] ${tool}: ${err}`);
    }
  }

  // Always try to delete the test project last
  if (ctx.projectId) {
    try {
      console.log(`  Final cleanup: delete_project(${ctx.projectId})`);
      await callTool('delete_project', { id: ctx.projectId });
    } catch (err) {
      console.error(`  [cleanup error] delete_project: ${err}`);
    }
  }
}

// ─── Report ──────────────────────────────────────────────────────

function printReport() {
  const pass = results.filter((r) => r.status === 'pass');
  const fail = results.filter((r) => r.status === 'fail');
  const skip = results.filter((r) => r.status === 'skip');

  console.log('\n╔══════════════════════════════════════════════════════════════╗');
  console.log('║                   MCP Integration Test Report               ║');
  console.log('╚══════════════════════════════════════════════════════════════╝\n');

  // Group by test group
  const groups = new Map<string, TestResult[]>();
  for (const r of results) {
    const list = groups.get(r.group) || [];
    list.push(r);
    groups.set(r.group, list);
  }

  for (const [group, groupResults] of groups) {
    const gPass = groupResults.filter((r) => r.status === 'pass').length;
    const gFail = groupResults.filter((r) => r.status === 'fail').length;
    const gSkip = groupResults.filter((r) => r.status === 'skip').length;
    const icon = gFail === 0 ? (gSkip === groupResults.length ? '○' : '●') : '✗';

    console.log(`${icon} ${group} (${gPass}/${groupResults.length} passed)`);

    for (const r of groupResults) {
      const statusIcon = r.status === 'pass' ? '  ✓' : r.status === 'fail' ? '  ✗' : '  ○';
      const timing = r.durationMs > 0 ? ` (${r.durationMs.toFixed(0)}ms)` : '';
      const detail = r.status === 'fail' ? `\n    → ${r.error}` : '';
      console.log(`${statusIcon} ${r.tool}${timing}${detail}`);
    }
    console.log('');
  }

  console.log('─────────────────────────────────────────────');
  console.log(`Total: ${results.length} tests`);
  console.log(`  ✓ Pass: ${pass.length}`);
  console.log(`  ✗ Fail: ${fail.length}`);
  console.log(`  ○ Skip: ${skip.length}`);
  console.log(`  Duration: ${(results.reduce((a, r) => a + r.durationMs, 0) / 1000).toFixed(1)}s`);
  console.log('');

  if (fail.length > 0) {
    console.log('FAILED TESTS:');
    for (const r of fail) {
      console.log(`  ✗ [${r.group}] ${r.tool}: ${r.error}`);
    }
    console.log('');
  }

  return fail.length;
}

// ─── Main ────────────────────────────────────────────────────────

async function main() {
  // Parse CLI args
  const args = process.argv.slice(2);
  const groupFilter = args.includes('--group') ? args[args.indexOf('--group') + 1] : undefined;
  const onlyFilter = args.includes('--only') ? args[args.indexOf('--only') + 1] : undefined;

  // Validate env
  if (!process.env.TEMPS_API_URL || !process.env.TEMPS_API_KEY) {
    console.error('Error: TEMPS_API_URL and TEMPS_API_KEY environment variables are required.');
    console.error('Usage: TEMPS_API_URL=http://localhost:8081 TEMPS_API_KEY=<key> bun run test');
    process.exit(1);
  }

  // Use existing project if TEST_PROJECT_ID is set
  if (process.env.TEST_PROJECT_ID) {
    ctx.projectId = Number(process.env.TEST_PROJECT_ID);
    console.log(`Using existing project: ${ctx.projectId}`);
  }

  console.log(`Temps MCP Integration Tests`);
  console.log(`API URL: ${process.env.TEMPS_API_URL}`);
  console.log(`Tools registered: ${toolCount}`);
  if (groupFilter) console.log(`Filter: group=${groupFilter}`);
  if (onlyFilter) console.log(`Filter: only=${onlyFilter}`);
  console.log('');

  // If --only, run just that tool
  if (onlyFilter) {
    await runTest('manual', onlyFilter, JSON.parse(args[args.indexOf('--only') + 2] || '{}'));
    const failCount = printReport();
    process.exit(failCount > 0 ? 1 : 0);
  }

  const allGroups: Array<[string, () => Promise<void>]> = [
    ['registry', testToolRegistry],
    ['platform', testPlatform],
    ['settings', testSettings],
    ['users', testUsers],
    ['presets', testPresets],
    ['api-keys-read', testApiKeysReadOnly],
    ['audit-read', testAuditReadOnly],
    ['proxy-logs-read', testProxyLogsReadOnly],
    ['domains-read', testDomainsReadOnly],
    ['services-read', testServicesReadOnly],
    ['dns-providers-read', testDnsProvidersReadOnly],
    ['notifications-read', testNotificationsReadOnly],
    ['email-read', testEmailReadOnly],
    ['lb-read', testLoadBalancerReadOnly],
    ['ip-access-read', testIpAccessReadOnly],
    ['backups-read', testBackupsReadOnly],
    ['webhooks-read', testWebhookEventTypes],
    ['projects', testProjectLifecycle],
    ['environments', testEnvironmentLifecycle],
    ['deployments', testDeployments],
    ['containers', testContainers],
    ['monitors', testMonitors],
    ['webhooks', testWebhooks],
    ['incidents', testIncidents],
    ['funnels', testFunnels],
    ['dsn', testDsn],
    ['scans', testScans],
    ['custom-domains', testCustomDomains],
    ['errors', testErrors],
    ['api-keys-crud', testApiKeyCrud],
    ['ip-access-crud', testIpAccessCrud],
    ['notification-prefs', testNotificationPrefs],
  ];

  for (const [name, fn] of allGroups) {
    if (groupFilter && name !== groupFilter) continue;

    console.log(`▶ Running: ${name}`);
    try {
      await fn();
    } catch (err) {
      console.error(`  [group error] ${name}: ${err}`);
      results.push({
        tool: `${name}_group`,
        group: name,
        status: 'fail',
        durationMs: 0,
        error: `Group failed: ${err instanceof Error ? err.message : String(err)}`,
      });
    }
  }

  // Cleanup resources
  await cleanup();

  // Print report and exit
  const failCount = printReport();
  process.exit(failCount > 0 ? 1 : 0);
}

main().catch((err) => {
  console.error('Fatal error:', err);
  process.exit(1);
});
