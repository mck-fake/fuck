// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

/**
 * The instance default IP list: every project and environment without a list
 * of its own follows it. A project opts out by saving its own list, or Off.
 */
import {
  deleteInstanceIpAccessDefaultMutation,
  getInstanceIpAccessDefaultOptions,
  getInstanceIpAccessDefaultQueryKey,
  putInstanceIpAccessDefaultMutation,
} from '@/api/client/@tanstack/react-query.gen'
import type { InstanceIpAccessDefaultDetail } from '@/api/client/types.gen'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { ConfirmDialog } from '@/components/ui/confirm-dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { Skeleton } from '@/components/ui/skeleton'
import { ErrorAlert } from '@/components/utils/ErrorAlert'
import { problemDetail } from '@/lib/api-problem'
import {
  MAX_CIDRS,
  entriesToSave,
  hostCidr,
  ipAccessFormSchema,
  modeLabel,
  showsEntries,
  type IpAccessFormValues,
  type IpAccessMode,
} from '@/components/project/settings/ip-access'
import { zodResolver } from '@hookform/resolvers/zod'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, Trash2, UserRound } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Controller, useFieldArray, useForm, useWatch } from 'react-hook-form'
import { toast } from 'sonner'

const MODES: { value: IpAccessMode; hint: string }[] = [
  { value: 'disabled', hint: 'Projects without a list of their own are open.' },
  {
    value: 'allow',
    hint: 'Projects without a list of their own answer 403 to everyone else.',
  },
  {
    value: 'deny',
    hint: 'Listed addresses get a 403 on every project without a list of its own.',
  },
]

/** The 409 for "this save would lock you out", not a concurrent update. */
export function isLockoutError(error: unknown): boolean {
  if (typeof error !== 'object' || error === null) return false
  const problem = error as { status?: unknown; title?: unknown }
  return problem.status === 409 && problem.title === 'Policy Would Lock You Out'
}

/** Form values for what the server returned. */
export function formValuesFor(
  policy: Pick<InstanceIpAccessDefaultDetail, 'mode' | 'cidrs'> | undefined
): IpAccessFormValues {
  return {
    mode: policy?.mode ?? 'disabled',
    cidrs: (policy?.cidrs ?? []).map((entry) => ({
      cidr: entry.cidr,
      description: entry.description ?? '',
    })),
  }
}

