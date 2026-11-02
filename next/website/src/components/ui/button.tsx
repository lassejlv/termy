import * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { Slot } from 'radix-ui'
import { cn } from '@/lib/utils'

const buttonVariants = cva(
  "inline-flex shrink-0 items-center justify-center gap-2 rounded-full font-semibold whitespace-nowrap transition-[background-color,color,transform] outline-none select-none focus-visible:ring-3 focus-visible:ring-ring/50 active:translate-y-px disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: 'bg-primary text-primary-foreground hover:bg-[#ff7d6a]',
        light: 'bg-foreground text-background hover:bg-white',
        secondary: 'bg-secondary text-secondary-foreground hover:bg-white/12',
        ghost: 'text-soft hover:bg-white/6 hover:text-foreground',
        link: 'rounded-none font-medium text-foreground underline decoration-white/30 underline-offset-[6px] hover:decoration-white/80',
      },
      size: {
        default: 'h-10 px-[18px] text-[15px]',
        sm: 'h-8 px-3.5 text-[14px]',
        lg: 'h-11 px-5 text-[15px]',
        link: 'h-auto px-0 text-[15px]',
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
