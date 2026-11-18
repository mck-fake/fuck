// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { brandedTitle } from '@/lib/branding'
import { useEffect } from 'react'

export function usePageTitle(title: string) {
  useEffect(() => {
    const previousTitle = document.title
    document.title = brandedTitle(title)

    return () => {
      document.title = previousTitle
    }
  }, [title])
}
