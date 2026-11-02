import * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { Slot } from 'radix-ui'
import { cn } from '@/lib/utils'

// In .astro files, put `buttonVariants()` on the <a> or <button> directly:
// `asChild` can't reach markup Astro renders as children.
const buttonVariants = cva(
  "inline-flex shrink-0 items-center justify-center gap-2 rounded-[10px] font-medium whitespace-nowrap transition-[background-color,color,transform] outline-none select-none focus-visible:ring-3 focus-visible:ring-ring/50 active:translate-y-px disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: 'bg-primary text-primary-foreground hover:bg-[#9fc0ff]',
        light: 'bg-foreground text-background hover:bg-white',
        secondary: 'bg-secondary text-secondary-foreground hover:bg-white/12',
        ghost: 'text-soft hover:bg-white/6 hover:text-foreground',
        link: 'rounded-none text-foreground underline decoration-white/30 underline-offset-[5px] hover:decoration-white/80',
      },
      size: {
        default: 'h-9 px-3.5 text-[14px]',
        sm: 'h-8 px-3 text-[13px]',
        lg: 'h-10 px-4 text-[14px]',
        link: 'h-auto px-0 text-[14px]',
      },
    },
    defaultVariants: { variant: 'default', size: 'default' },
  },
)

function Button({
  className,
  variant,
  size,
  asChild = false,
  ...props
}: React.ComponentProps<'button'> & VariantProps<typeof buttonVariants> & { asChild?: boolean }) {
  const Comp = asChild ? Slot.Root : 'button'
  return <Comp data-slot="button" className={cn(buttonVariants({ variant, size, className }))} {...props} />
}

export { Button, buttonVariants }
