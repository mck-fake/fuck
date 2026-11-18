// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, test } from 'bun:test'
import {
  describeChange,
  restoreChanges,
  revisionRuleFromApi,
  type ApiRevisionRule,
} from './history'

function apiRule(id: number, path: string, extra: Partial<ApiRevisionRule> = {}): ApiRevisionRule {
  return {
    id,
    name: `Rule ${id}`,
    enabled: true,
    mode: 'enforce',
    environment_id: null,
    expression: {
      combinator: 'all',
      children: [{ kind: 'condition', id: `c${id}`, field: 'path', op: 'starts_with', values: [path] }],
    },
    action: { type: 'block', status: 403 },
    ...extra,
  }
}

const rule = (id: number, path = `/p${id}`, extra: Partial<ApiRevisionRule> = {}) =>
  revisionRuleFromApi(apiRule(id, path, extra))

describe('restoreChanges', () => {
  test('a revision equal to the rules in force changes nothing', () => {
    expect(restoreChanges([rule(1), rule(2)], [rule(1), rule(2)])).toEqual([])
  })

  test('reports rules brought back, removed, changed and moved', () => {
    const revision = [rule(2), rule(1, '/old'), rule(3)]
    const current = [rule(1, '/new'), rule(2), rule(4)]
    expect(restoreChanges(revision, current)).toEqual([
      { kind: 'moved', name: 'Rule 2' },
      { kind: 'changed', name: 'Rule 1' },
      { kind: 'added', name: 'Rule 3' },
      { kind: 'removed', name: 'Rule 4' },
    ])
  })

  test('a toggled rule or another mode counts as a change', () => {
    expect(restoreChanges([rule(1)], [rule(1, '/p1', { enabled: false })])).toEqual([
      { kind: 'changed', name: 'Rule 1' },
    ])
    expect(restoreChanges([rule(1)], [rule(1, '/p1', { mode: 'log_only' })])).toEqual([
      { kind: 'changed', name: 'Rule 1' },
    ])
  })

  test('changes read as sentences', () => {
    expect(describeChange({ kind: 'removed', name: 'Block /admin' })).toBe(
      'Removes "Block /admin"'
    )
  })
})
