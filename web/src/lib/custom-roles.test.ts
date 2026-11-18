// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, test } from 'bun:test'
import {
  actionLabel,
  customRoleFormSchema,
  domainLabel,
  groupPermissions,
  toggleGroup,
  togglePermission,
} from './custom-roles'

describe('custom roles helpers', () => {
  test('permissions are grouped by domain, both levels sorted', () => {
    expect(
      groupPermissions([
        'projects:read',
        'deployments:read',
        'deployments:create',
      ])
    ).toEqual([
      {
        domain: 'deployments',
        permissions: ['deployments:create', 'deployments:read'],
      },
      { domain: 'projects', permissions: ['projects:read'] },
    ])
  })

  test('labels read as words', () => {
    expect(domainLabel('error_tracking')).toBe('Error tracking')
    expect(actionLabel('deployments:create')).toBe('create')
  })

  test('toggling keeps the selection sorted and unique', () => {
    expect(togglePermission(['b:x'], 'a:x', true)).toEqual(['a:x', 'b:x'])
    expect(togglePermission(['a:x', 'b:x'], 'a:x', true)).toEqual([
      'a:x',
      'b:x',
    ])
    expect(togglePermission(['a:x', 'b:x'], 'a:x', false)).toEqual(['b:x'])
    const group = { domain: 'a', permissions: ['a:read', 'a:write'] }
    expect(toggleGroup(['b:x'], group, true)).toEqual([
      'a:read',
      'a:write',
      'b:x',
    ])
    expect(toggleGroup(['a:read', 'b:x'], group, false)).toEqual(['b:x'])
  })

  test('the form needs a name and at least one permission', () => {
    expect(
      customRoleFormSchema.safeParse({
        name: '  ',
        description: '',
        permissions: ['projects:read'],
      }).success
    ).toBe(false)
    expect(
      customRoleFormSchema.safeParse({
        name: 'Release manager',
        description: '',
        permissions: [],
      }).success
    ).toBe(false)
    expect(
      customRoleFormSchema.safeParse({
        name: 'Release manager',
        description: '',
        permissions: ['deployments:create'],
      }).success
    ).toBe(true)
  })
})
