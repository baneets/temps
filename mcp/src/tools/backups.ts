/**
 * Backup management tools for Temps MCP Server
 *
 * Manages backup schedules, S3 sources, and backup operations
 * for both platform-managed and external services.
 */

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

interface BackupSchedule {
  id: number;
  name: string;
  backup_type: string;
  schedule_expression: string;
  retention_period: string;
  description?: string;
  s3_source_id: number;
  enabled: boolean;
  tags?: string[];
  created_at?: string;
  updated_at?: string;
  last_run_at?: string;
}

interface S3Source {
  id: number;
  name: string;
  bucket_name: string;
  region: string;
  endpoint: string;
  bucket_path?: string;
  created_at?: string;
  updated_at?: string;
}

interface Backup {
  id: number;
  status: string;
  backup_type?: string;
  size_bytes?: number;
  schedule_id?: number;
  s3_source_id?: number;
  service_id?: number;
  started_at?: string;
  completed_at?: string;
  created_at?: string;
  error_message?: string;
}

export const tools: ToolDefinition[] = [
  // ── Backup Schedules ──────────────────────────────────────────────

  {
    name: 'list_backup_schedules',
    description: 'List all configured backup schedules',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const schedules = await client.get<BackupSchedule[]>('/backups/schedules');

        if (!schedules || schedules.length === 0) {
          return ok('No backup schedules found.');
        }

        const rows = schedules.map((s) => [
          String(s.id),
          s.name,
          s.backup_type,
          s.schedule_expression,
          s.enabled ? 'Enabled' : 'Disabled',
          s.retention_period,
        ]);

        return ok(
          `## Backup Schedules\n\n${table(['ID', 'Name', 'Type', 'Schedule', 'Status', 'Retention'], rows)}`
        );
      }),
  },

  {
    name: 'create_backup_schedule',
    description: 'Create a new backup schedule for automated backups',
    inputSchema: {
      type: 'object',
      properties: {
        name: {
          type: 'string',
          description: 'Name for the backup schedule',
        },
        backup_type: {
          type: 'string',
          description: 'Type of backup (e.g. full, incremental)',
        },
        schedule_expression: {
          type: 'string',
          description: 'Cron expression for the schedule (e.g. "0 2 * * *" for daily at 2 AM)',
        },
        retention_period: {
          type: 'string',
          description: 'How long to retain backups (e.g. "30d", "1y")',
        },
        s3_source_id: {
          type: 'number',
          description: 'ID of the S3 source to store backups in',
        },
        description: {
          type: 'string',
          description: 'Optional description for the schedule',
        },
        enabled: {
          type: 'boolean',
          description: 'Whether the schedule is enabled (default: true)',
        },
        tags: {
          type: 'array',
          items: { type: 'string' },
          description: 'Optional tags for categorizing the schedule',
        },
      },
      required: ['name', 'backup_type', 'schedule_expression', 'retention_period', 's3_source_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const body: Record<string, unknown> = {
          name: requireParam<string>(args, 'name'),
          backup_type: requireParam<string>(args, 'backup_type'),
          schedule_expression: requireParam<string>(args, 'schedule_expression'),
          retention_period: requireParam<string>(args, 'retention_period'),
          s3_source_id: requireParam<number>(args, 's3_source_id'),
        };

        const description = optionalParam<string>(args, 'description');
        const enabled = optionalParam<boolean>(args, 'enabled');
        const tags = optionalParam<string[]>(args, 'tags');

        if (description !== undefined) body.description = description;
        if (enabled !== undefined) body.enabled = enabled;
        if (tags !== undefined) body.tags = tags;

        const schedule = await client.post<BackupSchedule>('/backups/schedules', body);
        return json('Backup Schedule Created', schedule);
      }),
  },

  {
    name: 'get_backup_schedule',
    description: 'Get detailed information about a specific backup schedule',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Backup schedule ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const schedule = await client.get<BackupSchedule>(`/backups/schedules/${id}`);
        return json('Backup Schedule Details', schedule);
      }),
  },

  {
    name: 'enable_backup_schedule',
    description: 'Enable a disabled backup schedule',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Backup schedule ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        await client.patch(`/backups/schedules/${id}/enable`);
        return ok(`Backup schedule ${id} enabled successfully.`);
      }),
  },

  {
    name: 'disable_backup_schedule',
    description: 'Disable an active backup schedule',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Backup schedule ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        await client.patch(`/backups/schedules/${id}/disable`);
        return ok(`Backup schedule ${id} disabled successfully.`);
      }),
  },

  {
    name: 'delete_backup_schedule',
    description: 'Delete a backup schedule. Existing backups are not removed.',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Backup schedule ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        await client.delete(`/backups/schedules/${id}`);
        return ok(`Backup schedule ${id} deleted successfully.`);
      }),
  },

  // ── S3 Sources ────────────────────────────────────────────────────

  {
    name: 'list_s3_sources',
    description: 'List all configured S3-compatible storage sources for backups',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const sources = await client.get<S3Source[]>('/backups/s3-sources');

        if (!sources || sources.length === 0) {
          return ok('No S3 sources found.');
        }

        const rows = sources.map((s) => [
          String(s.id),
          s.name,
          s.bucket_name,
          s.region,
          s.endpoint,
        ]);

        return ok(
          `## S3 Sources\n\n${table(['ID', 'Name', 'Bucket', 'Region', 'Endpoint'], rows)}`
        );
      }),
  },

  {
    name: 'create_s3_source',
    description:
      'Configure a new S3-compatible storage source for storing backups',
    inputSchema: {
      type: 'object',
      properties: {
        name: {
          type: 'string',
          description: 'Display name for the S3 source',
        },
        bucket_name: {
          type: 'string',
          description: 'S3 bucket name',
        },
        region: {
          type: 'string',
          description: 'AWS region or S3-compatible region (e.g. us-east-1)',
        },
        endpoint: {
          type: 'string',
          description:
            'S3 endpoint URL (e.g. https://s3.amazonaws.com or a MinIO endpoint)',
        },
        access_key_id: {
          type: 'string',
          description: 'Access key ID for authentication',
        },
        secret_key: {
          type: 'string',
          description: 'Secret access key for authentication',
        },
        bucket_path: {
          type: 'string',
          description: 'Optional path prefix within the bucket',
        },
      },
      required: ['name', 'bucket_name', 'region', 'endpoint', 'access_key_id', 'secret_key'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const body: Record<string, unknown> = {
          name: requireParam<string>(args, 'name'),
          bucket_name: requireParam<string>(args, 'bucket_name'),
          region: requireParam<string>(args, 'region'),
          endpoint: requireParam<string>(args, 'endpoint'),
          access_key_id: requireParam<string>(args, 'access_key_id'),
          secret_key: requireParam<string>(args, 'secret_key'),
        };

        const bucket_path = optionalParam<string>(args, 'bucket_path');
        if (bucket_path !== undefined) body.bucket_path = bucket_path;

        const source = await client.post<S3Source>('/backups/s3-sources', body);
        return json('S3 Source Created', source);
      }),
  },

  {
    name: 'get_s3_source',
    description: 'Get detailed information about a specific S3 source',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'S3 source ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const source = await client.get<S3Source>(`/backups/s3-sources/${id}`);
        return json('S3 Source Details', source);
      }),
  },

  {
    name: 'update_s3_source',
    description: 'Update an existing S3 source configuration',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'S3 source ID',
        },
        name: {
          type: 'string',
          description: 'Updated display name',
        },
        bucket_name: {
          type: 'string',
          description: 'Updated bucket name',
        },
        region: {
          type: 'string',
          description: 'Updated region',
        },
        endpoint: {
          type: 'string',
          description: 'Updated endpoint URL',
        },
        access_key_id: {
          type: 'string',
          description: 'Updated access key ID',
        },
        secret_key: {
          type: 'string',
          description: 'Updated secret access key',
        },
        bucket_path: {
          type: 'string',
          description: 'Updated path prefix within the bucket',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');

        const body: Record<string, unknown> = {};
        const fields = [
          'name',
          'bucket_name',
          'region',
          'endpoint',
          'access_key_id',
          'secret_key',
          'bucket_path',
        ];
        for (const field of fields) {
          const val = optionalParam<string>(args, field);
          if (val !== undefined) body[field] = val;
        }

        const source = await client.patch<S3Source>(`/backups/s3-sources/${id}`, body);
        return json('S3 Source Updated', source);
      }),
  },

  {
    name: 'delete_s3_source',
    description:
      'Delete an S3 source configuration. Existing backups stored in the bucket are not removed.',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'S3 source ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        await client.delete(`/backups/s3-sources/${id}`);
        return ok(`S3 source ${id} deleted successfully.`);
      }),
  },

  {
    name: 'list_source_backups',
    description: 'List all backups stored in a specific S3 source',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'S3 source ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const result = await client.get<{ backups: Backup[] }>(
          `/backups/s3-sources/${id}/backups`
        );

        const backups = result.backups;
        if (!backups || backups.length === 0) {
          return ok(`No backups found for S3 source ${id}.`);
        }

        const rows = backups.map((b) => [
          String(b.id),
          b.status,
          b.backup_type || 'N/A',
          b.size_bytes != null ? `${(b.size_bytes / 1024 / 1024).toFixed(2)} MB` : 'N/A',
          formatDate(b.created_at),
        ]);

        return ok(
          `## Backups for S3 Source ${id}\n\n${table(['ID', 'Status', 'Type', 'Size', 'Created'], rows)}`
        );
      }),
  },

  {
    name: 'run_backup_for_source',
    description: 'Trigger an immediate backup for an S3 source',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'S3 source ID',
        },
        backup_type: {
          type: 'string',
          description: 'Type of backup to run (e.g. full, incremental)',
        },
      },
      required: ['id', 'backup_type'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const backup_type = requireParam<string>(args, 'backup_type');
        const result = await client.post<Backup>(`/backups/s3-sources/${id}/run`, {
          backup_type,
        });
        return json('Backup Started', result);
      }),
  },

  // ── Backups per Schedule ──────────────────────────────────────────

  {
    name: 'list_backups_for_schedule',
    description: 'List all backups created by a specific backup schedule',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Backup schedule ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const backups = await client.get<Backup[]>(`/backups/schedules/${id}/backups`);

        if (!backups || backups.length === 0) {
          return ok(`No backups found for schedule ${id}.`);
        }

        const rows = backups.map((b) => [
          String(b.id),
          b.status,
          b.backup_type || 'N/A',
          b.size_bytes != null ? `${(b.size_bytes / 1024 / 1024).toFixed(2)} MB` : 'N/A',
          formatDate(b.created_at),
        ]);

        return ok(
          `## Backups for Schedule ${id}\n\n${table(['ID', 'Status', 'Type', 'Size', 'Created'], rows)}`
        );
      }),
  },

  // ── Individual Backup ─────────────────────────────────────────────

  {
    name: 'get_backup',
    description: 'Get detailed information about a specific backup',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Backup ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const backup = await client.get<Backup>(`/backups/${id}`);
        return json('Backup Details', backup);
      }),
  },

  // ── External Service Backup ───────────────────────────────────────

  {
    name: 'run_external_service_backup',
    description:
      'Trigger an immediate backup of an external service (e.g. a linked PostgreSQL or MongoDB instance)',
    inputSchema: {
      type: 'object',
      properties: {
        service_id: {
          type: 'number',
          description: 'External service ID to back up',
        },
        s3_source_id: {
          type: 'number',
          description: 'S3 source ID where the backup will be stored',
        },
        backup_type: {
          type: 'string',
          description: 'Type of backup (e.g. full, incremental). Defaults to full if omitted.',
        },
      },
      required: ['service_id', 's3_source_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const service_id = requireParam<number>(args, 'service_id');
        const s3_source_id = requireParam<number>(args, 's3_source_id');
        const backup_type = optionalParam<string>(args, 'backup_type');

        const body: Record<string, unknown> = { s3_source_id };
        if (backup_type !== undefined) body.backup_type = backup_type;

        const result = await client.post<Backup>(
          `/backups/external-services/${service_id}/run`,
          body
        );
        return json('Service Backup Started', result);
      }),
  },
];
