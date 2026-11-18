// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { z } from 'zod'

/** Name and mark shown when the operator has not configured branding. */
export const DEFAULT_BRAND_NAME = 'Temps'
export const DEFAULT_BRAND_ICON = '/svg/temps-icon.svg'

export interface Branding {
  product_name?: string | null
  primary_color?: string | null
  logo?: string | null
  favicon?: string | null
}

const HEX_COLOR = /^#[0-9a-f]{6}$/i

export function brandName(branding?: Branding | null): string {
  return branding?.product_name?.trim() || DEFAULT_BRAND_NAME
}

export function brandIcon(branding?: Branding | null): string {
  return branding?.logo || DEFAULT_BRAND_ICON
}

export function isHexColor(value: string): boolean {
  return HEX_COLOR.test(value)
}

/**
 * Text colour readable on `hex` (WCAG relative luminance): near-black on
 * light accents, near-white on dark ones. The values match the theme's own
 * primary-foreground tokens. `null` for anything that is not `#rrggbb`.
 */
export function foregroundFor(hex: string): string | null {
  if (!isHexColor(hex)) return null
  const channel = (offset: number) => {
    const c = parseInt(hex.slice(offset, offset + 2), 16) / 255
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
  }
  const luminance =
    0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5)
  return luminance > 0.179 ? 'oklch(0.145 0 0)' : 'oklch(0.985 0 0)'
}

let currentBrandName = DEFAULT_BRAND_NAME

/** The product name page titles use; kept in sync by `BrandingEffects`. */
export function getBrandName(): string {
  return currentBrandName
}

export function setBrandName(name: string): void {
  currentBrandName = name
}

export function brandedTitle(title: string, name = currentBrandName): string {
  return title ? `${title} - ${name}` : name
}

/** Mirrors the server limits in `temps_config::app_settings::branding`. */
export const MAX_BRAND_NAME_CHARS = 40
export const MAX_LOGO_BYTES = 128 * 1024
export const MAX_FAVICON_BYTES = 32 * 1024
export const BRAND_IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/webp']

export const brandingFormSchema = z.object({
  product_name: z
    .string()
    .max(MAX_BRAND_NAME_CHARS, `At most ${MAX_BRAND_NAME_CHARS} characters`),
  primary_color: z
    .string()
    .refine((v) => v === '' || isHexColor(v), 'Use a #rrggbb colour'),
  logo: z.string(),
  favicon: z.string(),
})

export type BrandingFormData = z.infer<typeof brandingFormSchema>

export function brandingToForm(branding?: Branding | null): BrandingFormData {
  return {
    product_name: branding?.product_name ?? '',
    primary_color: branding?.primary_color ?? '',
    logo: branding?.logo ?? '',
    favicon: branding?.favicon ?? '',
  }
}

/** Blank fields are sent as `null` so a cleared value reaches the server. */
export function formToBranding(form: BrandingFormData) {
  const orNull = (value: string) => value.trim() || null
  return {
    product_name: orNull(form.product_name),
    primary_color: orNull(form.primary_color),
    logo: orNull(form.logo),
    favicon: orNull(form.favicon),
  }
}

/** Why an uploaded image cannot be used, or `null` when it can. */
export function brandImageProblem(
  type: string,
  dataUrlLength: number,
  maxBytes: number
): string | null {
  if (!BRAND_IMAGE_TYPES.includes(type)) {
    return 'Use a PNG, JPEG or WebP image. SVG is not accepted.'
  }
  if (dataUrlLength > maxBytes) {
    return `The image is too large; keep it under ${Math.floor((maxBytes * 3) / 4 / 1024)} KB.`
  }
  return null
}
