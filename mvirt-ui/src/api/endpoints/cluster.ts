import { get } from '../client'
import { Node, ClusterInfo, NodeState, NodeRole } from '@/types'

interface BackendNode {
  id: number
  name: string
  address: string
  state: string
  is_leader: boolean
}

interface BackendCluster {
  cluster_id: string
  leader_id: number
  current_term: number
  commit_index: number
  nodes: BackendNode[]
}

function mapNode(n: BackendNode): Node {
  return {
    id: String(n.id),
    name: n.name,
    address: n.address,
    state: n.state === 'leader' || n.state === 'follower' ? NodeState.ONLINE : NodeState.OFFLINE,
    role: n.is_leader ? NodeRole.LEADER : NodeRole.FOLLOWER,
    version: '0.1.0',
    cpuCount: 0,
    memoryTotalBytes: 0,
    memoryUsedBytes: 0,
    vmCount: 0,
    uptime: 0,
    lastSeen: new Date().toISOString(),
  }
}

export async function getClusterInfo(): Promise<ClusterInfo> {
  const data = await get<BackendCluster>('/cluster')
  return {
    id: data.cluster_id,
    name: data.cluster_id,
    nodeCount: data.nodes?.length ?? 0,
    leaderNodeId: String(data.leader_id),
    term: data.current_term,
    createdAt: new Date().toISOString(),
  }
}

export async function getNodes(): Promise<Node[]> {
  const data = await get<BackendCluster>('/cluster')
  return (data.nodes ?? []).map(mapNode)
}

export async function getNode(id: string): Promise<Node> {
  const nodes = await getNodes()
  const node = nodes.find(n => n.id === id)
  if (!node) throw new Error(`Node ${id} not found`)
  return node
}

export async function setNodeMaintenance(id: string, _maintenance: boolean): Promise<Node> {
  // TODO: implement when backend supports this
  return getNode(id)
}

export async function removeNode(_id: string): Promise<void> {
  // TODO: implement when backend supports this
}
