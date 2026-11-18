// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Custom roles (`/custom-roles`) and assigning one to a team membership
//! (`/teams/{team_id}/members/{user_id}/custom-role`). Instance
//! administration, so guarded by the `Users*` permissions like the rest of
//! the teams surface.

use std::sync::Arc;

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use temps_auth::permission_guard;
use temps_auth::RequireAuth;
use temps_core::problemdetails::Problem;
use temps_core::{AuditContext, AuditOperation, RequestMetadata};
use temps_entities::custom_roles;
use utoipa::ToSchema;

use crate::custom_roles::{
    assignable_permissions, AssignCustomRoleRequest, CustomRoleAssignment, CustomRoleRequest,
};
use crate::service::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};

use super::TeamsAppState;

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct CustomRoleChangedAudit {
    #[serde(flatten)]
    context: AuditContext,
    #[serde(skip)]
    operation: &'static str,
    role_id: i32,
    name: Option<String>,
    permissions: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
struct CustomRoleAssignmentAudit {
    #[serde(flatten)]
    context: AuditContext,
    #[serde(skip)]
    operation: &'static str,
    team_id: i32,
    target_user_id: i32,
    custom_role_id: Option<i32>,
}

macro_rules! impl_audit {
    ($ty:ty) => {
        impl AuditOperation for $ty {
            fn operation_type(&self) -> String {
                self.operation.to_string()
            }
            fn user_id(&self) -> Option<i32> {
                Some(self.context.user_id)
            }
            fn ip_address(&self) -> Option<String> {
                self.context.ip_address.clone()
            }
            fn user_agent(&self) -> &str {
                &self.context.user_agent
            }
            fn serialize(&self) -> temps_core::AuditResult<String> {
                temps_core::serialize_audit_operation(self)
            }
        }
    };
}
impl_audit!(CustomRoleChangedAudit);
impl_audit!(CustomRoleAssignmentAudit);

fn context(user_id: i32, metadata: &RequestMetadata) -> AuditContext {
    AuditContext {
        user_id,
        ip_address: Some(metadata.ip_address.clone()),
        user_agent: metadata.user_agent.clone(),
    }
}

async fn write_audit(state: &TeamsAppState, audit: &dyn AuditOperation) {
    // The write already succeeded; a failed audit entry is logged, not fatal.
    if let Err(e) = state.audit.create_audit_log(audit).await {
        tracing::error!(error = %e, operation = %audit.operation_type(), "teams: failed to write custom role audit log");
    }
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CustomRoleResponse {
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    pub permissions: Vec<String>,
    #[schema(example = "2026-11-17T12:15:47Z")]
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[schema(example = "2026-11-17T12:15:47Z")]
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<custom_roles::Model> for CustomRoleResponse {
    fn from(m: custom_roles::Model) -> Self {
        Self {
            id: m.id,
            name: m.name,
            description: m.description,
            permissions: m.permissions.0,
            created_at: m.created_at,
            updated_at: m.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CustomRoleListResponse {
    pub roles: Vec<CustomRoleResponse>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

/// The permissions a custom role may carry.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AssignablePermissionsResponse {
    pub permissions: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ListCustomRolesQuery {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

pub(crate) fn router() -> Router<Arc<TeamsAppState>> {
    Router::new()
        .route(
            "/custom-roles",
            get(list_custom_roles).post(create_custom_role),
        )
        .route(
            "/custom-roles/permissions",
            get(list_assignable_permissions),
        )
        .route(
            "/custom-roles/{role_id}",
            get(get_custom_role)
                .put(update_custom_role)
                .delete(delete_custom_role),
        )
        .route("/teams/{team_id}/custom-roles", get(list_team_custom_roles))
        .route(
            "/teams/{team_id}/members/{user_id}/custom-role",
            put(assign_custom_role).delete(unassign_custom_role),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[utoipa::path(
    tag = "Teams",
    get,
    path = "/custom-roles",
    params(
        ("page" = Option<u64>, Query, description = "1-indexed page"),
        ("page_size" = Option<u64>, Query, description = "default 20, max 100"),
    ),
    responses(
        (status = 200, description = "Paginated custom roles, newest first", body = CustomRoleListResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_custom_roles(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<TeamsAppState>>,
    Query(q): Query<ListCustomRolesQuery>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, UsersRead);
    let (roles, total) = state.custom_roles.list(q.page, q.page_size).await?;
    Ok(Json(CustomRoleListResponse {
        roles: roles.into_iter().map(CustomRoleResponse::from).collect(),
        total,
        page: q.page.unwrap_or(1).max(1),
        page_size: q
            .page_size
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE),
    }))
}

#[utoipa::path(
    tag = "Teams",
    get,
    path = "/custom-roles/permissions",
    responses(
        (status = 200, description = "Permissions a custom role may carry", body = AssignablePermissionsResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_assignable_permissions(
    RequireAuth(auth): RequireAuth,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, UsersRead);
    Ok(Json(AssignablePermissionsResponse {
        permissions: assignable_permissions(),
    }))
}

#[utoipa::path(
    tag = "Teams",
    get,
    path = "/custom-roles/{role_id}",
    params(("role_id" = i32, Path)),
    responses(
        (status = 200, description = "The custom role", body = CustomRoleResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Custom role not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_custom_role(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<TeamsAppState>>,
    Path(role_id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, UsersRead);
    Ok(Json(CustomRoleResponse::from(
        state.custom_roles.get(role_id).await?,
    )))
}

#[utoipa::path(
    tag = "Teams",
    post,
    path = "/custom-roles",
    request_body = CustomRoleRequest,
    responses(
        (status = 201, description = "Custom role created", body = CustomRoleResponse),
        (status = 400, description = "Invalid name or permissions"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 409, description = "Name already taken"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_custom_role(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<TeamsAppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Json(req): Json<CustomRoleRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, UsersWrite);
    let role = state.custom_roles.create(req).await?;
    write_audit(
        &state,
        &CustomRoleChangedAudit {
            context: context(auth.user_id(), &metadata),
            operation: "CUSTOM_ROLE_CREATED",
            role_id: role.id,
            name: Some(role.name.clone()),
            permissions: Some(role.permissions.0.clone()),
        },
    )
    .await;
    Ok((StatusCode::CREATED, Json(CustomRoleResponse::from(role))))
}

#[utoipa::path(
    tag = "Teams",
    put,
    path = "/custom-roles/{role_id}",
    params(("role_id" = i32, Path)),
    request_body = CustomRoleRequest,
    responses(
        (status = 200, description = "Custom role replaced", body = CustomRoleResponse),
        (status = 400, description = "Invalid name or permissions"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Custom role not found"),
        (status = 409, description = "Name already taken"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_custom_role(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<TeamsAppState>>,
    Path(role_id): Path<i32>,
    Extension(metadata): Extension<RequestMetadata>,
    Json(req): Json<CustomRoleRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, UsersWrite);
    let role = state.custom_roles.update(role_id, req).await?;
    write_audit(
        &state,
        &CustomRoleChangedAudit {
            context: context(auth.user_id(), &metadata),
            operation: "CUSTOM_ROLE_UPDATED",
            role_id,
            name: Some(role.name.clone()),
            permissions: Some(role.permissions.0.clone()),
        },
    )
    .await;
    Ok(Json(CustomRoleResponse::from(role)))
}

#[utoipa::path(
    tag = "Teams",
    delete,
    path = "/custom-roles/{role_id}",
    params(("role_id" = i32, Path)),
    responses(
        (status = 204, description = "Custom role deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Custom role not found"),
        (status = 409, description = "Role still assigned to team members"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_custom_role(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<TeamsAppState>>,
    Path(role_id): Path<i32>,
    Extension(metadata): Extension<RequestMetadata>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, UsersDelete);
    state.custom_roles.delete(role_id).await?;
    write_audit(
        &state,
        &CustomRoleChangedAudit {
            context: context(auth.user_id(), &metadata),
            operation: "CUSTOM_ROLE_DELETED",
            role_id,
            name: None,
            permissions: None,
        },
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    tag = "Teams",
    get,
    path = "/teams/{team_id}/custom-roles",
    params(("team_id" = i32, Path)),
    responses(
        (status = 200, description = "Custom roles held by this team's members", body = Vec<CustomRoleAssignment>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_team_custom_roles(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<TeamsAppState>>,
    Path(team_id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, UsersRead);
    Ok(Json(state.custom_roles.team_assignments(team_id).await?))
}

#[utoipa::path(
    tag = "Teams",
    put,
    path = "/teams/{team_id}/members/{user_id}/custom-role",
    params(("team_id" = i32, Path), ("user_id" = i32, Path)),
    request_body = AssignCustomRoleRequest,
    responses(
        (status = 200, description = "The member now holds this custom role", body = CustomRoleAssignment),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Member or custom role not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn assign_custom_role(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<TeamsAppState>>,
    Path((team_id, user_id)): Path<(i32, i32)>,
    Extension(metadata): Extension<RequestMetadata>,
    Json(req): Json<AssignCustomRoleRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, UsersWrite);
    let assignment = state
        .custom_roles
        .assign(team_id, user_id, req.custom_role_id, auth.user_id())
        .await?;
    write_audit(
        &state,
        &CustomRoleAssignmentAudit {
            context: context(auth.user_id(), &metadata),
            operation: "TEAM_MEMBER_CUSTOM_ROLE_ASSIGNED",
            team_id,
            target_user_id: user_id,
            custom_role_id: Some(assignment.custom_role_id),
        },
    )
    .await;
    Ok(Json(assignment))
}

#[utoipa::path(
    tag = "Teams",
    delete,
    path = "/teams/{team_id}/members/{user_id}/custom-role",
    params(("team_id" = i32, Path), ("user_id" = i32, Path)),
    responses(
        (status = 204, description = "The member is back on their fixed role"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Member not found or holds no custom role"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn unassign_custom_role(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<TeamsAppState>>,
    Path((team_id, user_id)): Path<(i32, i32)>,
    Extension(metadata): Extension<RequestMetadata>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, UsersWrite);
    state.custom_roles.unassign(team_id, user_id).await?;
    write_audit(
        &state,
        &CustomRoleAssignmentAudit {
            context: context(auth.user_id(), &metadata),
            operation: "TEAM_MEMBER_CUSTOM_ROLE_UNASSIGNED",
            team_id,
            target_user_id: user_id,
            custom_role_id: None,
        },
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::TeamProjectAccessChecker;
    use crate::custom_roles::CustomRoleService;
    use crate::service::DefaultTeamService;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{header, Method, Request};
    use sea_orm::{DatabaseBackend, DatabaseConnection, MockDatabase};
    use temps_auth::permissions::Role;
    use temps_auth::AuthContext;
    use temps_core::{
        SensitiveAction, SensitiveActionAuthorizationError, SensitiveActionAuthorizer,
        SensitiveActionDecision, SensitiveActionPrincipal,
    };
    use temps_entities::custom_roles::RolePermissions;
    use tower::ServiceExt;

    #[derive(Default)]
    struct RecordingAudit(std::sync::Mutex<Vec<String>>);

    #[async_trait]
    impl temps_core::AuditLogger for RecordingAudit {
        async fn create_audit_log(
            &self,
            operation: &dyn AuditOperation,
        ) -> temps_core::AuditResult<()> {
            if let Ok(mut ops) = self.0.lock() {
                ops.push(operation.operation_type());
            }
            Ok(())
        }
    }

    struct AllowSensitiveActions;

    #[async_trait]
    impl SensitiveActionAuthorizer for AllowSensitiveActions {
        async fn authorize(
            &self,
            _action: &SensitiveAction,
            _principal: &SensitiveActionPrincipal,
        ) -> Result<SensitiveActionDecision, SensitiveActionAuthorizationError> {
            Ok(SensitiveActionDecision::Allow)
        }
    }

    fn user() -> temps_entities::users::Model {
        let now = chrono::Utc::now();
        temps_entities::users::Model {
            id: 1,
            name: "Test User".to_string(),
            email: "user1@example.test".to_string(),
            password_hash: None,
            email_verified: true,
            email_verification_token: None,
            email_verification_expires: None,
            password_reset_token: None,
            password_reset_expires: None,
            must_change_password: false,
            deleted_at: None,
            mfa_secret: None,
            mfa_enabled: false,
            mfa_recovery_codes: None,
            oidc_subject: None,
            oidc_provider_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn app(
        db: DatabaseConnection,
        auth: Option<AuthContext>,
        audit: Arc<RecordingAudit>,
    ) -> Router {
        let db = Arc::new(db);
        let checker = Arc::new(TeamProjectAccessChecker::new(db.clone()));
        let state = Arc::new(TeamsAppState::new(
            Arc::new(DefaultTeamService::new(db.clone())),
            audit,
            checker.clone(),
            Arc::new(AllowSensitiveActions),
            Arc::new(CustomRoleService::new(db, checker)),
        ));
        router()
            .layer(axum::middleware::from_fn(
                move |mut request: axum::extract::Request, next: axum::middleware::Next| {
                    let auth = auth.clone();
                    async move {
                        request.extensions_mut().insert(RequestMetadata {
                            ip_address: "203.0.113.9".to_string(),
                            user_agent: "route-test".to_string(),
                            headers: axum::http::HeaderMap::new(),
                            visitor_id_cookie: None,
                            session_id_cookie: None,
                            base_url: "http://console.test".to_string(),
                            scheme: "http".to_string(),
                            host: "console.test".to_string(),
                            is_secure: false,
                        });
                        if let Some(auth) = auth {
                            request.extensions_mut().insert(auth);
                        }
                        next.run(request).await
                    }
                },
            ))
            .with_state(state)
    }

    fn create_request() -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri("/custom-roles")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"name":"Release manager","permissions":["deployments:create"]}"#,
            ))
            .expect("request builds")
    }

    async fn status_and_body(app: Router, request: Request<Body>) -> (StatusCode, String) {
        let response = app.oneshot(request).await.expect("router answers");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn mock() -> MockDatabase {
        MockDatabase::new(DatabaseBackend::Postgres)
    }

    #[tokio::test]
    async fn every_route_requires_authentication() {
        let (status, _) = status_and_body(
            app(mock().into_connection(), None, Arc::default()),
            create_request(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_non_admin_cannot_create_a_role() {
        let audit = Arc::new(RecordingAudit::default());
        let (status, body) = status_and_body(
            app(
                mock().into_connection(),
                Some(AuthContext::new_session(user(), Role::User)),
                audit.clone(),
            ),
            create_request(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(audit.0.lock().map(|o| o.is_empty()).unwrap_or(false));
    }

    #[tokio::test]
    async fn an_admin_creates_a_role_and_it_is_audited() {
        let now = chrono::Utc::now();
        let db = mock()
            // name check: free
            .append_query_results([Vec::<custom_roles::Model>::new()])
            // insert returning
            .append_query_results([vec![custom_roles::Model {
                id: 5,
                name: "Release manager".into(),
                description: None,
                permissions: RolePermissions(vec!["deployments:create".into()]),
                created_at: now,
                updated_at: now,
            }]])
            .into_connection();
        let audit = Arc::new(RecordingAudit::default());
        let (status, body) = status_and_body(
            app(
                db,
                Some(AuthContext::new_session(user(), Role::Admin)),
                audit.clone(),
            ),
            create_request(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert!(body.contains("\"id\":5"), "{body}");
        assert_eq!(
            audit.0.lock().map(|o| o.clone()).unwrap_or_default(),
            vec!["CUSTOM_ROLE_CREATED".to_string()]
        );
    }

    #[tokio::test]
    async fn an_invalid_role_is_a_problem_naming_the_permission() {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/custom-roles")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"name":"R","permissions":["users:write"]}"#))
            .expect("request builds");
        let (status, body) = status_and_body(
            app(
                mock().into_connection(),
                Some(AuthContext::new_session(user(), Role::Admin)),
                Arc::default(),
            ),
            request,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("users:write"), "{body}");
        assert!(body.contains("Invalid Custom Role"), "{body}");
    }
}