export function InstanceIpAccessDefaultCard() {
  const queryClient = useQueryClient()
  const queryKey = getInstanceIpAccessDefaultQueryKey()
  const policyQuery = useQuery(getInstanceIpAccessDefaultOptions())
  const [lockout, setLockout] = useState<IpAccessFormValues | null>(null)
  const [confirmRemove, setConfirmRemove] = useState(false)

  const form = useForm<IpAccessFormValues>({
    resolver: zodResolver(ipAccessFormSchema),
    defaultValues: formValuesFor(undefined),
  })
  const { fields, append, remove } = useFieldArray({
    control: form.control,
    name: 'cidrs',
  })
  const mode = useWatch({ control: form.control, name: 'mode' }) as IpAccessMode

  useEffect(() => {
    if (policyQuery.data) form.reset(formValuesFor(policyQuery.data))
  }, [policyQuery.data, form])

  const save = useMutation({
    ...putInstanceIpAccessDefaultMutation(),
    onSuccess: (saved) => {
      queryClient.setQueryData(queryKey, saved)
      setLockout(null)
      toast.success('Default IP list saved')
      for (const warning of saved.warnings ?? []) toast.warning(warning)
    },
    onError: (error) => {
      if (isLockoutError(error)) {
        setLockout(form.getValues())
        return
      }
      toast.error(problemDetail(error, 'Could not save the default IP list'))
    },
  })
  const removeDefault = useMutation({
    ...deleteInstanceIpAccessDefaultMutation(),
    onSuccess: async () => {
      setConfirmRemove(false)
      toast.success('Default IP list removed')
      await queryClient.invalidateQueries({ queryKey })
    },
    onError: (error) => {
      setConfirmRemove(false)
      toast.error(problemDetail(error, 'Could not remove the default IP list'))
    },
  })

  const submit = (values: IpAccessFormValues, acknowledgeLockout: boolean) => {
    const saveMode = values.mode as IpAccessMode
    save.mutate({
      body: {
        mode: saveMode,
        cidrs: entriesToSave(saveMode, values.cidrs).map((entry) => ({
          cidr: entry.cidr.trim(),
          description: entry.description?.trim() || null,
        })),
        acknowledge_lockout: acknowledgeLockout,
      },
    })
  }

  if (policyQuery.isLoading) {
    return <Skeleton className="h-40 w-full rounded-md" />
  }
  if (policyQuery.isError) {
    return (
      <ErrorAlert
        title="Could not load the default IP list"
        description={problemDetail(policyQuery.error, 'Try again in a moment.')}
        retry={() => void policyQuery.refetch()}
      />
    )
  }

  const yourIp = policyQuery.data?.your_ip ?? null
  const yourIpListed =
    yourIp !== null &&
    form
      .getValues('cidrs')
      .some((entry) => entry.cidr.trim() === hostCidr(yourIp))
  const cidrsError = form.formState.errors.cidrs?.message

  return (
    // Not a <form>: the settings page around it already is one.
    <div className="space-y-4">
      <Controller
        control={form.control}
        name="mode"
        render={({ field }) => (
          <RadioGroup
            value={field.value}
            onValueChange={field.onChange}
            className="grid grid-cols-1 gap-2 md:grid-cols-3"
          >
            {MODES.map((option) => (
              <Label
                key={option.value}
                htmlFor={`instance-ip-default-${option.value}`}
                className="flex cursor-pointer items-start gap-3 rounded-md border p-3 font-normal has-[[data-state=checked]]:border-primary"
              >
                <RadioGroupItem
                  id={`instance-ip-default-${option.value}`}
                  value={option.value}
                  className="mt-0.5"
                />
                <span className="space-y-1">
                  <span className="block text-sm font-medium">
                    {modeLabel(option.value)}
                  </span>
                  <span className="block text-xs text-muted-foreground">
                    {option.hint}
                  </span>
                </span>
              </Label>
            ))}
          </RadioGroup>
        )}
      />

      {showsEntries(mode) && (
        <div className="space-y-2">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Label>
              Addresses and ranges{' '}
              <span className="font-normal text-muted-foreground">
                ({fields.length} / {MAX_CIDRS})
              </span>
            </Label>
            <div className="flex flex-wrap gap-2">
              {yourIp && !yourIpListed && (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={fields.length >= MAX_CIDRS}
                  onClick={() =>
                    append({
                      cidr: hostCidr(yourIp),
                      description: 'My current address',
                    })
                  }
                >
                  <UserRound className="mr-2 h-4 w-4" />
                  Add my IP ({yourIp})
                </Button>
              )}
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={fields.length >= MAX_CIDRS}
                onClick={() => append({ cidr: '', description: '' })}
              >
                <Plus className="mr-2 h-4 w-4" />
                Add range
              </Button>
            </div>
          </div>
          {fields.length === 0 ? (
            <p className="rounded-md border border-dashed p-4 text-sm text-muted-foreground">
              Enter a single address (203.0.113.9) or a range (203.0.113.0/24,
              2001:db8::/32).
            </p>
          ) : (
            <ul className="space-y-2">
              {fields.map((field, index) => {
                const rowErrors = form.formState.errors.cidrs?.[index]
                return (
                  <li
                    key={field.id}
                    className="grid grid-cols-1 gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]"
                  >
                    <div className="space-y-1">
                      <Input
                        aria-label={`Address or range ${index + 1}`}
                        aria-invalid={rowErrors?.cidr ? true : undefined}
                        placeholder="203.0.113.0/24"
                        className="font-mono"
                        {...form.register(`cidrs.${index}.cidr` as const)}
                      />
                      {rowErrors?.cidr?.message && (
                        <p className="text-sm text-destructive">
                          {rowErrors.cidr.message}
                        </p>
                      )}
                    </div>
                    <Input
                      aria-label={`Note for range ${index + 1}`}
                      placeholder="Note (optional)"
                      {...form.register(`cidrs.${index}.description` as const)}
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      aria-label={`Remove range ${index + 1}`}
                      onClick={() => remove(index)}
                    >
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  </li>
                )
              })}
            </ul>
          )}
          {cidrsError && (
            <p className="text-sm text-destructive" role="alert">
              {cidrsError}
            </p>
          )}
        </div>
      )}

      <div className="flex flex-wrap justify-end gap-2">
        {policyQuery.data?.configured && (
          <Button
            type="button"
            variant="outline"
            disabled={removeDefault.isPending}
            onClick={() => setConfirmRemove(true)}
          >
            Remove default
          </Button>
        )}
        <Button
          type="button"
          disabled={save.isPending}
          onClick={form.handleSubmit((values) => submit(values, false))}
        >
          {save.isPending ? 'Saving…' : 'Save default'}
        </Button>
      </div>

      <AlertDialog
        open={lockout !== null}
        onOpenChange={(open) => {
          if (!open) setLockout(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>This would block your address</AlertDialogTitle>
            <AlertDialogDescription>
              {yourIp ? `${yourIp} ` : 'Your address '}
              would no longer reach projects without a list of their own. The
              console itself stays reachable.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (lockout) submit(lockout, true)
              }}
            >
              Save anyway
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <ConfirmDialog
        open={confirmRemove}
        onOpenChange={setConfirmRemove}
        title="Remove the default IP list?"
        description="Projects without a list of their own become reachable from any address."
        confirmLabel="Remove default"
        destructive
        onConfirm={() => removeDefault.mutate({})}
      />
    </div>
  )
}
