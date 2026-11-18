// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

/**
 * Shown on a deployment held for approval: Approve runs it, Reject cancels
 * it. After a decision it says which one was taken.
 */
import {
  decideDeploymentApprovalMutation,
  getDeploymentApprovalOptions,
  getDeploymentApprovalQueryKey,
  getDeploymentQueryKey,
} from '@/api/client/@tanstack/react-query.gen'
import { Button } from '@/components/ui/button'
import { ConfirmDialog } from '@/components/ui/confirm-dialog'
import { Input } from '@/components/ui/input'
import { problemDetail } from '@/lib/api-problem'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Check, Hourglass, X } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { approvalState, decisionSummary } from './approvals'

export function DeploymentApprovalBanner({
  projectId,
  deploymentId,
}: {
  projectId: number
  deploymentId: number
}) {
  const queryClient = useQueryClient()
  const path = { project_id: projectId, deployment_id: deploymentId }
  const approvalQuery = useQuery({
    ...getDeploymentApprovalOptions({ path }),
    // Poll while waiting so a decision taken elsewhere shows up.
    refetchInterval: (query) =>
      query.state.data?.approval?.status === 'pending' ? 10_000 : false,
  })
  const [comment, setComment] = useState('')
  const [confirmReject, setConfirmReject] = useState(false)

  const decide = useMutation({
    ...decideDeploymentApprovalMutation(),
    onSuccess: async (decided) => {
      toast.success(
        decided.status === 'approved'
          ? 'Approved: the deployment is starting'
          : 'Rejected: the deployment was cancelled'
      )
      setConfirmReject(false)
      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: getDeploymentApprovalQueryKey({ path }),
        }),
        queryClient.invalidateQueries({
          queryKey: getDeploymentQueryKey({ path }),
        }),
      ])
    },
    onError: (error) => {
      setConfirmReject(false)
      toast.error(problemDetail(error, 'Could not record the decision'))
    },
  })

  const approval = approvalQuery.data?.approval
  const state = approvalState(approval)
  if (state === 'none' || !approval) return null

  if (state !== 'pending') {
    return (
      <p className="text-sm text-muted-foreground">
        {decisionSummary(approval)}
      </p>
    )
  }

  const submit = (decision: 'approve' | 'reject') =>
    decide.mutate({
      path,
      body: { decision, comment: comment.trim() || null },
    })

  return (
    <div className="flex flex-col gap-3 rounded-lg border border-amber-500/40 bg-amber-500/5 p-4 sm:flex-row sm:items-center">
      <Hourglass className="hidden h-4 w-4 shrink-0 text-amber-600 sm:block" />
      <div className="min-w-0 flex-1 space-y-2">
        <p className="text-sm font-medium">Waiting for approval</p>
        <Input
          aria-label="Comment (optional)"
          placeholder="Comment (optional)"
          maxLength={500}
          value={comment}
          onChange={(event) => setComment(event.target.value)}
        />
      </div>
      <div className="flex shrink-0 gap-2">
        <Button
          variant="outline"
          disabled={decide.isPending}
          onClick={() => setConfirmReject(true)}
        >
          <X className="mr-2 h-4 w-4" />
          Reject
        </Button>
        <Button disabled={decide.isPending} onClick={() => submit('approve')}>
          <Check className="mr-2 h-4 w-4" />
          Approve
        </Button>
      </div>
      <ConfirmDialog
        open={confirmReject}
        onOpenChange={setConfirmReject}
        title="Reject this deployment?"
        description="It is cancelled and does not run. Deploy again to retry."
        confirmLabel="Reject"
        destructive
        onConfirm={() => submit('reject')}
      />
    </div>
  )
}
