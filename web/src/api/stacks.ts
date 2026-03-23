import { client } from './client/client.gen'

export interface Stack {
  id: number
  name: string
  description: string | null
  compose_content: string
  env_content: string | null
  node_id: number | null
  state: string
  repo_url: string | null
  repo_branch: string | null
  repo_compose_path: string | null
  last_synced_at: string | null
  created_at: string
  updated_at: string
}

export interface PaginatedStacks {
  items: Stack[]
  total: number
}

export interface CreateStackRequest {
  name: string
  description?: string | null
  compose_content?: string
  env_content?: string | null
  node_id?: number | null
  repo_url?: string
  repo_branch?: string
  repo_compose_path?: string
  repo_access_token?: string
}

export interface UpdateStackRequest {
  name?: string
  description?: string | null
  compose_content?: string
  env_content?: string | null
}

export async function listStacks(page = 1, pageSize = 20) {
  return client.get<PaginatedStacks>({
    url: '/stacks',
    query: { page, page_size: pageSize },
  })
}

export async function getStack(id: number) {
  return client.get<Stack>({
    url: '/stacks/{id}',
    path: { id },
  })
}

export async function createStack(body: CreateStackRequest) {
  return client.post<Stack>({
    url: '/stacks',
    body,
  })
}

export async function updateStack(id: number, body: UpdateStackRequest) {
  return client.patch<Stack>({
    url: '/stacks/{id}',
    path: { id },
    body,
  })
}

export async function deleteStack(id: number) {
  return client.delete<void>({
    url: '/stacks/{id}',
    path: { id },
  })
}

export async function deployStack(id: number) {
  return client.post<Stack>({
    url: '/stacks/{id}/deploy',
    path: { id },
  })
}

export async function stopStack(id: number) {
  return client.post<Stack>({
    url: '/stacks/{id}/stop',
    path: { id },
  })
}

export async function restartStack(id: number) {
  return client.post<Stack>({
    url: '/stacks/{id}/restart',
    path: { id },
  })
}

export async function pullStack(id: number) {
  return client.post<Stack>({
    url: '/stacks/{id}/pull',
    path: { id },
  })
}

export async function syncStack(id: number) {
  return client.post<Stack>({
    url: '/stacks/{id}/sync',
    path: { id },
  })
}

export interface DiscoverComposeRequest {
  repo_url: string
  repo_branch?: string
  repo_access_token?: string
}

export interface DiscoverComposeResponse {
  files: string[]
}

export async function discoverComposeFiles(body: DiscoverComposeRequest) {
  return client.post<DiscoverComposeResponse>({
    url: '/stacks/discover',
    body,
  })
}

export interface StackContainersResponse {
  raw: string
}

export interface StackLogsResponse {
  logs: string
}

export interface ComposeContainer {
  ID: string
  Name: string
  Service: string
  State: string
  Status: string
  Health: string
  Image: string
  Publishers: { URL: string; TargetPort: number; PublishedPort: number; Protocol: string }[]
}

export async function getStackContainers(id: number) {
  return client.get<StackContainersResponse>({
    url: '/stacks/{id}/containers',
    path: { id },
  })
}

export async function getStackLogs(
  id: number,
  service?: string,
  tail?: number
) {
  return client.get<StackLogsResponse>({
    url: '/stacks/{id}/logs',
    path: { id },
    query: { service, tail },
  })
}

export interface ContainerStats {
  container_id: string
  container_name: string
  service: string
  cpu_percent: number
  memory_bytes: number
  memory_limit: number
  memory_percent: number
  network_rx_bytes: number
  network_tx_bytes: number
}

export interface StackStatsResponse {
  containers: ContainerStats[]
}

export async function getStackStats(id: number) {
  return client.get<StackStatsResponse>({
    url: '/stacks/{id}/stats',
    path: { id },
  })
}

export interface StackRoute {
  id: number
  stack_id: number
  domain: string
  target_port: number
  service_name: string | null
  enabled: boolean
  created_at: string
  updated_at: string
}

export interface CreateStackRouteRequest {
  domain: string
  target_port: number
  service_name?: string | null
}

export async function listStackRoutes(stackId: number) {
  return client.get<StackRoute[]>({
    url: '/stacks/{id}/routes',
    path: { id: stackId },
  })
}

export async function createStackRoute(
  stackId: number,
  body: CreateStackRouteRequest
) {
  return client.post<StackRoute>({
    url: '/stacks/{id}/routes',
    path: { id: stackId },
    body,
  })
}

export async function deleteStackRoute(stackId: number, routeId: number) {
  return client.delete<void>({
    url: '/stacks/{stack_id}/routes/{route_id}',
    path: { stack_id: stackId, route_id: routeId },
  })
}

export async function toggleStackRoute(
  stackId: number,
  routeId: number,
  enabled: boolean
) {
  return client.patch<StackRoute>({
    url: '/stacks/{stack_id}/routes/{route_id}',
    path: { stack_id: stackId, route_id: routeId },
    body: { enabled },
  })
}
