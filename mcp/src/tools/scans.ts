import { getClient } from '../api/index.js';
import {
  ok,
  json,
  table,
  formatDate,
  handleToolCall,
  requireParam,
  optionalParam,
} from './_helpers.js';
import type { ToolDefinition } from '../types/index.js';

interface Scan {
  id: number;
  project_id: number;
  environment_id: number;
  status: string;
  total_vulnerabilities?: number;
  critical_count?: number;
  high_count?: number;
  medium_count?: number;
  low_count?: number;
  started_at?: string;
  completed_at?: string;
  created_at?: string;
}

interface ScanTriggerResult {
  scan_id: number;
  status: string;
}

interface Vulnerability {
  id: number;
  scan_id: number;
  severity: string;
  package_name?: string;
  installed_version?: string;
  fixed_version?: string;
  title?: string;
  description?: string;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_project_scans',
    description: 'List vulnerability scans for a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        page: {
          type: 'number',
          description: 'Page number (default: 1)',
        },
        page_size: {
          type: 'number',
          description: 'Items per page (default: 20, max: 100)',
        },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const page = optionalParam<number>(args, 'page');
        const pageSize = optionalParam<number>(args, 'page_size');
        const client = getClient();

        const query: Record<string, unknown> = {};
        if (page !== undefined) query.page = page;
        if (pageSize !== undefined) query.page_size = pageSize;

        const result = await client.get<{ data: Scan[]; total: number } | Scan[]>(
          `/projects/${projectId}/vulnerability-scans`,
          query
        );

        const scans = Array.isArray(result) ? result : (result.data ?? []);
        if (!scans || scans.length === 0) {
          return ok('No scans found for this project.');
        }

        const rows = scans.map((s) => [
          String(s.id),
          s.status,
          String(s.total_vulnerabilities ?? 0),
          String(s.critical_count ?? 0),
          String(s.high_count ?? 0),
          formatDate(s.created_at),
        ]);

        return ok(
          table(
            ['ID', 'Status', 'Total', 'Critical', 'High', 'Created'],
            rows
          )
        );
      }),
  },
  {
    name: 'trigger_scan',
    description: 'Trigger a new vulnerability scan for a project environment',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        environment_id: {
          type: 'number',
          description: 'Environment ID to scan',
        },
      },
      required: ['project_id', 'environment_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const environmentId = requireParam<number>(args, 'environment_id');
        const client = getClient();

        const result = await client.post<ScanTriggerResult>(
          `/projects/${projectId}/vulnerability-scans`,
          { environment_id: environmentId }
        );

        return json('Scan Triggered', result);
      }),
  },
  {
    name: 'get_latest_scan',
    description:
      'Get the latest vulnerability scan for a project, optionally filtered by environment',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
        environment_id: {
          type: 'number',
          description: 'Optional environment ID to filter by',
        },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const environmentId = optionalParam<number>(args, 'environment_id');
        const client = getClient();

        const query: Record<string, unknown> = {};
        if (environmentId !== undefined) query.environment_id = environmentId;

        const scan = await client.get<Scan>(
          `/projects/${projectId}/vulnerability-scans/latest`,
          query
        );

        return json('Latest Scan', scan);
      }),
  },
  {
    name: 'get_latest_scans_per_environment',
    description:
      'Get the latest scan for each environment in a project',
    inputSchema: {
      type: 'object',
      properties: {
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
      },
      required: ['project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const projectId = requireParam<number>(args, 'project_id');
        const client = getClient();

        const scans = await client.get<Scan[]>(
          `/projects/${projectId}/vulnerability-scans/environments`
        );

        if (!scans || scans.length === 0) {
          return ok('No scans found for any environment.');
        }

        const rows = scans.map((s) => [
          String(s.id),
          String(s.environment_id),
          s.status,
          String(s.total_vulnerabilities ?? 0),
          formatDate(s.created_at),
        ]);

        return ok(
          table(
            ['Scan ID', 'Environment ID', 'Status', 'Vulnerabilities', 'Created'],
            rows
          )
        );
      }),
  },
  {
    name: 'get_scan',
    description: 'Get details of a specific vulnerability scan',
    inputSchema: {
      type: 'object',
      properties: {
        scan_id: {
          type: 'number',
          description: 'Scan ID',
        },
      },
      required: ['scan_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const scanId = requireParam<number>(args, 'scan_id');
        const client = getClient();
        const scan = await client.get<Scan>(`/vulnerability-scans/${scanId}`);

        return json('Scan Details', scan);
      }),
  },
  {
    name: 'get_scan_vulnerabilities',
    description: 'Get vulnerabilities found in a specific scan',
    inputSchema: {
      type: 'object',
      properties: {
        scan_id: {
          type: 'number',
          description: 'Scan ID',
        },
        severity: {
          type: 'string',
          description:
            'Filter by severity level (e.g. critical, high, medium, low)',
        },
      },
      required: ['scan_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const scanId = requireParam<number>(args, 'scan_id');
        const severity = optionalParam<string>(args, 'severity');
        const client = getClient();

        const query: Record<string, unknown> = {};
        if (severity !== undefined) query.severity = severity;

        const vulnerabilities = await client.get<Vulnerability[]>(
          `/vulnerability-scans/${scanId}/vulnerabilities`,
          query
        );

        if (!vulnerabilities || vulnerabilities.length === 0) {
          return ok('No vulnerabilities found for this scan.');
        }

        const rows = vulnerabilities.map((v) => [
          String(v.id),
          v.severity,
          v.package_name ?? 'N/A',
          v.installed_version ?? 'N/A',
          v.fixed_version ?? 'N/A',
          v.title ?? 'N/A',
        ]);

        return ok(
          table(
            ['ID', 'Severity', 'Package', 'Installed', 'Fixed', 'Title'],
            rows
          )
        );
      }),
  },
  {
    name: 'delete_scan',
    description: 'Delete a vulnerability scan',
    inputSchema: {
      type: 'object',
      properties: {
        scan_id: {
          type: 'number',
          description: 'Scan ID',
        },
      },
      required: ['scan_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const scanId = requireParam<number>(args, 'scan_id');
        const client = getClient();
        await client.delete(`/vulnerability-scans/${scanId}`);

        return ok(`Scan ${scanId} deleted successfully.`);
      }),
  },
];
