/**
 * Service management tools for Temps MCP Server
 *
 * Manages external services (PostgreSQL, MongoDB, Redis, S3) including
 * lifecycle operations and project linking.
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

interface Service {
  id: number;
  name: string;
  service_type: string;
  status: string;
  docker_image?: string;
  created_at?: string;
  updated_at?: string;
}

interface ServiceDetails {
  service: Service;
  current_parameters: Record<string, unknown>;
}

interface ServiceType {
  name: string;
  description?: string;
  default_image?: string;
}

interface LinkedProject {
  id: number;
  name: string;
  slug?: string;
}

interface EnvVar {
  key: string;
  value: string;
}

interface Backup {
  id: number;
  status: string;
  backup_type?: string;
  created_at?: string;
}

export const tools: ToolDefinition[] = [
  {
    name: 'list_services',
    description: 'List all external services (databases, caches, storage) managed by the platform',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const services = await client.get<Service[]>('/external-services');

        if (!services || services.length === 0) {
          return ok('No services found.');
        }

        const rows = services.map((s) => [
          String(s.id),
          s.name,
          s.service_type,
          s.status,
        ]);

        return ok(
          `## Services\n\n${table(['ID', 'Name', 'Type', 'Status'], rows)}`
        );
      }),
  },

  {
    name: 'create_service',
    description:
      'Create a new external service instance (postgres, mongodb, redis, or s3)',
    inputSchema: {
      type: 'object',
      properties: {
        name: {
          type: 'string',
          description: 'Name for the service',
        },
        service_type: {
          type: 'string',
          description: 'Type of service to create',
          enum: ['postgres', 'mongodb', 'redis', 's3'],
        },
        parameters: {
          type: 'object',
          description:
            'Optional service-specific parameters (e.g. version, memory limits). Use get_service_type_parameters to see available options.',
        },
      },
      required: ['name', 'service_type'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const name = requireParam<string>(args, 'name');
        const service_type = requireParam<string>(args, 'service_type');
        const parameters = optionalParam<Record<string, unknown>>(args, 'parameters');

        const body: Record<string, unknown> = { name, service_type };
        if (parameters) body.parameters = parameters;

        const service = await client.post<Service>('/external-services', body);
        return json('Service Created', service);
      }),
  },

  {
    name: 'get_service',
    description:
      'Get detailed information about a specific service including its current parameters',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Service ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const details = await client.get<ServiceDetails>(`/external-services/${id}`);
        return json('Service Details', details);
      }),
  },

  {
    name: 'delete_service',
    description:
      'Delete an external service. The service must be stopped and unlinked from all projects first.',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Service ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        await client.delete(`/external-services/${id}`);
        return ok(`Service ${id} deleted successfully.`);
      }),
  },

  {
    name: 'start_service',
    description: 'Start a stopped external service',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Service ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        await client.post(`/external-services/${id}/start`);
        return ok(`Service ${id} started successfully.`);
      }),
  },

  {
    name: 'stop_service',
    description: 'Stop a running external service',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Service ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        await client.post(`/external-services/${id}/stop`);
        return ok(`Service ${id} stopped successfully.`);
      }),
  },

  {
    name: 'get_service_types',
    description:
      'List all available service types (e.g. postgres, mongodb, redis, s3) and their default configurations',
    inputSchema: {
      type: 'object',
      properties: {},
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const types = await client.get<ServiceType[]>('/external-services/types');
        return json('Available Service Types', types);
      }),
  },

  {
    name: 'get_service_type_parameters',
    description:
      'Get the JSON Schema describing configurable parameters for a specific service type',
    inputSchema: {
      type: 'object',
      properties: {
        service_type: {
          type: 'string',
          description: 'Service type (e.g. postgres, mongodb, redis, s3)',
        },
      },
      required: ['service_type'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const serviceType = requireParam<string>(args, 'service_type');
        const schema = await client.get<Record<string, unknown>>(
          `/external-services/types/${serviceType}/parameters`
        );
        return json(`Parameters for ${serviceType}`, schema);
      }),
  },

  {
    name: 'update_service',
    description:
      'Update a service configuration (docker image and/or parameters)',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Service ID',
        },
        docker_image: {
          type: 'string',
          description: 'New Docker image to use for the service',
        },
        parameters: {
          type: 'object',
          description: 'Updated service-specific parameters',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const docker_image = optionalParam<string>(args, 'docker_image');
        const parameters = optionalParam<Record<string, unknown>>(args, 'parameters');

        const body: Record<string, unknown> = {};
        if (docker_image !== undefined) body.docker_image = docker_image;
        if (parameters !== undefined) body.parameters = parameters;

        const service = await client.put<Service>(`/external-services/${id}`, body);
        return json('Service Updated', service);
      }),
  },

  {
    name: 'upgrade_service',
    description:
      'Upgrade a service to a new Docker image version. This will restart the service.',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Service ID',
        },
        docker_image: {
          type: 'string',
          description: 'Docker image to upgrade to (e.g. postgres:16)',
        },
      },
      required: ['id', 'docker_image'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const docker_image = requireParam<string>(args, 'docker_image');
        const service = await client.post<Service>(`/external-services/${id}/upgrade`, {
          docker_image,
        });
        return json('Service Upgraded', service);
      }),
  },

  {
    name: 'link_service_to_project',
    description:
      'Link an external service to a project so it can access connection credentials via environment variables',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Service ID',
        },
        project_id: {
          type: 'number',
          description: 'Project ID to link the service to',
        },
      },
      required: ['id', 'project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const project_id = requireParam<number>(args, 'project_id');
        await client.post(`/external-services/${id}/projects`, { project_id });
        return ok(
          `Service ${id} linked to project ${project_id} successfully.`
        );
      }),
  },

  {
    name: 'unlink_service_from_project',
    description:
      'Remove the link between an external service and a project',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Service ID',
        },
        project_id: {
          type: 'number',
          description: 'Project ID to unlink from the service',
        },
      },
      required: ['id', 'project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const project_id = requireParam<number>(args, 'project_id');
        await client.delete(`/external-services/${id}/projects/${project_id}`);
        return ok(
          `Service ${id} unlinked from project ${project_id} successfully.`
        );
      }),
  },

  {
    name: 'get_service_environment_variables',
    description:
      'Get the environment variables that a linked service exposes to a specific project',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Service ID',
        },
        project_id: {
          type: 'number',
          description: 'Project ID',
        },
      },
      required: ['id', 'project_id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const project_id = requireParam<number>(args, 'project_id');
        const envVars = await client.get<EnvVar[]>(
          `/external-services/${id}/projects/${project_id}/environment`
        );
        return json(`Environment Variables (Service ${id} → Project ${project_id})`, envVars);
      }),
  },

  {
    name: 'list_service_projects',
    description: 'List all projects linked to a specific service',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'number',
          description: 'Service ID',
        },
      },
      required: ['id'],
    },
    handler: (args) =>
      handleToolCall(async () => {
        const client = getClient();
        const id = requireParam<number>(args, 'id');
        const projects = await client.get<LinkedProject[]>(
          `/external-services/${id}/projects`
        );

        if (!projects || projects.length === 0) {
          return ok(`No projects linked to service ${id}.`);
        }

        const rows = projects.map((p) => [
          String(p.id),
          p.name,
          p.slug || '',
        ]);

        return ok(
          `## Projects Linked to Service ${id}\n\n${table(['ID', 'Name', 'Slug'], rows)}`
        );
      }),
  },
];
