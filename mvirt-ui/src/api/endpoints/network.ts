import { get, post, del } from '../client'
import type { Network, Nic, CreateNetworkRequest, CreateNicRequest } from '@/types'

export async function listNetworks(projectId: string): Promise<Network[]> {
  const response = await get<{ networks: Network[] }>(`/projects/${projectId}/networks`)
  return response.networks
}

export async function getNetwork(id: string): Promise<Network> {
  return get<Network>(`/networks/${id}`)
}

export async function createNetwork(projectId: string, request: CreateNetworkRequest): Promise<Network> {
  return post<Network>(`/projects/${projectId}/networks`, request)
}

export async function deleteNetwork(id: string): Promise<void> {
  await del<void>(`/networks/${id}`)
}

export async function listNics(projectId: string): Promise<Nic[]> {
  const response = await get<{ nics: Nic[] }>(`/projects/${projectId}/nics`)
  return response.nics
}

export async function getNic(id: string): Promise<Nic> {
  return get<Nic>(`/nics/${id}`)
}

export async function createNic(projectId: string, request: CreateNicRequest): Promise<Nic> {
  return post<Nic>(`/projects/${projectId}/nics`, request)
}

export async function deleteNic(id: string): Promise<void> {
  await del<void>(`/nics/${id}`)
}

export async function attachNic(nicId: string, vmId: string): Promise<Nic> {
  return post<Nic>(`/nics/${nicId}/attach`, { vmId })
}

export async function detachNic(nicId: string): Promise<Nic> {
  return post<Nic>(`/nics/${nicId}/detach`)
}
