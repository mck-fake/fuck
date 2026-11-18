// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, test } from 'bun:test'
import { approvalState, decisionSummary } from './approvals'

describe('approvalState', () => {
  test('a deployment without a request shows nothing', () => {
    expect(approvalState(null)).toBe('none')
    expect(approvalState(undefined)).toBe('none')
  })

  test('known statuses pass through and unknown ones show nothing', () => {
    expect(approvalState({ status: 'pending' })).toBe('pending')
    expect(approvalState({ status: 'approved' })).toBe('approved')
    expect(approvalState({ status: 'rejected' })).toBe('rejected')
    expect(approvalState({ status: 'later' })).toBe('none')
  })
})

describe('decisionSummary', () => {
  test('includes the comment when there is one', () => {
    expect(decisionSummary({ status: 'approved' })).toBe('Approved')
    expect(decisionSummary({ status: 'rejected', comment: 'wrong branch' })).toBe(
      'Rejected: “wrong branch”'
    )
  })
})
