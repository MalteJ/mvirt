import { useEffect, useRef, useCallback, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { createEventSource } from '@/api/client'
import { vmKeys } from './queries/useVms'
import type { Vm, LogEntry } from '@/types'

export function useVmEventStream() {
  const queryClient = useQueryClient()
  const eventSourceRef = useRef<EventSource | null>(null)

  useEffect(() => {
    const eventSource = createEventSource('/events/vms')
    eventSourceRef.current = eventSource

    eventSource.onmessage = (event) => {
      try {
        const vm: Vm = JSON.parse(event.data)
        queryClient.setQueryData(vmKeys.detail(vm.id), vm)
        queryClient.invalidateQueries({ queryKey: vmKeys.lists() })
      } catch {
        console.error('Failed to parse VM event')
      }
    }

    eventSource.onerror = () => {
      eventSource.close()
      // Reconnect after 5 seconds
      setTimeout(() => {
        if (eventSourceRef.current === eventSource) {
          eventSourceRef.current = createEventSource('/events/vms')
        }
      }, 5000)
    }

    return () => {
      eventSource.close()
      eventSourceRef.current = null
    }
  }, [queryClient])
}

export function useLogStream(onLog: (entry: LogEntry) => void) {
  const eventSourceRef = useRef<EventSource | null>(null)
  const [connected, setConnected] = useState(false)

  const connect = useCallback(() => {
    if (eventSourceRef.current) {
      eventSourceRef.current.close()
    }

    const eventSource = createEventSource('/logs/stream')
    eventSourceRef.current = eventSource

    eventSource.onopen = () => {
      setConnected(true)
    }

    eventSource.onmessage = (event) => {
      try {
        const entry: LogEntry = JSON.parse(event.data)
        onLog(entry)
      } catch {
        console.error('Failed to parse log event')
      }
    }

    eventSource.onerror = () => {
      setConnected(false)
      eventSource.close()
      // Reconnect after 5 seconds
      setTimeout(() => {
        if (eventSourceRef.current === eventSource) {
          connect()
        }
      }, 5000)
    }
  }, [onLog])

  const disconnect = useCallback(() => {
    if (eventSourceRef.current) {
      eventSourceRef.current.close()
      eventSourceRef.current = null
      setConnected(false)
    }
  }, [])

  useEffect(() => {
    connect()
    return () => {
      disconnect()
    }
  }, [connect, disconnect])

  return { connected, reconnect: connect }
}
