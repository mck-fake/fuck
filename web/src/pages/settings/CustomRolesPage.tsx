// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

/**
 * Custom roles: named sets of project-scoped permissions a team membership
 * can hold instead of its fixed role. A member never gets more than their
 * team's grant on a project allows, whatever the role says.
 */
import {
  createCustomRoleMutation,
  deleteCustomRoleMutation,
  listAssignablePermissionsOptions,
  listCustomRolesOptions,
  listCustomRolesQueryKey,
  updateCustomRoleMutation,
} from '@/api/client/@tanstack/react-query.gen'
import type { CustomRoleResponse } from '@/api/client/types.gen'
import { PageHeader } from '@/components/layout/PageContainer'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { ConfirmDeleteDialog } from '@/components/ui/confirm-delete-dialog'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { ReadFailure } from '@/components/ui/read-failure'
import { ResponsivePagination } from '@/components/ui/responsive-pagination'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { Textarea } from '@/components/ui/textarea'
import { useBreadcrumbs } from '@/contexts/BreadcrumbContext'
import { usePageTitle } from '@/hooks/usePageTitle'
import { problemDetail } from '@/lib/api-problem'
import {
  actionLabel,
  customRoleFormSchema,
  domainLabel,
  groupPermissions,
  toggleGroup,
  togglePermission,
  type CustomRoleFormValues,
} from '@/lib/custom-roles'
import { zodResolver } from '@hookform/resolvers/zod'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Pencil, Plus, ShieldCheck, Trash2 } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { Controller, useForm } from 'react-hook-form'
import { toast } from 'sonner'

const PAGE_SIZE = 20
const EMPTY_FORM: CustomRoleFormValues = {
  name: '',
  description: '',
  permissions: [],
}

