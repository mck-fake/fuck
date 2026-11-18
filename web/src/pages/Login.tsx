// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { BrandMark } from '@/components/branding/Brand'
import { LoginForm } from '@/components/auth/login-form'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import {
  emailStatusOptions,
  loginMutation,
} from '@/api/client/@tanstack/react-query.gen'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { AlertCircle } from 'lucide-react'
import { useMemo } from 'react'
import { toast } from 'sonner'
import { useNavigate, useSearchParams } from 'react-router'
import { useAuth } from '@/contexts/AuthContext'
import { usePageTitle } from '@/hooks/usePageTitle'
import { consumeReturnTo, goToReturnTo } from '@/lib/return-to'
import { useI18n } from '@/components/providers/I18nProvider'
import { AuthPreferences } from '@/components/preferences/PreferenceSwitchers'
import type { Messages } from '@/lib/i18n'

/**
 * Maps opaque SSO error codes (from `login_error_code_for` in
 * `oidc_handler.rs`) to user-facing messages. Server returns codes
 * instead of raw IdP text so we don't leak IdP error descriptions
 * into the browser URL / history / Referer. Unknown codes fall
 * through to a generic message. The texts live in `auth.oidcErrors`.
 */
function oidcErrorMessage(t: Messages, reason: string | null): string {
  const messages: Record<string, string> = t.auth.oidcErrors
  return (reason && messages[reason]) || t.auth.ssoFailed
}

export const Login = () => {
  const { t } = useI18n()
  usePageTitle(t.auth.loginPageTitle)
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const { refetch } = useAuth()
  const [searchParams] = useSearchParams()

  const { data: emailStatus } = useQuery(emailStatusOptions())

  const oidcError = useMemo(() => {
    if (searchParams.get('error') !== 'oidc_failed') {
      return null
    }
    // The server returns short opaque codes via `?reason=` (see
    // `login_error_code_for` in `oidc_handler.rs`) so we don't leak
    // raw IdP error text into the browser address bar / history /
    // Referer. Translate each known code into a user-facing message
    // here; unknown values fall through to a generic string.
    const reason = searchParams.get('reason')
    return oidcErrorMessage(t, reason)
  }, [searchParams, t])

  const login = useMutation({
    ...loginMutation(),
    meta: {
      errorTitle: t.auth.loginFailed,
    },
    onSuccess: async (data) => {
      if (data.password_change_required) {
        navigate('/auth/change-password', { replace: true })
        return
      }

      if (data.mfa_required) {
        toast.success(t.auth.completeMfa)
        navigate('/mfa-verify')
        return
      }

      if (data.mfa_enrollment_required && data.mfa_setup) {
        toast.success(t.auth.setUpMfa)
        navigate('/auth/change-password', {
          replace: true,
          state: { mfaSetup: data.mfa_setup },
        })
        return
      }

      toast.success(t.auth.loggedIn)
      await queryClient.invalidateQueries({ queryKey: ['getCurrentUser'] })
      await refetch()
      goToReturnTo(navigate, consumeReturnTo('/dashboard'))
    },
  })

  // `mutate`, not `mutateAsync`: a rejected sign-in is reported by the
  // mutation's error handler (the "Login failed" toast) and must not also
  // escape this submit handler as an unhandled promise rejection.
  const handleSubmit = (data: { email: string; password: string }) => {
    login.mutate({ body: data })
  }

  return (
    <div className="relative flex min-h-screen flex-col items-center justify-center bg-background p-4">
      <AuthPreferences />
      <div className="w-full max-w-sm space-y-6">
        <div className="flex flex-col items-center space-y-6">
          <BrandMark />
          <div className="flex flex-col space-y-2 text-center">
            <h1 className="text-2xl font-semibold tracking-tight">
              {t.auth.welcomeBack}
            </h1>
            <p className="text-sm text-muted-foreground">
              {t.auth.signInToContinue}
            </p>
          </div>
        </div>

        {oidcError && (
          <Alert variant="destructive">
            <AlertCircle className="h-4 w-4" />
            <AlertTitle>{t.auth.ssoFailedTitle}</AlertTitle>
            <AlertDescription>{oidcError}</AlertDescription>
          </Alert>
        )}

        <LoginForm
          onSubmit={handleSubmit}
          isLoading={login.isPending}
          oidcProviders={emailStatus?.oidc_providers ?? []}
          passwordLoginEnabled={emailStatus?.password_login_enabled ?? true}
        />
      </div>
    </div>
  )
}
