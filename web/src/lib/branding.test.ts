// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { expect, test } from 'bun:test'
import {
  brandedTitle,
  brandIcon,
  brandImageProblem,
  brandingFormSchema,
  brandingToForm,
  formToBranding,
  MAX_FAVICON_BYTES,
  brandName,
  DEFAULT_BRAND_ICON,
  foregroundFor,
} from './branding'

test('unset or blank branding falls back to the built-in look', () => {
  expect(brandName(undefined)).toBe('Temps')
  expect(brandName({ product_name: '   ' })).toBe('Temps')
  expect(brandName({ product_name: 'Acme Cloud' })).toBe('Acme Cloud')
  expect(brandIcon(null)).toBe(DEFAULT_BRAND_ICON)
  expect(brandIcon({ logo: 'data:image/png;base64,AAAA' })).toBe(
    'data:image/png;base64,AAAA'
  )
})

test('accent foreground is dark on light colours and light on dark ones', () => {
  expect(foregroundFor('#ffffff')).toBe('oklch(0.145 0 0)')
  expect(foregroundFor('#facc15')).toBe('oklch(0.145 0 0)')
  expect(foregroundFor('#1e3a8a')).toBe('oklch(0.985 0 0)')
  expect(foregroundFor('#000000')).toBe('oklch(0.985 0 0)')
  expect(foregroundFor('blue')).toBeNull()
  expect(foregroundFor('#12345')).toBeNull()
})

test('page titles carry the product name', () => {
  expect(brandedTitle('Projects', 'Acme')).toBe('Projects - Acme')
  expect(brandedTitle('', 'Acme')).toBe('Acme')
})

test('the form sends cleared fields as null and validates the colour', () => {
  expect(
    formToBranding({
      product_name: ' Acme ',
      primary_color: '',
      logo: '',
      favicon: 'data:image/png;base64,AA',
    })
  ).toEqual({
    product_name: 'Acme',
    primary_color: null,
    logo: null,
    favicon: 'data:image/png;base64,AA',
  })
  expect(brandingToForm(undefined).product_name).toBe('')
  const parse = (primary_color: string) =>
    brandingFormSchema.safeParse({
      ...brandingToForm(null),
      primary_color,
    }).success
  expect(parse('')).toBe(true)
  expect(parse('#2563eb')).toBe(true)
  expect(parse('blue')).toBe(false)
})

test('uploads must be small raster images', () => {
  expect(brandImageProblem('image/svg+xml', 10, MAX_FAVICON_BYTES)).toContain(
    'SVG'
  )
  expect(
    brandImageProblem('image/png', MAX_FAVICON_BYTES + 1, MAX_FAVICON_BYTES)
  ).toContain('too large')
  expect(brandImageProblem('image/webp', 100, MAX_FAVICON_BYTES)).toBeNull()
})
