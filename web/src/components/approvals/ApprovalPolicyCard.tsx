// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import {
  getApprovalPolicyOptions,
  getApprovalPolicyQueryKey,
  setApprovalPolicyMutation,
} from '@/api/client/@tanstack/react-query.gen'
import { SettingsSection } from '@/components/ui/settings-section'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { ErrorAlert } from '@/components/utils/ErrorAlert'
import { problemDetail } from '@/lib/api-problem'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ShieldCheck } from 'lucide-react'
import { toast } from 'sonner'

/** The per-environment switch that holds deployments for approval. */
export function ApprovalPolicyCard({
  projectId,
  environmentId,
}: {
  projectId: number
  environmentId: number
}) {
  const queryClient = useQueryClient()
  const path = { project_id: projectId, environment_id: environmentId }
  const policyQuery = useQuery(getApprovalPolicyOptions({ path }))
  const save = useMutation({
    ...setApprovalPolicyMutation(),
    onSuccess: (saved) => {
      queryClient.setQueryData(getApprovalPolicyQueryKey({ path }), saved)
      toast.success(
        saved.required
          ? 'Deployments here now wait for approval'
          : 'Deployments here run without approval'
      )
    },
    onError: (error) =>
      toast.error(problemDetail(error, 'Could not change the approval setting')),
  })

  return (
    <SettingsSection
      id="deployment-approvals"
      title="Deployment approvals"
      description="Hold every deployment to this environment until someone with deployment access approves it"
      icon={ShieldCheck}
    >
      {policyQuery.isLoading ? (
        <Skeleton className="h-6 w-48" />
      ) : policyQuery.isError ? (
        <ErrorAlert
          title="Could not load the approval setting"
          description={problemDetail(policyQuery.error, 'Try again in a moment.')}
          retry={() => void policyQuery.refetch()}
        />
      ) : (
        <label className="flex items-center justify-between gap-4">
          <span className="text-sm">
            {policyQuery.data?.required
              ? 'Each deployment waits for approval on its page.'
              : 'Deployments start right away.'}
          </span>
          <Switch
            checked={policyQuery.data?.required ?? false}
            disabled={save.isPending}
            onCheckedChange={(required) =>
              save.mutate({ path, body: { required } })
            }
            aria-label="Require approval before each deployment"
          />
        </label>
      )}
    </SettingsSection>
  )
}
