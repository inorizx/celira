import { useEffect } from 'react'

export function useUnreadIndicators(unread: number) {
  useEffect(() => {
    document.title = unread > 0 ? `(${unread > 99 ? '99+' : unread}) Celira` : 'Celira'
  }, [unread])

  useEffect(() => {
    let cancelled = false
    const link = document.querySelector<HTMLLinkElement>('link[rel="icon"]')
    if (!link) return
    const original = link.href
    const img = new Image()
    img.src = '/logo/logo-mark.svg'
    img.onload = () => {
      if (cancelled) return
      const canvas = document.createElement('canvas')
      canvas.width = 64
      canvas.height = 64
      const ctx = canvas.getContext('2d')
      if (!ctx) return
      ctx.drawImage(img, 0, 0, 64, 64)
      if (unread > 0) {
        const color = getComputedStyle(document.documentElement)
          .getPropertyValue('--presence-failed')
          .trim()
        if (color) {
          ctx.fillStyle = color
          ctx.beginPath()
          ctx.arc(48, 16, 12, 0, Math.PI * 2)
          ctx.fill()
        }
      }
      link.href = canvas.toDataURL()
    }
    return () => {
      cancelled = true
      link.href = original
    }
  }, [unread])
}
