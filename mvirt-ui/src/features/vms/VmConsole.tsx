import { useEffect, useRef, useState } from 'react'
import { Terminal } from 'xterm'
import { FitAddon } from 'xterm-addon-fit'
import { Maximize2, Minimize2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { createWebSocket } from '@/api/client'
import { cn } from '@/lib/utils'
import 'xterm/css/xterm.css'

interface VmConsoleProps {
  vmId: string
}

export function VmConsole({ vmId }: VmConsoleProps) {
  const terminalRef = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitAddonRef = useRef<FitAddon | null>(null)
  const wsRef = useRef<WebSocket | null>(null)
  const [connected, setConnected] = useState(false)
  const [fullscreen, setFullscreen] = useState(false)

  useEffect(() => {
    if (!terminalRef.current) return

    const term = new Terminal({
      theme: {
        background: '#0a0a0a',
        foreground: '#fafafa',
        cursor: '#fafafa',
        cursorAccent: '#0a0a0a',
        selectionBackground: '#3b3b3b',
      },
      fontFamily: 'JetBrains Mono, monospace',
      fontSize: 14,
      cursorBlink: true,
    })

    const fitAddon = new FitAddon()
    term.loadAddon(fitAddon)
    term.open(terminalRef.current)
    fitAddon.fit()

    termRef.current = term
    fitAddonRef.current = fitAddon

    // Connect to WebSocket
    const ws = createWebSocket(`/vms/${vmId}/console`)
    wsRef.current = ws

    ws.onopen = () => {
      setConnected(true)
      term.writeln('\x1b[32mConnected to console\x1b[0m')
      term.writeln('Press Ctrl+a t to detach\n')
    }

    ws.onmessage = (event) => {
      term.write(event.data)
    }

    ws.onclose = () => {
      setConnected(false)
      term.writeln('\n\x1b[31mDisconnected\x1b[0m')
    }

    ws.onerror = () => {
      setConnected(false)
      term.writeln('\n\x1b[31mConnection error\x1b[0m')
    }

    term.onData((data) => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(data)
      }
    })

    // Handle resize
    const resizeObserver = new ResizeObserver(() => {
      fitAddon.fit()
    })
    resizeObserver.observe(terminalRef.current)

    return () => {
      resizeObserver.disconnect()
      ws.close()
      term.dispose()
    }
  }, [vmId])

  return (
    <Card className={cn(fullscreen && 'fixed inset-4 z-50')}>
      <CardHeader className="flex flex-row items-center justify-between py-2">
        <CardTitle className="text-sm font-medium flex items-center gap-2">
          Console
          <span
            className={cn(
              'h-2 w-2 rounded-full',
              connected ? 'bg-state-running' : 'bg-state-stopped'
            )}
          />
        </CardTitle>
        <Button
          variant="ghost"
          size="icon"
          onClick={() => setFullscreen(!fullscreen)}
        >
          {fullscreen ? (
            <Minimize2 className="h-4 w-4" />
          ) : (
            <Maximize2 className="h-4 w-4" />
          )}
        </Button>
      </CardHeader>
      <CardContent className="p-0">
        <div
          ref={terminalRef}
          className={cn('bg-background', fullscreen ? 'h-[calc(100vh-8rem)]' : 'h-96')}
        />
      </CardContent>
    </Card>
  )
}
