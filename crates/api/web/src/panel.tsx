import type { CSSProperties, ReactNode } from 'react'

export function AuthPanel({
  title,
  children,
  live,
}: {
  title: string
  children: ReactNode
  live?: boolean
}) {
  return (
    <section className="term-window" aria-live={live ? 'polite' : undefined}>
      <div className="term-titlebar" aria-hidden="true">
        <span className="term-dots">
          <i />
          <i />
          <i />
        </span>
        <span className="term-title">{title}</span>
      </div>
      <div className="term-body">{children}</div>
    </section>
  )
}

export function PromptLine({ text }: { text: string }) {
  return (
    <p className="prompt-line">
      <span className="prompt-glyph" aria-hidden="true">
        ❯
      </span>
      <span className="prompt-text" style={{ '--chars': text.length } as CSSProperties}>
        {text}
      </span>
    </p>
  )
}
