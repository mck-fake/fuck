// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

export type ApprovalState = 'none' | 'pending' | 'approved' | 'rejected'

/** What the deployment page shows for a deployment's approval request. */
export function approvalState(
  approval: { status: string } | null | undefined
): ApprovalState {
  switch (approval?.status) {
    case 'pending':
    case 'approved':
    case 'rejected':
      return approval.status
    default:
      return 'none'
  }
}

/** One line describing a decided request. */
export function decisionSummary(approval: {
  status: string
  comment?: string | null
}): string {
  const verb = approval.status === 'approved' ? 'Approved' : 'Rejected'
  return approval.comment ? `${verb}: “${approval.comment}”` : verb
}
