import { client } from './client/client.gen'

export interface Stack {
  id: number
  name: string
  description: string | null
  compose_content: string
  env_content: string | null
  node_id: number | null
  state: string
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
  compose_content: string
  env_content?: string | null
  node_id?: number | null
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
