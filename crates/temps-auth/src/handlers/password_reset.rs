// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Password reset by email: request a link, then set a new password with it.
//! Both refuse while the instance is SSO only (`password_login_disabled`).

use super::*;

/// The same answer on every password path when the instance is SSO only.
pub(super) fn password_login_disabled_problem() -> temps_core::problemdetails::Problem {
    problem_new(StatusCode::FORBIDDEN)
        .with_title("Password Sign-In Disabled")
        .with_detail(
            "This instance only allows single sign-on. Sign in with your SSO provider; an operator can re-enable passwords with `temps reset-admin-password`.",
        )
}

#[utoipa::path(
    post,
    path = "/auth/password-reset/request",
    request_body = EmailRequest,
    responses(
        (status = 200, description = "Reset email sent if account exists", body = AuthResponse),
        (status = 403, description = "Password sign-in is disabled; the instance is SSO only"),
        (status = 503, description = "Email service not configured")
    ),
    tag = "Authentication"
)]
pub async fn request_password_reset(
    State(state): State<Arc<AuthState>>,
    Json(body): Json<EmailRequest>,
) -> Result<impl IntoResponse, temps_core::problemdetails::Problem> {
    if !state.auth_service.is_email_configured().await {
        return Err(problem_new(StatusCode::SERVICE_UNAVAILABLE)
            .with_title("Email Service Not Configured")
            .with_detail("Password reset is not available without email configuration"));
    }

    let email = &body.email;

    match state.auth_service.request_password_reset(email).await {
        Err(crate::auth_service::UserAuthError::PasswordLoginDisabled) => {
            Err(password_login_disabled_problem())
        }
        Ok(_) | Err(_) => {
            // Always return success to prevent email enumeration
            Ok(Json(AuthResponse {
                success: true,
                message:
                    "If an account exists with this email, a password reset link has been sent"
                        .to_string(),
                user_id: None,
                mfa_required: false,
                mfa_enrollment_required: false,
                mfa_setup: None,
                password_change_required: false,
            }))
        }
    }
}

#[utoipa::path(
    post,
    path = "/auth/password-reset/verify",
    request_body = ResetPasswordRequest,
    responses(
        (status = 200, description = "Password reset successful", body = AuthResponse),
        (status = 400, description = "Invalid or expired token"),
        (status = 403, description = "Password sign-in is disabled; the instance is SSO only"),
        (status = 500, description = "Internal server error")
    ),
    tag = "Authentication"
)]
pub async fn reset_password(
    State(state): State<Arc<AuthState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Json(request): Json<ResetPasswordRequest>,
) -> Result<impl IntoResponse, temps_core::problemdetails::Problem> {
    match state.auth_service.reset_password(request.into()).await {
        Ok(user) => {
            // Create audit log
            let audit_context = AuditContext {
                user_id: user.id,
                ip_address: Some(metadata.ip_address.to_string()),
                user_agent: metadata.user_agent.as_str().to_string(),
            };

            let password_reset_audit = PasswordResetAudit {
                context: audit_context,
                username: user.name.clone(),
            };

            if let Err(e) = state
                .audit_service
                .create_audit_log(&password_reset_audit)
                .await
            {
                error!("Failed to create audit log: {}", e);
            }

            Ok(Json(AuthResponse {
                success: true,
                message: "Password reset successful. You can now login with your new password."
                    .to_string(),
                user_id: None,
                mfa_required: false,
                mfa_enrollment_required: false,
                mfa_setup: None,
                password_change_required: false,
            }))
        }
        Err(crate::auth_service::UserAuthError::InvalidToken) => {
            Err(problem_new(StatusCode::BAD_REQUEST)
                .with_title("Password Reset Failed")
                .with_detail("The password reset link is invalid or expired."))
        }
        Err(crate::auth_service::UserAuthError::WeakPassword(message)) => {
            Err(problem_new(StatusCode::BAD_REQUEST)
                .with_title("Password Requirements Not Met")
                .with_detail(message))
        }
        Err(crate::auth_service::UserAuthError::SamePassword) => {
            Err(problem_new(StatusCode::BAD_REQUEST)
                .with_title("Choose a Different Password")
                .with_detail("Your new password must differ from the temporary password."))
        }
        Err(crate::auth_service::UserAuthError::PasswordLoginDisabled) => {
            Err(password_login_disabled_problem())
        }
        Err(error) => {
            error!(error = %error, "Password reset failed internally");
            Err(problem_new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Password Reset Failed")
                .with_detail("Could not reset the password. Please try again."))
        }
    }
}
