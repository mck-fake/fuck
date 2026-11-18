// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, test } from 'bun:test'
import { formValuesFor, isLockoutError } from './InstanceIpAccessDefaultCard'

describe('InstanceIpAccessDefaultCard helpers', () => {
  test('an unconfigured default starts the form Off with no entries', () => {
    expect(formValuesFor(undefined)).toEqual({ mode: 'disabled', cidrs: [] })
  })

  test('a stored default fills the form, blank notes as empty strings', () => {
    expect(
      formValuesFor({
        mode: 'allow',
        cidrs: [{ cidr: '10.0.0.0/8' }, { cidr: '192.0.2.0/24', description: 'Office' }],
      })
    ).toEqual({
      mode: 'allow',
      cidrs: [
        { cidr: '10.0.0.0/8', description: '' },
        { cidr: '192.0.2.0/24', description: 'Office' },
      ],
    })
  })

  test('only the lock-out 409 offers "Save anyway"', () => {
    expect(isLockoutError({ status: 409, title: 'Policy Would Lock You Out' })).toBe(true)
    expect(isLockoutError({ status: 409, title: 'IP Access Policy Changed' })).toBe(false)
    expect(isLockoutError(null)).toBe(false)
  })
})
