// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { getBrandingQueryKey } from '@/api/client/@tanstack/react-query.gen'
import { SettingsFormSkeleton } from '@/components/settings/SettingsFormSkeleton'
import { SettingsLoadFailure } from '@/components/settings/SettingsLoadFailure'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { useBreadcrumbs } from '@/contexts/BreadcrumbContext'
import { usePageTitle } from '@/hooks/usePageTitle'
import { useSettingsPage, useUpdateSettings } from '@/hooks/useSettings'
import {
  brandImageProblem,
  BRAND_IMAGE_TYPES,
  brandingFormSchema,
  brandingToForm,
  DEFAULT_BRAND_ICON,
  DEFAULT_BRAND_NAME,
  formToBranding,
  isHexColor,
  MAX_FAVICON_BYTES,
  MAX_LOGO_BYTES,
  type BrandingFormData,
} from '@/lib/branding'
import { isForbiddenRead } from '@/lib/read-failure'
import { zodResolver } from '@hookform/resolvers/zod'
import { PageHeader, SaveRow, SettingsGroup } from '@temps-sdk/ds'
import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { useForm } from 'react-hook-form'
import { toast } from 'sonner'

function readAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result))
    reader.onerror = () => reject(reader.error)
    reader.readAsDataURL(file)
  })
}

function ImageField({
  id,
  label,
  hint,
  value,
  maxBytes,
  onChange,
}: {
  id: string
  label: string
  hint: string
  value: string
  maxBytes: number
  onChange: (value: string) => void
}) {
  const [problem, setProblem] = useState<string | null>(null)

  const pick = async (file: File | undefined) => {
    if (!file) return
    try {
      const dataUrl = await readAsDataUrl(file)
      const issue = brandImageProblem(file.type, dataUrl.length, maxBytes)
      setProblem(issue)
      if (!issue) onChange(dataUrl)
    } catch {
      setProblem('The file could not be read.')
    }
  }

  return (
    <div className="space-y-2">
      <Label htmlFor={id}>{label}</Label>
      <div className="flex items-center gap-3">
        <span className="flex size-12 shrink-0 items-center justify-center rounded-md border bg-muted/40">
          <img
            src={value || DEFAULT_BRAND_ICON}
            alt={`${label} preview`}
            className="size-9 object-contain"
          />
        </span>
        <Input
          id={id}
          type="file"
          accept={BRAND_IMAGE_TYPES.join(',')}
          onChange={(event) => void pick(event.target.files?.[0])}
        />
        {value && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => {
              setProblem(null)
              onChange('')
            }}
          >
            Remove
          </Button>
        )}
      </div>
      <p className="text-xs text-muted-foreground">{hint}</p>
      {problem && <p className="text-xs text-destructive">{problem}</p>}
    </div>
  )
}

export function BrandingPage() {
  const { setBreadcrumbs } = useBreadcrumbs()
  const queryClient = useQueryClient()
  const { data: settings, isLoading, error, refetch } = useSettingsPage()
  const updateSettings = useUpdateSettings()

  const {
    register,
    handleSubmit,
    watch,
    setValue,
    reset,
    formState: { isDirty, isSubmitting, errors },
  } = useForm<BrandingFormData>({
    resolver: zodResolver(brandingFormSchema),
    defaultValues: brandingToForm(null),
  })

  useEffect(() => {
    setBreadcrumbs([
      { label: 'Settings', href: '/settings' },
      { label: 'Branding' },
    ])
  }, [setBreadcrumbs])

  usePageTitle('Branding')

  useEffect(() => {
    if (settings) reset(brandingToForm(settings.branding))
  }, [settings, reset])

  const save = async (data: BrandingFormData, message: string) => {
    try {
      await updateSettings.mutateAsync({ branding: formToBranding(data) })
      reset(data)
      await queryClient.invalidateQueries({ queryKey: getBrandingQueryKey() })
      toast.success(message)
    } catch {
      // useUpdateSettings already toasts this failure, with the server's reason.
    }
  }

  if (isLoading) {
    return <SettingsFormSkeleton title="Branding" label="Loading branding" />
  }

  if (error) {
    return (
      <SettingsLoadFailure
        error={error}
        onRetry={isForbiddenRead(error) ? undefined : () => void refetch()}
      />
    )
  }

  const color = watch('primary_color')
  const setImage = (field: 'logo' | 'favicon') => (value: string) =>
    setValue(field, value, { shouldDirty: true })

  return (
    <form
      onSubmit={handleSubmit((data) => save(data, 'Branding saved'))}
      className="space-y-6"
    >
      <PageHeader
        title="Branding"
        actions={
          <Button
            type="button"
            variant="outline"
            disabled={isSubmitting}
            onClick={() =>
              void save(brandingToForm(null), 'Branding reset to the default')
            }
          >
            Reset to default
          </Button>
        }
      />
      <div className="max-w-5xl space-y-10">
        <SettingsGroup title="Identity">
          <div className="space-y-2">
            <Label htmlFor="product_name">Product name</Label>
            <Input
              id="product_name"
              placeholder={DEFAULT_BRAND_NAME}
              {...register('product_name')}
            />
            <p className="text-xs text-muted-foreground">
              Shown in the sidebar, the sign-in page and the browser tab.
            </p>
            {errors.product_name && (
              <p className="text-xs text-destructive">
                {errors.product_name.message}
              </p>
            )}
          </div>
          <div className="space-y-2">
            <Label htmlFor="primary_color">Accent colour</Label>
            <div className="flex items-center gap-3">
              <input
                type="color"
                aria-label="Pick the accent colour"
                className="h-9 w-12 shrink-0 cursor-pointer rounded-md border bg-transparent"
                value={isHexColor(color) ? color : '#000000'}
                onChange={(event) =>
                  setValue('primary_color', event.target.value, {
                    shouldDirty: true,
                    shouldValidate: true,
                  })
                }
              />
              <Input
                id="primary_color"
                placeholder="Default"
                {...register('primary_color')}
              />
            </div>
            <p className="text-xs text-muted-foreground">
              Primary buttons and links. Leave empty for the default.
            </p>
            {errors.primary_color && (
              <p className="text-xs text-destructive">
                {errors.primary_color.message}
              </p>
            )}
          </div>
        </SettingsGroup>
        <SettingsGroup title="Images">
          <ImageField
            id="logo"
            label="Logo"
            hint="Square PNG, JPEG or WebP, under 96 KB."
            value={watch('logo')}
            maxBytes={MAX_LOGO_BYTES}
            onChange={setImage('logo')}
          />
          <ImageField
            id="favicon"
            label="Favicon"
            hint="Square PNG, JPEG or WebP, under 24 KB."
            value={watch('favicon')}
            maxBytes={MAX_FAVICON_BYTES}
            onChange={setImage('favicon')}
          />
        </SettingsGroup>
      </div>
      <SaveRow sticky dirty={isDirty} saving={isSubmitting} />
    </form>
  )
}
