import { get, del } from '../client'
import type { Node, ClusterInfo } from '@/types'

export async function getClusterInfo(): Promise<ClusterInfo> {
  return get<ClusterInfo>('/controlplane')
}

export async function getNodes(): Promise<Node[]> {
  return get<Node[]>('/nodes')
}

export async function getNode(id: string): Promise<Node> {
  return get<Node>(`/nodes/${id}`)
}

export async function setNodeMaintenance(id: string, _maintenance: boolean): Promise<Node> {
  // TODO: implement when backend supports this
  return getNode(id)
}

export async function removeNode(id: string): Promise<void> {
  await del<void>(`/nodes/${id}`)
}
