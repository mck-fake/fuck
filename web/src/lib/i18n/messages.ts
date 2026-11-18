// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// English is the source of truth. Every other language is typed as
// `Messages`, so a missing or extra key fails the TypeScript build instead of
// shipping a half-translated screen. Placeholders use `{name}` (see `format`).

export const en = {
  common: {
    cancel: 'Cancel',
    delete: 'Delete',
    copyToClipboard: 'Copy to clipboard',
    copied: 'Copied',
    copyFailed: 'Copy failed',
    copyFailedTitle: "Couldn't copy to clipboard",
    copyBlocked:
      'Your browser blocked the copy. Select the text and copy it manually.',
  },
  preferences: {
    appearance: 'Appearance',
    light: 'Light',
    dark: 'Dark',
    system: 'System',
    language: 'Language',
  },
  account: {
    account: 'Account',
    logOut: 'Log out',
  },
  auth: {
    loginPageTitle: 'Login',
    welcomeBack: 'Welcome back',
    signInToContinue: 'Sign in to your account to continue',
    loginTitle: 'Login',
    loginDescription: 'Enter your email and password to access your account',
    email: 'Email',
    password: 'Password',
    passwordPlaceholder: 'Enter your password',
    forgotPassword: 'Forgot password?',
    signIn: 'Sign in',
    signingIn: 'Signing in...',
    or: 'or',
    passwordLoginDisabled:
      'Password sign-in is disabled and no SSO provider is configured on this server. Contact your administrator.',
    invalidEmail: 'Please enter a valid email address',
    passwordRequired: 'Password is required',
    signInWith: 'Sign in with {name}',
    continueWithCloud: 'Continue with Temps Cloud',
    loginFailed: 'Login failed',
    completeMfa: 'Please complete MFA verification',
    setUpMfa: 'Set up multi-factor authentication to continue',
    loggedIn: 'Logged in successfully',
    ssoFailedTitle: 'SSO sign-in failed',
    ssoFailed: 'SSO sign-in failed.',
    oidcErrors: {
      idp_error:
        'Your identity provider rejected the login. Check that your account is allowed.',
      idp_unreachable:
        "We couldn't reach your identity provider. Try again in a moment.",
      idp_rejected_code:
        'Your identity provider rejected the authorization code. Try signing in again.',
      state_invalid:
        'This SSO link is invalid or has already been used. Start sign-in again.',
      state_expired: 'This SSO link expired. Start sign-in again.',
      id_token_invalid:
        'Your identity provider returned an invalid token. Contact your administrator.',
      callback_invalid: 'The SSO callback was malformed. Start sign-in again.',
      email_missing:
        'Your identity provider did not return an email address. Grant the "email" scope and try again.',
      email_not_verified:
        'Your identity provider has not confirmed your email. Verify it at the IdP, then try again.',
      user_not_provisioned:
        'No Temps account exists for this email. Ask an administrator to create one.',
      provider_disabled: 'This SSO provider is currently disabled.',
      credentials_changed:
        'Your password was reset by an administrator while you were signing in. Start sign-in again.',
      provider_not_found: 'The SSO provider configuration was not found.',
      no_provider_configured:
        'No SSO provider is configured on this Temps instance.',
      issuer_invalid: 'The SSO provider URL is invalid.',
      return_to_invalid: 'Invalid post-login redirect target.',
      role_invalid: 'The role assigned by the SSO provider is invalid.',
      role_mapping_not_found: 'No matching SSO role mapping.',
      provider_conflict: 'SSO provider configuration conflict.',
      provider_managed_by_cloud:
        'This SSO provider is managed by Temps Cloud and cannot be edited here.',
      insufficient_role:
        "Your Temps Cloud account doesn't have the owner or admin role on this instance. Ask an instance owner or admin to grant it, then try again.",
      issuer_managed_by_cloud:
        'This issuer is already used by the Temps Cloud-managed sign-in provider. Use "Continue with Temps Cloud" to sign in, or remove the Cloud link before adding a custom provider with this issuer.',
      internal_error:
        'An internal error occurred while processing the SSO callback.',
    },
    forgot: {
      pageTitle: 'Forgot password',
      title: 'Forgot password?',
      description:
        "Enter your email and we'll send you a link to reset your password.",
      unavailableTitle: 'Password reset unavailable',
      unavailable:
        "No email provider is configured, so reset links can't be sent. If you run this server, set a new admin password from its terminal:",
      copyCommand: 'Copy command',
      send: 'Send reset link',
      sending: 'Sending...',
      backToLogin: 'Back to login',
      checkEmail: 'Check your email',
      sent: "If an account exists for {email}, we've sent a link to reset your password. The link expires in 1 hour.",
      sendFailed: 'Could not send reset link',
    },
  },
  // Keyed by the English title in primary-navigation.ts; a title missing here
  // falls back to English (see `navTitle`).
  navTitles: {} as Record<string, string>,
} as const

