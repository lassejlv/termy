import Lenis from 'lenis'
import 'lenis/dist/lenis.css'
import { useEffect } from 'react'

/** Heavy, eased page scrolling, unless the visitor asked for less motion. */
export function SmoothScroll() {
  useEffect(() => {
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return
    const lenis = new Lenis({
      autoRaf: true,
      // Low lerp: the page trails the wheel and settles slowly.
      lerp: 0.07,
      wheelMultiplier: 0.9,
      anchors: { offset: -24 },
    })
    return () => lenis.destroy()
  }, [])
  return null
}
