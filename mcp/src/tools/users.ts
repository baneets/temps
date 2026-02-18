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

interface User {
  id: number;
  username: string;
  email?: string;
  roles?: string[];
  created_at?: string;
  deleted_at?: string;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_users',
    description: 'List all users',
    inputSchema: {
      type: 'object',
      properties: {
        include_deleted: {
          type: 'boolean',
          description: 'Include soft-deleted users',
        },
      },
    },
    handler: (args) =>
      handleToolCall(async () => {
        const includeDeleted = optionalParam<boolean>(
          args,
          'include_deleted'
        );
        const client = getClient();

        const query: Record<string, unknown> = {};
        if (includeDeleted !== undefined)
          query.include_deleted = includeDeleted;

        const users = await client.get<User[]>('/users', query);

        if (!users || users.length === 0) {
          return ok('No users found.');
        }

        const rows = users.map((u) => [
          String(u.id),
          u.username,
          u.email || 'N/A',
          (u.roles || []).join(', ') || 'none',
        ]);

        return ok(table(['ID', 'Username', 'Email', 'Roles'], rows));
      }),
  },
  {
    name: 'create_user',
    description: 'Create a new user',
    inputSchema: {
      type: 'object',
      properties: {
        username: {
          type: 'string',
          description: 'Username',
        },
        email: {
          type: 'string',
          description: 'Email address',
        },
        password: {
          type: 'string',
          description: 'Password',
        },
        roles: {
          type: 'array',
          items: { type: 'string' },
          description: 'Roles to assign',
        },
      },
      required: ['username'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const username = requireParam<string>(args, 'username');
        const email = optionalParam<string>(args, 'email');
        const password = optionalParam<string>(args, 'password');
        const roles = optionalParam<string[]>(args, 'roles');
        const client = getClient();

        const body: Record<string, unknown> = { username };
        if (email !== undefined) body.email = email;
        if (password !== undefined) body.password = password;
        if (roles !== undefined) body.roles = roles;

        const user = await client.post<User>('/users', body);

        return json('User Created', user);
      }),
  },
  {
    name: 'get_current_user',
    description: 'Get the currently authenticated user',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const user = await client.get<User>('/user/me');

        return json('Current User', user);
      }),
  },
  {
    name: 'delete_user',
    description: 'Delete a user (soft delete)',
    inputSchema: {
      type: 'object',
      properties: {
        user_id: {
          type: 'number',
          description: 'User ID',
        },
      },
      required: ['user_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const userId = requireParam<number>(args, 'user_id');
        const client = getClient();
        await client.delete(`/users/${userId}`);

        return ok(`User ${userId} deleted successfully.`);
      }),
  },
  {
    name: 'restore_user',
    description: 'Restore a soft-deleted user',
    inputSchema: {
      type: 'object',
      properties: {
        user_id: {
          type: 'number',
          description: 'User ID',
        },
      },
      required: ['user_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const userId = requireParam<number>(args, 'user_id');
        const client = getClient();
        await client.post(`/users/${userId}/restore`);

        return ok(`User ${userId} restored successfully.`);
      }),
  },
  {
    name: 'assign_role',
    description: 'Assign a role to a user',
    inputSchema: {
      type: 'object',
      properties: {
        user_id: {
          type: 'number',
          description: 'User ID',
        },
        role_type: {
          type: 'string',
          description: 'Role type to assign',
        },
      },
      required: ['user_id', 'role_type'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const userId = requireParam<number>(args, 'user_id');
        const roleType = requireParam<string>(args, 'role_type');
        const client = getClient();

        await client.post(`/users/${userId}/roles`, {
          role_type: roleType,
          user_id: userId,
        });

        return ok(
          `Role '${roleType}' assigned to user ${userId} successfully.`
        );
      }),
  },
  {
    name: 'remove_role',
    description: 'Remove a role from a user',
    inputSchema: {
      type: 'object',
      properties: {
        user_id: {
          type: 'number',
          description: 'User ID',
        },
        role_type: {
          type: 'string',
          description: 'Role type to remove',
        },
      },
      required: ['user_id', 'role_type'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const userId = requireParam<number>(args, 'user_id');
        const roleType = requireParam<string>(args, 'role_type');
        const client = getClient();

        await client.delete(`/users/${userId}/roles/${roleType}`);

        return ok(
          `Role '${roleType}' removed from user ${userId} successfully.`
        );
      }),
  },
];