type Widen<T> = {
  [K in keyof T]: T[K] extends string ? string : Widen<T[K]>
}

export type Messages = Widen<typeof en>

export const es: Messages = {
  common: {
    cancel: 'Cancelar',
    delete: 'Eliminar',
    copyToClipboard: 'Copiar al portapapeles',
    copied: 'Copiado',
    copyFailed: 'No se pudo copiar',
    copyFailedTitle: 'No se pudo copiar al portapapeles',
    copyBlocked:
      'Tu navegador bloqueó la copia. Selecciona el texto y cópialo manualmente.',
  },
  preferences: {
    appearance: 'Apariencia',
    light: 'Claro',
    dark: 'Oscuro',
    system: 'Sistema',
    language: 'Idioma',
  },
  account: {
    account: 'Cuenta',
    logOut: 'Cerrar sesión',
  },
  auth: {
    loginPageTitle: 'Iniciar sesión',
    welcomeBack: 'Bienvenido de nuevo',
    signInToContinue: 'Inicia sesión en tu cuenta para continuar',
    loginTitle: 'Iniciar sesión',
    loginDescription:
      'Introduce tu correo y tu contraseña para acceder a tu cuenta',
    email: 'Correo electrónico',
    password: 'Contraseña',
    passwordPlaceholder: 'Introduce tu contraseña',
    forgotPassword: '¿Olvidaste tu contraseña?',
    signIn: 'Iniciar sesión',
    signingIn: 'Iniciando sesión...',
    or: 'o',
    passwordLoginDisabled:
      'El inicio de sesión con contraseña está desactivado y no hay ningún proveedor SSO configurado en este servidor. Contacta con tu administrador.',
    invalidEmail: 'Introduce un correo electrónico válido',
    passwordRequired: 'La contraseña es obligatoria',
    signInWith: 'Iniciar sesión con {name}',
    continueWithCloud: 'Continuar con Temps Cloud',
    loginFailed: 'No se pudo iniciar sesión',
    completeMfa: 'Completa la verificación en dos pasos',
    setUpMfa: 'Configura la autenticación en dos pasos para continuar',
    loggedIn: 'Sesión iniciada',
    ssoFailedTitle: 'Falló el inicio de sesión con SSO',
    ssoFailed: 'Falló el inicio de sesión con SSO.',
    oidcErrors: {
      idp_error:
        'Tu proveedor de identidad rechazó el inicio de sesión. Comprueba que tu cuenta tiene acceso.',
      idp_unreachable:
        'No pudimos contactar con tu proveedor de identidad. Inténtalo de nuevo en un momento.',
      idp_rejected_code:
        'Tu proveedor de identidad rechazó el código de autorización. Vuelve a iniciar sesión.',
      state_invalid:
        'Este enlace de SSO no es válido o ya se usó. Vuelve a iniciar sesión.',
      state_expired: 'Este enlace de SSO caducó. Vuelve a iniciar sesión.',
      id_token_invalid:
        'Tu proveedor de identidad devolvió un token no válido. Contacta con tu administrador.',
      callback_invalid:
        'La respuesta de SSO llegó mal formada. Vuelve a iniciar sesión.',
      email_missing:
        'Tu proveedor de identidad no devolvió un correo electrónico. Concede el permiso "email" e inténtalo de nuevo.',
      email_not_verified:
        'Tu proveedor de identidad no ha confirmado tu correo. Verifícalo allí e inténtalo de nuevo.',
      user_not_provisioned:
        'No existe una cuenta de Temps para este correo. Pide a un administrador que la cree.',
      provider_disabled: 'Este proveedor SSO está desactivado.',
      credentials_changed:
        'Un administrador restableció tu contraseña mientras iniciabas sesión. Vuelve a iniciar sesión.',
      provider_not_found: 'No se encontró la configuración del proveedor SSO.',
      no_provider_configured:
        'No hay ningún proveedor SSO configurado en esta instancia de Temps.',
      issuer_invalid: 'La URL del proveedor SSO no es válida.',
      return_to_invalid: 'El destino tras iniciar sesión no es válido.',
      role_invalid: 'El rol asignado por el proveedor SSO no es válido.',
      role_mapping_not_found:
        'No hay ninguna asignación de rol SSO que coincida.',
      provider_conflict: 'Conflicto en la configuración del proveedor SSO.',
      provider_managed_by_cloud:
        'Este proveedor SSO lo gestiona Temps Cloud y no se puede editar aquí.',
      insufficient_role:
        'Tu cuenta de Temps Cloud no tiene el rol de propietario o administrador en esta instancia. Pide a un propietario o administrador que te lo conceda e inténtalo de nuevo.',
      issuer_managed_by_cloud:
        'Este emisor ya lo usa el proveedor de inicio de sesión gestionado por Temps Cloud. Usa "Continuar con Temps Cloud" para entrar, o quita el enlace con Cloud antes de añadir un proveedor propio con este emisor.',
      internal_error:
        'Ocurrió un error interno al procesar la respuesta de SSO.',
    },
    forgot: {
      pageTitle: 'Recuperar contraseña',
      title: '¿Olvidaste tu contraseña?',
      description:
        'Introduce tu correo y te enviaremos un enlace para restablecer tu contraseña.',
      unavailableTitle: 'Recuperación de contraseña no disponible',
      unavailable:
        'No hay ningún proveedor de correo configurado, así que no se pueden enviar enlaces. Si administras este servidor, define una nueva contraseña de administrador desde su terminal:',
      copyCommand: 'Copiar comando',
      send: 'Enviar enlace',
      sending: 'Enviando...',
      backToLogin: 'Volver al inicio de sesión',
      checkEmail: 'Revisa tu correo',
      sent: 'Si existe una cuenta para {email}, te hemos enviado un enlace para restablecer tu contraseña. El enlace caduca en 1 hora.',
      sendFailed: 'No se pudo enviar el enlace',
    },
  },
  navTitles: {
    Applications: 'Aplicaciones',
    Projects: 'Proyectos',
    Vault: 'Bóveda',
    'Git connections': 'Conexiones Git',
    Domains: 'Dominios',
    Email: 'Correo',
    'Worker nodes': 'Nodos de trabajo',
    Storage: 'Almacenamiento',
    Databases: 'Bases de datos',
    Backups: 'Copias de seguridad',
    Observability: 'Observabilidad',
    Analytics: 'Analítica',
    Traces: 'Trazas',
    Logs: 'Registros',
    Errors: 'Errores',
    'Server metrics': 'Métricas del servidor',
    Monitoring: 'Monitorización',
    Proxy: 'Proxy',
    'Status pages': 'Páginas de estado',
    Administration: 'Administración',
    'All tools': 'Todas las herramientas',
    Plugins: 'Plugins',
  },
}
