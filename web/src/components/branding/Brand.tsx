// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { getBrandingOptions } from '@/api/client/@tanstack/react-query.gen'
import {
  brandIcon,
  brandName,
  DEFAULT_BRAND_ICON,
  foregroundFor,
  getBrandName,
  setBrandName,
} from '@/lib/branding'
import { cn } from '@/lib/utils'
import { useQuery } from '@tanstack/react-query'
import { useEffect, useState } from 'react'

/** The instance branding (public endpoint, readable before sign-in). */
export function useBranding() {
  return useQuery({
    ...getBrandingOptions(),
    staleTime: 5 * 60 * 1000,
    retry: false,
  })
}

export function BrandName() {
  const { data } = useBranding()
  return <>{brandName(data)}</>
}

/** The console mark; falls back to the Temps icon, also for a broken image. */
export function BrandIcon({
  className = 'size-full',
  alt = '',
}: {
  className?: string
  alt?: string
}) {
  const { data } = useBranding()
  const src = brandIcon(data)
  const [failed, setFailed] = useState<string | null>(null)
  return (
    <img
      src={failed === src ? DEFAULT_BRAND_ICON : src}
      alt={alt}
      className={cn('object-contain', className)}
      onError={() => setFailed(src)}
    />
  )
}

/** Logo and name, as on the sign-in pages. */
export function BrandMark() {
  const { data } = useBranding()
  const name = brandName(data)
  return (
    <div className="flex items-center gap-3">
      <BrandIcon className="size-12" alt={`${name} logo`} />
      <span className="text-2xl font-bold">{name}</span>
    </div>
  )
}

function setFavicon(href: string | null) {
  let link = document.querySelector<HTMLLinkElement>(
    'link[rel="icon"][data-branding]'
  )
  if (!href) {
    link?.remove()
    return
  }
  if (!link) {
    link = document.createElement('link')
    link.rel = 'icon'
    link.dataset.branding = 'true'
    // After the built-in icon links, so it takes precedence.
    document.head.appendChild(link)
  }
  link.href = href
}

/**
 * Applies the branding to the document: page-title name, favicon and accent
 * colour. Renders nothing; unset fields leave the built-in look in place.
 */
export function BrandingEffects() {
  const { data } = useBranding()
  const name = brandName(data)
  const color = data?.primary_color ?? null
  const favicon = data?.favicon ?? null

  useEffect(() => {
    const previous = getBrandName()
    setBrandName(name)
    if (previous !== name && document.title.endsWith(previous)) {
      document.title = document.title.slice(0, -previous.length) + name
    }
  }, [name])

  useEffect(() => setFavicon(favicon), [favicon])

  useEffect(() => {
    const style = document.documentElement.style
    const foreground = color ? foregroundFor(color) : null
    if (color && foreground) {
      style.setProperty('--primary', color)
      style.setProperty('--primary-foreground', foreground)
    }
    return () => {
      style.removeProperty('--primary')
      style.removeProperty('--primary-foreground')
    }
  }, [color])

  return null
}
