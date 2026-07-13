import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { createFileRoute } from '@tanstack/react-router'
import {
  PlusIcon,
  SquareIcon,
  TerminalIcon,
  Trash2Icon,
  ZapIcon,
} from 'lucide-react'
import { useState } from 'react'
import type { FormEvent } from 'react'
import {
  ApiError,
  createProject,
  deleteProject,
  disconnectRailway,
  getSession,
  startSession,
  stopSession,
} from '@/api'
import { SandboxTerminal } from '@/terminal'
import type { Project } from '@/api'
import { projectsQuery, railwayStatusQuery } from '@/query'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import {
  AlertDialog,
  AlertDialogClose,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogPopup,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogPanel,
  DialogPopup,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog'
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from '@/components/ui/empty'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Spinner } from '@/components/ui/spinner'

export const Route = createFileRoute('/dashboard/projects')({
  loader: ({ context }) => {
    void context.queryClient.prefetchQuery(railwayStatusQuery)
    void context.queryClient.prefetchQuery(projectsQuery)
  },
  component: ProjectsPage,
})

/** Session statuses that mean "still working towards ready". */
const IN_FLIGHT = ['pending', 'provisioning', 'cloning', 'setting_up', 'stopping']

function errorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    return error.message
  }
  return error instanceof Error ? error.message : 'Something went wrong'
}

function ProjectsPage() {
  const railway = useQuery(railwayStatusQuery)
  const projects = useQuery({
    ...projectsQuery,
    refetchInterval: (query) =>
      query.state.data?.some(
        (project) =>
          project.active_session && IN_FLIGHT.includes(project.active_session.status),
      )
        ? 2500
        : false,
  })

  return (
    <div className="mx-auto flex w-full max-w-4xl flex-1 flex-col gap-6 p-4 py-8 md:p-8">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="font-semibold text-xl tracking-tight">Projects</h1>
          <p className="mt-1 text-muted-foreground text-sm">
            Git-backed workspaces that run in disposable cloud sandboxes.
          </p>
        </div>
        {railway.data?.connected ? <NewProjectDialog /> : null}
      </div>

      <RailwayCard
        connected={railway.data?.connected ?? false}
        accountName={railway.data?.account_name ?? null}
        loading={railway.isPending}
      />

      {railway.data?.connected ? (
        <ProjectList projects={projects.data ?? []} loading={projects.isPending} />
      ) : null}
    </div>
  )
}

function RailwayCard({
  connected,
  accountName,
  loading,
}: {
  connected: boolean
  accountName: string | null
  loading: boolean
}) {
  const queryClient = useQueryClient()
  const disconnect = useMutation({
    mutationFn: disconnectRailway,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: railwayStatusQuery.queryKey }),
  })

  if (loading) {
    return (
      <Card>
        <CardContent className="flex items-center gap-2 p-4 text-muted-foreground text-sm">
          <Spinner className="size-4" /> Checking provider connection…
        </CardContent>
      </Card>
    )
  }

  return (
    <Card>
      <CardHeader className="border-b p-4">
        <CardTitle className="flex items-center justify-between text-sm">
          <span>Compute provider</span>
          {connected ? (
            <Badge variant="success" className="gap-1.5">
              <span aria-hidden="true" className="size-1.5 rounded-full bg-success" />
              railway
            </Badge>
          ) : (
            <Badge variant="outline">not connected</Badge>
          )}
        </CardTitle>
      </CardHeader>
      <CardContent className="p-4 text-sm">
        {connected ? (
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="text-muted-foreground">
              Sandboxes run on{' '}
              <span className="text-foreground">{accountName ?? 'your Railway account'}</span>
              . Railway bills the compute; commit and push before a sandbox is destroyed.
            </p>
            <Button
              variant="outline"
              size="sm"
              disabled={disconnect.isPending}
              onClick={() => disconnect.mutate()}
            >
              Disconnect
            </Button>
          </div>
        ) : (
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="text-muted-foreground">
              Connect your Railway account to run projects in cloud sandboxes. Compute is
              billed to your Railway account.
            </p>
            <Button
              size="sm"
              render={<a href="/api/providers/railway/connect" />}
            >
              Connect Railway
            </Button>
          </div>
        )}
        {disconnect.isError ? (
          <p className="mt-2 text-destructive-foreground text-xs">
            {errorMessage(disconnect.error)}
          </p>
        ) : null}
      </CardContent>
    </Card>
  )
}

