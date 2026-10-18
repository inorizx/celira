export type BroadcastMessage =
  | { type: 'rooms.changed' }
  | { type: 'messages.changed'; roomId: string }
  | { type: 'read.cursor'; roomId: string }
  | { type: 'prefs.changed' }

const CHANNEL = 'celira-v1'
let channel: BroadcastChannel | null = null

function getChannel(): BroadcastChannel | null {
  if (typeof BroadcastChannel === 'undefined') return null
  if (!channel) {
    try {
      channel = new BroadcastChannel(CHANNEL)
    } catch {
      channel = null
    }
  }
  return channel
}

export function emitBroadcast(msg: BroadcastMessage): void {
  getChannel()?.postMessage(msg)
}

export function subscribeBroadcast(handler: (msg: BroadcastMessage) => void): () => void {
  const bc = getChannel()
  if (!bc) return () => {}
  bc.onmessage = (ev) => {
    const msg = ev.data as BroadcastMessage | undefined
    if (msg && typeof msg === 'object' && 'type' in msg) handler(msg)
  }
  return () => {
    bc.onmessage = null
  }
}
