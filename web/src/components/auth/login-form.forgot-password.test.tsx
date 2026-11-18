// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { afterEach, expect, test } from 'bun:test'
import { cleanup, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { installBrowserWindow } from '@/test/dom'
import { LoginForm } from './login-form'

installBrowserWindow()
afterEach(cleanup)

test('"Forgot password?" is always offered next to the password field', () => {
  render(
    <MemoryRouter>
      <LoginForm onSubmit={async () => {}} isLoading={false} />
    </MemoryRouter>
  )

  const link = screen.getByRole('link', { name: 'Forgot password?' })
  expect(link.getAttribute('href')).toBe('/forgot-password')
})