function ProjectList({ projects, loading }: { projects: Project[]; loading: boolean }) {
  if (loading) {
    return (
      <Card>
        <CardContent className="flex items-center gap-2 p-4 text-muted-foreground text-sm">
          <Spinner className="size-4" /> Loading projects…
        </CardContent>
      </Card>
    )
  }
  if (projects.length === 0) {
    return (
      <Card>
        <CardContent className="p-4">
          <Empty className="py-10">
            <EmptyHeader>
              <EmptyTitle>No projects yet</EmptyTitle>
              <EmptyDescription>
                Create a project from a public GitHub repository, then start a sandbox
                here or from the CLI.
              </EmptyDescription>
            </EmptyHeader>
            <code className="mt-4 block w-fit rounded-lg border bg-code px-3 py-2 text-code-foreground text-xs">
              <span aria-hidden="true" className="me-2 font-bold text-(--prompt)">
                ❯
              </span>
              termy cloud projects create --name app --repo https://github.com/you/app
            </code>
          </Empty>
        </CardContent>
      </Card>
    )
  }
  return (
    <div className="flex flex-col gap-4">
      {projects.map((project) => (
        <ProjectRow key={project.id} project={project} />
      ))}
    </div>
  )
}

function ProjectRow({ project }: { project: Project }) {
  const queryClient = useQueryClient()
  // Failed sessions drop out of `active_session`, so remember the one we
  // watch to keep its failure reason on screen.
  const [watchedSessionId, setWatchedSessionId] = useState<string | null>(null)
  const sessionId = project.active_session?.id ?? watchedSessionId

  const session = useQuery({
    queryKey: ['session', sessionId],
    queryFn: () => getSession(sessionId as string),
    enabled: sessionId !== null,
    refetchInterval: (query) => {
      const status = query.state.data?.status
      return status && IN_FLIGHT.includes(status) ? 2000 : false
    },
  })

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: projectsQuery.queryKey })
    if (sessionId) {
      void queryClient.invalidateQueries({ queryKey: ['session', sessionId] })
    }
  }

  const start = useMutation({
    mutationFn: () => startSession(project.id),
    onSuccess: (started) => {
      setWatchedSessionId(started.session_id)
      invalidate()
    },
  })
  const stop = useMutation({
    mutationFn: () => stopSession(sessionId as string),
    onSuccess: invalidate,
  })
  const remove = useMutation({
    mutationFn: () => deleteProject(project.id),
    onSuccess: invalidate,
  })

  const status = session.data?.status ?? project.active_session?.status ?? null
  const busy = status !== null && IN_FLIGHT.includes(status)
  const ready = status === 'ready'
  const failed = status === 'failed'
  const running = busy || ready
  const mutationError = start.error ?? stop.error ?? remove.error

  return (
    <Card>
      <CardHeader className="border-b p-4">
        <CardTitle className="flex flex-wrap items-center justify-between gap-2 text-sm">
          <span className="flex min-w-0 items-baseline gap-2">
            <span className="truncate">{project.name}</span>
            <span className="truncate font-normal text-muted-foreground text-xs">
              {project.repo_url.replace('https://github.com/', '')} · {project.default_branch}
            </span>
          </span>
          <StatusBadge status={status} />
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-3 p-4 text-sm">
        {busy ? (
          <p className="flex items-center gap-2 text-muted-foreground text-xs">
            <Spinner className="size-3.5" />
            {statusLine(status)}
          </p>
        ) : null}
        {failed && session.data?.status_detail ? (
          <p className="rounded-lg border border-destructive/32 bg-destructive/8 px-3 py-2 text-destructive-foreground text-xs">
            {session.data.status_detail}
          </p>
        ) : null}
        {mutationError ? (
          <p className="text-destructive-foreground text-xs">{errorMessage(mutationError)}</p>
        ) : null}
        <div className="flex flex-wrap items-center gap-2">
          {ready && sessionId ? (
            <TerminalDialog projectName={project.name} sessionId={sessionId} />
          ) : null}
          {running ? (
            <Button
              variant="outline"
              size="sm"
              disabled={stop.isPending || status === 'stopping'}
              onClick={() => stop.mutate()}
            >
              <SquareIcon /> Stop sandbox
            </Button>
          ) : (
            <Button size="sm" disabled={start.isPending} onClick={() => start.mutate()}>
              <ZapIcon /> Start sandbox
            </Button>
          )}
          <DeleteProjectDialog
            projectName={project.name}
            disabled={running || remove.isPending}
            onConfirm={() => remove.mutate()}
          />
        </div>
      </CardContent>
    </Card>
  )
}

function statusLine(status: string | null): string {
  switch (status) {
    case 'pending':
      return 'Queued…'
    case 'provisioning':
      return 'Creating the sandbox…'
    case 'cloning':
      return 'Cloning the repository…'
    case 'setting_up':
      return 'Running the setup command…'
    case 'stopping':
      return 'Stopping the sandbox…'
    default:
      return 'Working…'
  }
}

