// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Version history of a project's edge rules (ADR-089 A1): what restoring a
// revision would change compared with the rules in force.

import { actionFromApi, groupFromApi, type ApiAction, type ApiGroup } from './api'
import { describeAction, describeExpression } from './expression'
import type { EdgeRuleDraft, RuleMode } from './types'

/** One rule as a revision stored it, in the API's format. */
export interface ApiRevisionRule {
  id: number
  environment_id?: number | null
  name: string
  enabled: boolean
  mode: RuleMode
  expression: ApiGroup
  action: ApiAction
}

export type RevisionRule = EdgeRuleDraft & { id: string }

export function revisionRuleFromApi(rule: ApiRevisionRule): RevisionRule {
  return {
    id: String(rule.id),
    name: rule.name,
    enabled: rule.enabled,
    mode: rule.mode,
    environmentId: rule.environment_id ?? null,
    expression: groupFromApi(rule.expression),
    action: actionFromApi(rule.action),
  }
}

export interface RestoreChange {
  kind: 'added' | 'removed' | 'changed' | 'moved'
  name: string
}

/** What a rule does, for comparing two versions of it. */
function behaviour(rule: EdgeRuleDraft): string {
  return [
    rule.name,
    rule.enabled,
    rule.mode,
    rule.environmentId ?? '',
    describeExpression(rule.expression),
    describeAction(rule.action),
  ].join('\u0000')
}

/**
 * What restoring `revision` would do to `current`: rules it brings back,
 * rules it removes, rules whose behaviour differs, and rules that only
 * change position. Empty when the revision matches the rules in force.
 */
export function restoreChanges(
  revision: readonly RevisionRule[],
  current: readonly RevisionRule[]
): RestoreChange[] {
  const now = new Map(current.map((rule, index) => [rule.id, { rule, index }]))
  const restored = new Set(revision.map((rule) => rule.id))
  const changes: RestoreChange[] = []
  revision.forEach((rule, index) => {
    const existing = now.get(rule.id)
    if (!existing) {
      changes.push({ kind: 'added', name: rule.name })
    } else if (behaviour(existing.rule) !== behaviour(rule)) {
      changes.push({ kind: 'changed', name: rule.name })
    } else if (existing.index !== index) {
      changes.push({ kind: 'moved', name: rule.name })
    }
  })
  for (const rule of current) {
    if (!restored.has(rule.id)) changes.push({ kind: 'removed', name: rule.name })
  }
  return changes
}

export function describeChange(change: RestoreChange): string {
  switch (change.kind) {
    case 'added':
      return `Brings back "${change.name}"`
    case 'removed':
      return `Removes "${change.name}"`
    case 'changed':
      return `Changes "${change.name}" back`
    case 'moved':
      return `Moves "${change.name}" back to its old position`
  }
}
