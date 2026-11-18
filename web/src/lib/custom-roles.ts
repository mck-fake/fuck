// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { z } from 'zod'

export const MAX_ROLE_NAME_LENGTH = 64
export const MAX_ROLE_DESCRIPTION_LENGTH = 500

export const customRoleFormSchema = z.object({
  name: z
    .string()
    .trim()
    .min(1, 'Give the role a name')
    .max(MAX_ROLE_NAME_LENGTH, `At most ${MAX_ROLE_NAME_LENGTH} characters`),
  description: z
    .string()
    .trim()
    .max(
      MAX_ROLE_DESCRIPTION_LENGTH,
      `At most ${MAX_ROLE_DESCRIPTION_LENGTH} characters`
    ),
  permissions: z.array(z.string()).min(1, 'Pick at least one permission'),
})

export type CustomRoleFormValues = z.infer<typeof customRoleFormSchema>

export interface PermissionGroup {
  /** The part before `:`, e.g. `deployments`. */
  domain: string
  permissions: string[]
}

/** Group `domain:action` strings by domain, both levels sorted. */
export function groupPermissions(permissions: readonly string[]): PermissionGroup[] {
  const groups = new Map<string, string[]>()
  for (const permission of permissions) {
    const domain = permission.split(':')[0] ?? permission
    groups.set(domain, [...(groups.get(domain) ?? []), permission])
  }
  return [...groups.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([domain, list]) => ({ domain, permissions: [...list].sort() }))
}

/** `deployments` → `Deployments`, `error_tracking` → `Error tracking`. */
export function domainLabel(domain: string): string {
  const words = domain.replace(/[_-]+/g, ' ')
  return words.charAt(0).toUpperCase() + words.slice(1)
}

/** The action half of a permission string: `deployments:create` → `create`. */
export function actionLabel(permission: string): string {
  return permission.split(':')[1] ?? permission
}

/** Add or remove one permission, keeping the list sorted and unique. */
export function togglePermission(
  selected: readonly string[],
  permission: string,
  on: boolean
): string[] {
  const set = new Set(selected)
  if (on) set.add(permission)
  else set.delete(permission)
  return [...set].sort()
}

/** Select or clear a whole domain group. */
export function toggleGroup(
  selected: readonly string[],
  group: PermissionGroup,
  on: boolean
): string[] {
  return group.permissions.reduce(
    (acc, permission) => togglePermission(acc, permission, on),
    [...selected]
  )
}