function StatusBadge({ status }: { status: string | null }) {
  if (status === null || status === 'stopped') {
    return <Badge variant="outline">idle</Badge>
  }
  if (status === 'ready') {
    return (
      <Badge variant="success" className="gap-1.5">
        <span aria-hidden="true" className="size-1.5 rounded-full bg-success" />
        ready
      </Badge>
    )
  }
  if (status === 'failed') {
    return <Badge variant="error">failed</Badge>
  }
  return <Badge variant="info">{status.replace('_', ' ')}</Badge>
}

function TerminalDialog({
  projectName,
  sessionId,
}: {
  projectName: string
  sessionId: string
}) {
  const [open, setOpen] = useState(false)
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger
        render={
          <Button size="sm">
            <TerminalIcon /> Open terminal
          </Button>
        }
      />
      <DialogPopup className="flex h-[80vh] max-h-[720px] w-[92vw] max-w-4xl flex-col overflow-hidden">
        <DialogHeader className="border-b">
          <DialogTitle className="flex items-baseline gap-2 font-normal text-sm">
            <span aria-hidden="true" className="font-bold text-(--prompt)">
              ❯
            </span>
            <span className="text-muted-foreground">{projectName}</span>
            <span className="-ms-1">/workspace/app</span>
          </DialogTitle>
        </DialogHeader>
        <div className="min-h-0 flex-1 bg-[#0d0f17] p-2">
          {/* Remount per open so each session gets a fresh socket. */}
          {open ? <SandboxTerminal key={sessionId} sessionId={sessionId} /> : null}
        </div>
      </DialogPopup>
    </Dialog>
  )
}

function DeleteProjectDialog({
  projectName,
  disabled,
  onConfirm,
}: {
  projectName: string
  disabled: boolean
  onConfirm: () => void
}) {
  return (
    <AlertDialog>
      <AlertDialogTrigger
        render={
          <Button variant="ghost" size="sm" disabled={disabled}>
            <Trash2Icon /> Delete
          </Button>
        }
      />
      <AlertDialogPopup>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete {projectName}?</AlertDialogTitle>
          <AlertDialogDescription>
            This removes the project from Termy Cloud. The Git repository is untouched.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogClose render={<Button variant="outline">Cancel</Button>} />
          <AlertDialogClose
            render={
              <Button variant="destructive" onClick={onConfirm}>
                Delete project
              </Button>
            }
          />
        </AlertDialogFooter>
      </AlertDialogPopup>
    </AlertDialog>
  )
}

function NewProjectDialog() {
  const queryClient = useQueryClient()
  const [open, setOpen] = useState(false)
  const create = useMutation({
    mutationFn: createProject,
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: projectsQuery.queryKey })
      setOpen(false)
    },
  })

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const data = new FormData(event.currentTarget)
    create.mutate({
      name: String(data.get('name') ?? '').trim(),
      repo_url: String(data.get('repo_url') ?? '').trim(),
      default_branch: String(data.get('default_branch') ?? '').trim() || 'main',
      setup_command: String(data.get('setup_command') ?? '').trim() || null,
    })
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next)
        if (!next) {
          create.reset()
        }
      }}
    >
      <DialogTrigger
        render={
          <Button size="sm">
            <PlusIcon /> New project
          </Button>
        }
      />
      <DialogPopup>
        <form onSubmit={submit}>
          <DialogHeader>
            <DialogTitle>New project</DialogTitle>
            <DialogDescription>
              Point Termy at a public GitHub repository. Creating a project provisions a
              dedicated Railway project for its sandboxes.
            </DialogDescription>
          </DialogHeader>
          <DialogPanel className="space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="project-name">Name</Label>
              <Input id="project-name" name="name" required placeholder="my-app" />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="project-repo">Repository</Label>
              <Input
                id="project-repo"
                name="repo_url"
                type="url"
                required
                placeholder="https://github.com/you/my-app"
              />
              <p className="text-muted-foreground text-xs">
                Public GitHub repositories only, for now.
              </p>
            </div>
            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-1.5">
                <Label htmlFor="project-branch">Branch</Label>
                <Input id="project-branch" name="default_branch" placeholder="main" />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="project-setup">Setup command</Label>
                <Input id="project-setup" name="setup_command" placeholder="npm install" />
              </div>
            </div>
            {create.isError ? (
              <p className="text-destructive-foreground text-xs">
                {errorMessage(create.error)}
              </p>
            ) : null}
          </DialogPanel>
          <DialogFooter>
            <Button type="submit" disabled={create.isPending}>
              {create.isPending ? <Spinner className="size-4" /> : null}
              Create project
            </Button>
          </DialogFooter>
        </form>
      </DialogPopup>
    </Dialog>
  )
}
