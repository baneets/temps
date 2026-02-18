import { getClient } from '../api/index.js';
import { ok, json, table, formatDate, handleToolCall, requireParam, optionalParam } from './_helpers.js';
import type { ToolDefinition } from '../types/index.js';

export const tools: ToolDefinition[] = [
  {
    name: 'list_audit_logs',
    description: 'List audit logs with optional filtering by operation type, user, and date range',
    inputSchema: {
      type: 'object',
      properties: {
        limit: { type: 'number', description: 'Maximum number of logs to return' },
        offset: { type: 'number', description: 'Offset for pagination' },
        operation_type: { type: 'string', description: 'Filter by operation type (e.g. CREATE, UPDATE, DELETE)' },
        user_id: { type: 'number', description: 'Filter by user ID' },
        from: { type: 'string', description: 'Start date filter (ISO 8601)' },
        to: { type: 'string', description: 'End date filter (ISO 8601)' },
      },
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const query: Record<string, unknown> = {};

        const limit = optionalParam<number>(args, 'limit');
        const offset = optionalParam<number>(args, 'offset');
        const operationType = optionalParam<string>(args, 'operation_type');
        const userId = optionalParam<number>(args, 'user_id');
        const from = optionalParam<string>(args, 'from');
        const to = optionalParam<string>(args, 'to');

        if (limit !== undefined) query.limit = limit;
        if (offset !== undefined) query.offset = offset;
        if (operationType !== undefined) query.operation_type = operationType;
        if (userId !== undefined) query.user_id = userId;
        if (from !== undefined) query.from = from;
        if (to !== undefined) query.to = to;

        const data = await client.get<Array<Record<string, unknown>>>('/audit/logs', query);
        const logs = Array.isArray(data) ? data : [];

        if (logs.length === 0) {
          return ok('No audit logs found matching the criteria.');
        }

        const rows = logs.map((log) => [
          String(log.id ?? ''),
          String(log.operation ?? log.operation_type ?? ''),
          String(log.user ?? log.user_id ?? ''),
          formatDate(log.created_at as string | null ?? log.timestamp as string | null),
        ]);

        return ok(
          `## Audit Logs (${logs.length})\n\n${table(['ID', 'Operation', 'User', 'Timestamp'], rows)}`
        );
      }),
  },
  {
    name: 'get_audit_log',
    description: 'Get full details of a specific audit log entry including user info, IP address, and changed data',
    inputSchema: {
      type: 'object',
      properties: {
        id: { type: 'number', description: 'Audit log entry ID' },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const data = await client.get<Record<string, unknown>>(`/audit/logs/${id}`);
        return json('Audit Log Details', data);
      }),
  },
];
