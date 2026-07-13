import { useMutation, useQueryClient, useSuspenseQuery } from '@tanstack/react-query'
import {
  Link,
  Outlet,
  createFileRoute,
  redirect,
  useNavigate,
  useRouterState,
} from '@tanstack/react-router'
import {
  FolderGit2Icon,
  LayoutDashboardIcon,
  LogOutIcon,
  MonitorSmartphoneIcon,
  SettingsIcon,
} from 'lucide-react'
import { signOut } from '@/api'
import { currentUserQuery } from '@/query'
import { Button } from '@/components/ui/button'
import { Separator } from '@/components/ui/separator'
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarInset,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
  SidebarTrigger,
} from '@/components/ui/sidebar'

export const Route = createFileRoute('/dashboard')({
  beforeLoad: async ({ context }) => {
    const user = await context.queryClient.ensureQueryData(currentUserQuery)
    if (!user) {
      throw redirect({ to: '/' })
    }
  },
  component: DashboardLayout,
})

const NAV_ITEMS = [
  { title: 'Overview', icon: LayoutDashboardIcon, to: '/dashboard', soon: false },
  { title: 'Projects', icon: FolderGit2Icon, to: '/dashboard/projects', soon: false },
  { title: 'Devices', icon: MonitorSmartphoneIcon, to: null, soon: true },
  { title: 'Settings', icon: SettingsIcon, to: null, soon: true },
] as const

function DashboardLayout() {
  const user = useSuspenseQuery(currentUserQuery)
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const pathname = useRouterState({ select: (state) => state.location.pathname })
  const logout = useMutation({
    mutationFn: signOut,
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: currentUserQuery.queryKey })
      await navigate({ to: '/' })
    },
  })

  if (!user.data) {
    return null
  }

  const displayName = user.data.name?.trim() || user.data.email.split('@')[0]
  const section = pathname.startsWith('/dashboard/projects') ? 'projects' : 'overview'

  return (
    <SidebarProvider>
      <Sidebar>
        <SidebarHeader className="border-sidebar-border border-b">
          <a href="/" className="flex items-center gap-2.5 px-2 py-2">
            <span className="brand-prompt" aria-hidden="true">
              &gt;_
            </span>
            <span className="font-semibold text-sm tracking-tight">Termy Cloud</span>
          </a>
        </SidebarHeader>
        <SidebarContent>
          <SidebarGroup>
            <SidebarGroupLabel>Account</SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu>
                {NAV_ITEMS.map((item) => (
                  <SidebarMenuItem key={item.title}>
                    {item.to ? (
                      <SidebarMenuButton
                        isActive={
                          item.to === '/dashboard'
                            ? pathname === '/dashboard'
                            : pathname.startsWith(item.to)
                        }
                        tooltip={item.title}
                        render={<Link to={item.to} />}
                      >
                        <item.icon />
                        <span>{item.title}</span>
                      </SidebarMenuButton>
                    ) : (
                      <SidebarMenuButton tooltip={item.title} aria-disabled>
                        <item.icon />
                        <span>{item.title}</span>
                      </SidebarMenuButton>
                    )}
                    {item.soon ? (
                      <SidebarMenuBadge className="text-sidebar-foreground/56">
                        soon
                      </SidebarMenuBadge>
                    ) : null}
                  </SidebarMenuItem>
                ))}
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
        </SidebarContent>
        <SidebarFooter className="border-sidebar-border border-t">
          <div className="flex items-center gap-2.5 px-2 py-1.5">
            <span
              aria-hidden="true"
              className="grid size-7 shrink-0 place-items-center rounded-md bg-primary/16 font-semibold text-primary text-xs uppercase"
            >
              {displayName.slice(0, 1)}
            </span>
            <div className="min-w-0 flex-1">
              <p className="truncate font-medium text-sm leading-tight">{displayName}</p>
              <p className="truncate text-muted-foreground text-xs leading-tight">
                {user.data.email}
              </p>
            </div>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="Sign out"
              disabled={logout.isPending}
              onClick={() => logout.mutate()}
            >
              <LogOutIcon />
            </Button>
          </div>
        </SidebarFooter>
      </Sidebar>
      <SidebarInset className="bg-transparent">
        <header className="flex h-14 shrink-0 items-center gap-2 border-b bg-background/64 px-4 backdrop-blur-sm">
          <SidebarTrigger className="-ms-1" />
          <Separator className="me-1 h-4" orientation="vertical" />
          <p className="flex items-baseline gap-2 text-sm">
            <span aria-hidden="true" className="font-bold text-(--prompt)">
              ❯
            </span>
            <span className="text-muted-foreground">~/</span>
            <span className="-ms-2 font-medium">{section}</span>
          </p>
        </header>
        <Outlet />
      </SidebarInset>
    </SidebarProvider>
  )
}