/** Create (role = null) or edit one role. Mounted always; `open` controls it. */
function RoleDialog({
  open,
  role,
  onOpenChange,
}: {
  open: boolean
  role: CustomRoleResponse | null
  onOpenChange: (open: boolean) => void
}) {
  const queryClient = useQueryClient()
  const assignable = useQuery({
    ...listAssignablePermissionsOptions(),
    enabled: open,
  })
  const groups = useMemo(
    () => groupPermissions(assignable.data?.permissions ?? []),
    [assignable.data]
  )
  const form = useForm<CustomRoleFormValues>({
    resolver: zodResolver(customRoleFormSchema),
    defaultValues: EMPTY_FORM,
  })

  useEffect(() => {
    if (!open) return
    form.reset(
      role
        ? {
            name: role.name,
            description: role.description ?? '',
            permissions: role.permissions,
          }
        : EMPTY_FORM
    )
  }, [open, role, form])

  const done = async (message: string) => {
    toast.success(message)
    onOpenChange(false)
    await queryClient.invalidateQueries({ queryKey: listCustomRolesQueryKey() })
  }
  const create = useMutation({
    ...createCustomRoleMutation(),
    onSuccess: () => done('Role created'),
    onError: (error) =>
      toast.error(problemDetail(error, 'Could not create the role')),
  })
  const update = useMutation({
    ...updateCustomRoleMutation(),
    onSuccess: () => done('Role saved'),
    onError: (error) =>
      toast.error(problemDetail(error, 'Could not save the role')),
  })
  const pending = create.isPending || update.isPending

  const submit = form.handleSubmit((values) => {
    const body = {
      name: values.name,
      description: values.description || null,
      permissions: values.permissions,
    }
    if (role) update.mutate({ path: { role_id: role.id }, body })
    else create.mutate({ body })
  })

  const errors = form.formState.errors

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{role ? `Edit ${role.name}` : 'New role'}</DialogTitle>
        </DialogHeader>
        <form onSubmit={submit} className="space-y-4" noValidate>
          <div className="space-y-1">
            <Label htmlFor="role-name">Name</Label>
            <Input
              id="role-name"
              placeholder="Release manager"
              aria-invalid={errors.name ? true : undefined}
              {...form.register('name')}
            />
            {errors.name?.message && (
              <p className="text-sm text-destructive">{errors.name.message}</p>
            )}
          </div>
          <div className="space-y-1">
            <Label htmlFor="role-description">Description</Label>
            <Textarea
              id="role-description"
              rows={2}
              {...form.register('description')}
            />
            {errors.description?.message && (
              <p className="text-sm text-destructive">
                {errors.description.message}
              </p>
            )}
          </div>
          <Controller
            control={form.control}
            name="permissions"
            render={({ field }) => (
              <fieldset className="space-y-3">
                <legend className="text-sm font-medium">Permissions</legend>
                {assignable.isLoading ? (
                  <Skeleton className="h-40 w-full" />
                ) : (
                  <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                    {groups.map((group) => {
                      const selected = group.permissions.filter((p) =>
                        field.value.includes(p)
                      ).length
                      return (
                        <div key={group.domain} className="rounded-md border p-3">
                          <label className="flex items-center gap-2 text-sm font-medium">
                            <Checkbox
                              checked={
                                selected === group.permissions.length
                                  ? true
                                  : selected > 0
                                    ? 'indeterminate'
                                    : false
                              }
                              onCheckedChange={(checked) =>
                                field.onChange(
                                  toggleGroup(field.value, group, checked === true)
                                )
                              }
                            />
                            {domainLabel(group.domain)}
                          </label>
                          <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 pl-6">
                            {group.permissions.map((permission) => (
                              <label
                                key={permission}
                                className="flex items-center gap-1.5 text-sm text-muted-foreground"
                              >
                                <Checkbox
                                  checked={field.value.includes(permission)}
                                  onCheckedChange={(checked) =>
                                    field.onChange(
                                      togglePermission(
                                        field.value,
                                        permission,
                                        checked === true
                                      )
                                    )
                                  }
                                />
                                {actionLabel(permission)}
                              </label>
                            ))}
                          </div>
                        </div>
                      )
                    })}
                  </div>
                )}
                {errors.permissions?.message && (
                  <p className="text-sm text-destructive" role="alert">
                    {errors.permissions.message}
                  </p>
                )}
              </fieldset>
            )}
          />
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={pending}>
              {pending ? 'Saving…' : role ? 'Save role' : 'Create role'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

export function CustomRolesPage() {
  usePageTitle('Roles')
  const { setBreadcrumbs } = useBreadcrumbs()
  const queryClient = useQueryClient()
  const [page, setPage] = useState(1)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editing, setEditing] = useState<CustomRoleResponse | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<CustomRoleResponse | null>(
    null
  )

  useEffect(() => {
    setBreadcrumbs([
      { label: 'Settings', href: '/settings' },
      { label: 'Roles' },
    ])
  }, [setBreadcrumbs])

  const rolesQuery = useQuery(
    listCustomRolesOptions({ query: { page, page_size: PAGE_SIZE } })
  )
  const remove = useMutation({
    ...deleteCustomRoleMutation(),
    onSuccess: async () => {
      toast.success('Role deleted')
      setDeleteTarget(null)
      await queryClient.invalidateQueries({ queryKey: listCustomRolesQueryKey() })
    },
    onError: (error) => {
      setDeleteTarget(null)
      toast.error(problemDetail(error, 'Could not delete the role'))
    },
  })

  const openCreate = () => {
    setEditing(null)
    setDialogOpen(true)
  }
  const roles = rolesQuery.data?.roles ?? []
  const total = rolesQuery.data?.total ?? 0

  return (
    <div className="space-y-6">
      <PageHeader
        title="Roles"
        description="Custom roles for team members. A member never exceeds what their team is granted on a project."
        actions={
          roles.length > 0 ? (
            <Button onClick={openCreate}>
              <Plus className="h-4 w-4 sm:mr-2" />
              <span className="max-sm:sr-only">New role</span>
            </Button>
          ) : undefined
        }
      />

      {rolesQuery.isError ? (
        <ReadFailure
          resource="Roles"
          error={rolesQuery.error}
          onRetry={() => void rolesQuery.refetch()}
          retrying={rolesQuery.isFetching}
        />
      ) : rolesQuery.isLoading ? (
        <div className="space-y-2">
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-12 w-full" />
          ))}
        </div>
      ) : roles.length === 0 ? (
        <EmptyState
          size="compact"
          icon={ShieldCheck}
          title="No custom roles yet"
          description="For example, a Release manager who can deploy and read logs but not change domains. Assign it to a member from their team's page."
          action={
            <Button onClick={openCreate}>
              <Plus className="mr-2 h-4 w-4" />
              New role
            </Button>
          }
        />
      ) : (
        <>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead className="hidden md:table-cell">
                  Description
                </TableHead>
                <TableHead className="text-right">Permissions</TableHead>
                <TableHead className="w-24 text-right">Actions</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {roles.map((role) => (
                <TableRow key={role.id}>
                  <TableCell className="font-medium [overflow-wrap:anywhere]">
                    {role.name}
                  </TableCell>
                  <TableCell className="hidden text-muted-foreground md:table-cell">
                    {role.description}
                  </TableCell>
                  <TableCell className="text-right">
                    <Badge variant="secondary">{role.permissions.length}</Badge>
                  </TableCell>
                  <TableCell className="text-right">
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label={`Edit ${role.name}`}
                      onClick={() => {
                        setEditing(role)
                        setDialogOpen(true)
                      }}
                    >
                      <Pencil className="h-4 w-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label={`Delete ${role.name}`}
                      onClick={() => setDeleteTarget(role)}
                    >
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
          <ResponsivePagination
            page={page}
            pageSize={PAGE_SIZE}
            total={total}
            totalPages={Math.max(1, Math.ceil(total / PAGE_SIZE))}
            onPageChange={setPage}
          />
        </>
      )}

      <RoleDialog
        open={dialogOpen}
        role={editing}
        onOpenChange={setDialogOpen}
      />
      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(null)
        }}
        resourceType="role"
        resourceName={deleteTarget?.name ?? ''}
        consequences="A role still assigned to team members cannot be deleted; move them to another role first."
        isPending={remove.isPending}
        onConfirm={() => {
          if (deleteTarget) remove.mutate({ path: { role_id: deleteTarget.id } })
        }}
      />
    </div>
  )
}
