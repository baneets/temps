/**
 * Tool Registry
 *
 * Aggregates all tool modules and provides list/dispatch functions
 * for the MCP server.
 */

import type { ToolDefinition, ToolResult } from '../types/index.js';

import { tools as projectsTools } from './projects.js';
import { tools as deploymentsTools } from './deployments.js';
import { tools as environmentsTools } from './environments.js';
import { tools as domainsTools } from './domains.js';
import { tools as servicesTools } from './services.js';
import { tools as backupsTools } from './backups.js';
import { tools as monitorsTools } from './monitors.js';
import { tools as containersTools } from './containers.js';
import { tools as usersTools } from './users.js';
import { tools as settingsTools } from './settings.js';
import { tools as apiKeysTools } from './api-keys.js';
import { tools as webhooksTools } from './webhooks.js';
import { tools as auditTools } from './audit.js';
import { tools as dnsProvidersTools } from './dns-providers.js';
import { tools as notificationsTools } from './notifications.js';
import { tools as scansTools } from './scans.js';
import { tools as customDomainsTools } from './custom-domains.js';
import { tools as errorsTools } from './errors.js';
import { tools as proxyLogsTools } from './proxy-logs.js';
import { tools as dsnTools } from './dsn.js';
import { tools as ipAccessTools } from './ip-access.js';
import { tools as incidentsTools } from './incidents.js';
import { tools as funnelsTools } from './funnels.js';
import { tools as presetsTools } from './presets.js';
import { tools as platformTools } from './platform.js';
import { tools as emailDomainsTools } from './email-domains.js';
import { tools as emailProvidersTools } from './email-providers.js';
import { tools as loadBalancerTools } from './load-balancer.js';
import { tools as notificationPrefsTools } from './notification-prefs.js';
// Disabled: deployment-tokens endpoints do not exist in the API
// import { tools as tokensTools } from './tokens.js';

/** All registered tools */
const allTools: ToolDefinition[] = [
  ...projectsTools,
  ...deploymentsTools,
  ...environmentsTools,
  ...domainsTools,
  ...servicesTools,
  ...backupsTools,
  ...monitorsTools,
  ...containersTools,
  ...usersTools,
  ...settingsTools,
  ...apiKeysTools,
  ...webhooksTools,
  ...auditTools,
  ...dnsProvidersTools,
  ...notificationsTools,
  ...scansTools,
  ...customDomainsTools,
  ...errorsTools,
  ...proxyLogsTools,
  ...dsnTools,
  ...ipAccessTools,
  ...incidentsTools,
  ...funnelsTools,
  ...presetsTools,
  ...platformTools,
  ...emailDomainsTools,
  ...emailProvidersTools,
  ...loadBalancerTools,
  ...notificationPrefsTools,
  // ...tokensTools, // Disabled: deployment-tokens endpoints do not exist in the API
];

/** Build lookup map for O(1) dispatch */
const toolMap = new Map<string, ToolDefinition>();
for (const tool of allTools) {
  if (toolMap.has(tool.name)) {
    throw new Error(`Duplicate tool name: ${tool.name}`);
  }
  toolMap.set(tool.name, tool);
}

/**
 * List all available tools (for ListToolsRequest)
 */
export function listTools() {
  return {
    tools: allTools.map((t) => ({
      name: t.name,
      description: t.description,
      inputSchema: t.inputSchema,
    })),
  };
}

/**
 * Call a tool by name (for CallToolRequest)
 */
export async function callTool(
  name: string,
  args: Record<string, unknown>
): Promise<ToolResult> {
  const tool = toolMap.get(name);
  if (!tool) {
    return {
      content: [{ type: 'text', text: `Unknown tool: ${name}` }],
      isError: true,
    };
  }
  return tool.handler(args);
}

/** Total number of registered tools */
export const toolCount = allTools.length;
