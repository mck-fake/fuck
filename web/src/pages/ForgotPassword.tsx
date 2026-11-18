// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { BrandMark } from '@/components/branding/Brand'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form'
import { Input } from '@/components/ui/input'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { CopyButton } from '@/components/ui/copy-button'
import {
  emailStatusOptions,
  requestPasswordResetMutation,
} from '@/api/client/@tanstack/react-query.gen'
import { zodResolver } from '@hookform/resolvers/zod'
import { useMutation, useQuery } from '@tanstack/react-query'
import { AlertCircle, ArrowLeft, Loader2, MailCheck } from 'lucide-react'
import { useMemo } from 'react'
import { useForm } from 'react-hook-form'
import { Link } from 'react-router'
import { z } from 'zod'
import { usePageTitle } from '@/hooks/usePageTitle'
import { useI18n } from '@/components/providers/I18nProvider'
import { AuthPreferences } from '@/components/preferences/PreferenceSwitchers'
import { formatParts, type Messages } from '@/lib/i18n'

// Resets the admin password without email; run on the server, where the
// installer's `temps` command already knows the database URL.
export const RESET_ADMIN_PASSWORD_COMMAND = 'sudo temps reset-admin-password'

// Built per language so the validation message follows the selected one.
const forgotPasswordSchemaFor = (t: Messages) =>
  z.object({ email: z.email(t.auth.invalidEmail) })

type ForgotPasswordFormData = z.infer<
  ReturnType<typeof forgotPasswordSchemaFor>
>

export const ForgotPassword = () => {
  const { t } = useI18n()
  usePageTitle(t.auth.forgot.pageTitle)
  const forgotPasswordSchema = useMemo(() => forgotPasswordSchemaFor(t), [t])

  // The server only offers password reset when an email provider is
  // configured (request_password_reset returns 503 otherwise). Surface
  // that up front so the user isn't told "check your inbox" for an email
  // that will never arrive.
  const { data: emailStatus } = useQuery(emailStatusOptions())
  const resetAvailable = emailStatus?.password_reset_available ?? true

  const form = useForm<ForgotPasswordFormData>({
    resolver: zodResolver(forgotPasswordSchema),
    defaultValues: { email: '' },
  })

  // The request endpoint is enumeration-safe: it returns 200 whether or
  // not the account exists. The only error path is 503 (email provider
  // not configured), which the global mutation handler toasts — and we
  // also pre-gate the form on `resetAvailable` so it rarely fires.
  const requestReset = useMutation({
    ...requestPasswordResetMutation(),
    meta: { errorTitle: t.auth.forgot.sendFailed },
  })

  const handleSubmit = async (data: ForgotPasswordFormData) => {
    await requestReset.mutateAsync({ body: { email: data.email } })
  }

  return (
    <div className="relative flex min-h-screen flex-col items-center justify-center bg-background p-4">
      <AuthPreferences />
      <div className="w-full max-w-sm space-y-6">
        <div className="flex flex-col items-center space-y-6">
          <BrandMark />
        </div>

        {requestReset.isSuccess ? (
          <Card className="w-full max-w-sm">
            <CardHeader>
              <div className="mb-2 flex size-10 items-center justify-center rounded-full bg-primary/10">
                <MailCheck className="size-5 text-primary" />
              </div>
              <CardTitle className="text-2xl">
                {t.auth.forgot.checkEmail}
              </CardTitle>
              <CardDescription>
                {formatParts(t.auth.forgot.sent, {
                  email: (
                    <span key="email" className="font-medium text-foreground">
                      {form.getValues('email')}
                    </span>
                  ),
                })}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <Button asChild variant="outline" className="w-full">
                <Link to="/">
                  <ArrowLeft className="mr-2 h-4 w-4" />
                  {t.auth.forgot.backToLogin}
                </Link>
              </Button>
            </CardContent>
          </Card>
        ) : (
          <Card className="w-full max-w-sm">
            <CardHeader>
              <CardTitle className="text-2xl">{t.auth.forgot.title}</CardTitle>
              <CardDescription>{t.auth.forgot.description}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              {!resetAvailable && (
                <Alert variant="destructive">
                  <AlertCircle className="h-4 w-4" />
                  <AlertTitle>{t.auth.forgot.unavailableTitle}</AlertTitle>
                  <AlertDescription className="space-y-2">
                    <p>{t.auth.forgot.unavailable}</p>
                    <div className="flex items-center gap-2 rounded-md bg-muted px-3 py-2 text-foreground">
                      <code className="min-w-0 flex-1 font-mono text-xs [overflow-wrap:anywhere]">
                        {RESET_ADMIN_PASSWORD_COMMAND}
                      </code>
                      <CopyButton
                        value={RESET_ADMIN_PASSWORD_COMMAND}
                        minimal
                        label={t.auth.forgot.copyCommand}
                        className="shrink-0 rounded p-1 text-muted-foreground [&_svg]:size-3.5"
                      />
                    </div>
                  </AlertDescription>
                </Alert>
              )}

              <Form {...form}>
                <form
                  onSubmit={form.handleSubmit(handleSubmit)}
                  className="space-y-4"
                >
                  <FormField
                    control={form.control}
                    name="email"
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel>{t.auth.email}</FormLabel>
                        <FormControl>
                          <Input
                            type="email"
                            placeholder="you@example.com"
                            autoComplete="email"
                            disabled={requestReset.isPending || !resetAvailable}
                            {...field}
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                  <Button
                    type="submit"
                    className="w-full"
                    disabled={requestReset.isPending || !resetAvailable}
                  >
                    {requestReset.isPending ? (
                      <>
                        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                        {t.auth.forgot.sending}
                      </>
                    ) : (
                      t.auth.forgot.send
                    )}
                  </Button>
                </form>
              </Form>

              <Button asChild variant="ghost" className="w-full">
                <Link to="/">
                  <ArrowLeft className="mr-2 h-4 w-4" />
                  {t.auth.forgot.backToLogin}
                </Link>
              </Button>
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  )
}
