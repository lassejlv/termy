import { useSuspenseQuery } from '@tanstack/react-query'
import { createFileRoute } from '@tanstack/react-router'
import type { ReactNode } from 'react'
import { currentUserQuery } from '@/query'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'

export const Route = createFileRoute('/dashboard/')({
  component: Overview,
})

function Overview() {
  const user = useSuspenseQuery(currentUserQuery)

  if (!user.data) {
    return null
  }

  const displayName = user.data.name?.trim() || user.data.email.split('@')[0]

  return (
    <div className="mx-auto flex w-full max-w-4xl flex-1 flex-col gap-6 p-4 py-8 md:p-8">
      <div>
        <h1 className="font-semibold text-xl tracking-tight">
          Welcome back, {displayName}.
        </h1>
        <p className="mt-1 text-muted-foreground text-sm">
          Your account and the terminals connected to it.
        </p>
      </div>
      <div className="grid gap-4 md:grid-cols-2">
        <Card>
          <CardHeader className="border-b p-4">
            <CardTitle className="text-sm">Account</CardTitle>
          </CardHeader>
          <CardContent className="p-4 space-y-2.5 text-sm">
            <InfoRow label="name" value={user.data.name ?? '—'} />
            <InfoRow label="email" value={user.data.email} />
            <InfoRow label="plan" value="free" />
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="border-b p-4">
            <CardTitle className="text-sm">Session</CardTitle>
          </CardHeader>
          <CardContent className="p-4 space-y-2.5 text-sm">
            <InfoRow
              label="status"
              value={
                <Badge variant="success" className="gap-1.5">
                  <span aria-hidden="true" className="size-1.5 rounded-full bg-success" />
                  active
                </Badge>
              }
            />
            <InfoRow label="client" value="this browser" />
          </CardContent>
        </Card>
        <Card className="md:col-span-2">
          <CardHeader className="border-b p-4">
            <CardTitle className="text-sm">Devices</CardTitle>
          </CardHeader>
          <CardContent className="p-4 text-sm">
            <p className="text-muted-foreground">
              No terminals linked yet. Sign in from the Termy desktop app and it will show
              up here.
            </p>
            <code className="mt-3 block w-fit rounded-lg border bg-code px-3 py-2 text-code-foreground text-xs">
              <span aria-hidden="true" className="me-2 font-bold text-(--prompt)">
                ❯
              </span>
              termy login
            </code>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}

function InfoRow({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4">
      <span className="text-muted-foreground">{label}</span>
      <span className="min-w-0 truncate text-end">{value}</span>
    </div>
  )
}
