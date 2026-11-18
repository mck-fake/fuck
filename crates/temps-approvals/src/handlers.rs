// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! HTTP surface: the per-environment switch, a deployment's request, the
//! approve/reject decision and the project's list of requests.

use std::sync::Arc;

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use temps_authz::{project_permission_guard, project_scope_guard, RequireAuth};
use temps_core::problemdetails::{self, Problem};
use temps_core::{
    AuditContext, AuditLogger, AuditOperation, ProjectAccessChecker, RequestMetadata,
};
use temps_entities::deployment_approvals as approvals;
use tracing::error;
use utoipa::{IntoParams, OpenApi, ToSchema};

use crate::error::ApprovalError;
use crate::service::{normalize_pagination, ApprovalDecision, ApprovalService};

pub struct ApprovalsAppState {
    pub approval_service: Arc<ApprovalService>,
    pub audit_service: Arc<dyn AuditLogger>,
    pub project_access_checker: Option<Arc<dyn ProjectAccessChecker>>,
}

impl From<ApprovalError> for Problem {
    fn from(error: ApprovalError) -> Self {
        let (status, title) = match &error {
            ApprovalError::EnvironmentNotFound { .. } => {
                (StatusCode::NOT_FOUND, "Environment Not Found")
            }
            ApprovalError::DeploymentNotFound { .. } => {
                (StatusCode::NOT_FOUND, "Deployment Not Found")
            }
            ApprovalError::ApprovalNotFound { .. } => {
                (StatusCode::NOT_FOUND, "Approval Request Not Found")
            }
            ApprovalError::AlreadyDecided { .. } => (StatusCode::CONFLICT, "Already Decided"),
            ApprovalError::CommentTooLong { .. } => (StatusCode::BAD_REQUEST, "Comment Too Long"),
            ApprovalError::InvalidStatus { .. } => {
                (StatusCode::BAD_REQUEST, "Invalid Status Filter")
            }
            ApprovalError::InvalidDeploymentId { .. } => {
                (StatusCode::BAD_REQUEST, "Invalid Deployment Id")
            }
            ApprovalError::RecheckNotQueued { .. } => {
                (StatusCode::SERVICE_UNAVAILABLE, "Deployment Not Queued")
            }
            ApprovalError::CancelFailed { .. } => {
                (StatusCode::INTERNAL_SERVER_ERROR, "Cancellation Failed")
            }
            ApprovalError::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Database Error"),
        };
        problemdetails::new(status)
            .with_title(title)
            .with_detail(error.to_string())
    }
}

// ---- types -------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct ApprovalPolicyResponse {
    pub environment_id: i32,
    /// Every deployment to this environment waits for a person to approve it.
    pub required: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetApprovalPolicyRequest {
    pub required: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApprovalResponse {
    pub id: i32,
    pub deployment_id: i32,
    pub project_id: i32,
    pub environment_id: i32,
    /// `pending`, `approved` or `rejected`.
    pub status: String,
    pub decided_by: Option<i32>,
    #[schema(value_type = Option<String>, format = "date-time")]
    pub decided_at: Option<chrono::DateTime<chrono::Utc>>,
    pub comment: Option<String>,
    #[schema(value_type = String, format = "date-time")]
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<approvals::Model> for ApprovalResponse {
    fn from(model: approvals::Model) -> Self {
        Self {
            id: model.id,
            deployment_id: model.deployment_id,
            project_id: model.project_id,
            environment_id: model.environment_id,
            status: model.status,
            decided_by: model.decided_by,
            decided_at: model.decided_at,
            comment: model.comment,
            created_at: model.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeploymentApprovalStatus {
    pub deployment_id: i32,
    /// `null` when the deployment never needed approval.
    pub approval: Option<ApprovalResponse>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DecideApprovalRequest {
    pub decision: ApprovalDecision,
    /// Optional note kept with the decision (at most 500 characters).
    pub comment: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListApprovalsQuery {
    /// `pending`, `approved` or `rejected`; all when omitted.
    pub status: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApprovalListResponse {
    pub approvals: Vec<ApprovalResponse>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

// ---- audit -------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ApprovalPolicyUpdatedAudit {
    context: AuditContext,
    project_id: i32,
    environment_id: i32,
    required: bool,
}

#[derive(Debug, Serialize)]
struct DeploymentApprovalDecidedAudit {
    context: AuditContext,
    project_id: i32,
    deployment_id: i32,
    status: String,
    comment: Option<String>,
}

macro_rules! impl_audit_operation {
    ($ty:ty, $op:literal) => {
        impl AuditOperation for $ty {
            fn operation_type(&self) -> String {
                $op.to_string()
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

impl_audit_operation!(
    ApprovalPolicyUpdatedAudit,
    "ENVIRONMENT_APPROVAL_POLICY_UPDATED"
);
impl_audit_operation!(
    DeploymentApprovalDecidedAudit,
    "DEPLOYMENT_APPROVAL_DECIDED"
);

fn audit_context(auth: &temps_authz::AuthContext, metadata: &RequestMetadata) -> AuditContext {
    AuditContext {
        user_id: auth.user_id(),
        ip_address: Some(metadata.ip_address.clone()),
        user_agent: metadata.user_agent.clone(),
    }
}

async fn write_audit(state: &ApprovalsAppState, audit: &dyn AuditOperation) {
    // The change already happened; a lost audit entry is logged, not fatal.
    if let Err(e) = state.audit_service.create_audit_log(audit).await {
        error!(
            "Failed to write audit log {}: {}",
            audit.operation_type(),
            e
        );
    }
}

// ---- handlers ----------------------------------------------------------

/// Whether an environment requires approval before each deployment
#[utoipa::path(
    tag = "Deployment Approvals",
    get,
    path = "/projects/{project_id}/environments/{environment_id}/approval-policy",
    params(
        ("project_id" = i32, Path, description = "Project ID"),
        ("environment_id" = i32, Path, description = "Environment ID")
    ),
    responses(
        (status = 200, description = "The environment's policy", body = ApprovalPolicyResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Environment not found"),
        (status = 500, description = "Internal server error")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_approval_policy(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<ApprovalsAppState>>,
    Path((project_id, environment_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    project_scope_guard!(auth, project_id);
    project_permission_guard!(
        auth,
        ProjectsRead,
        project_id,
        &state.project_access_checker
    );
    let required = state
        .approval_service
        .policy(project_id, environment_id)
        .await?;
    Ok(Json(ApprovalPolicyResponse {
        environment_id,
        required,
    }))
}

/// Require (or stop requiring) approval before each deployment
///
/// Turning it off lets the deployments waiting there run.
#[utoipa::path(
    tag = "Deployment Approvals",
    put,
    path = "/projects/{project_id}/environments/{environment_id}/approval-policy",
    params(
        ("project_id" = i32, Path, description = "Project ID"),
        ("environment_id" = i32, Path, description = "Environment ID")
    ),
    request_body = SetApprovalPolicyRequest,
    responses(
        (status = 200, description = "Policy saved", body = ApprovalPolicyResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Environment not found"),
        (status = 500, description = "Internal server error")
    ),
    security(("bearer_auth" = []))
)]
pub async fn set_approval_policy(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<ApprovalsAppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Path((project_id, environment_id)): Path<(i32, i32)>,
    Json(request): Json<SetApprovalPolicyRequest>,
) -> Result<impl IntoResponse, Problem> {
    project_scope_guard!(auth, project_id);
    project_permission_guard!(
        auth,
        ProjectsWrite,
        project_id,
        &state.project_access_checker
    );
    let required = state
        .approval_service
        .set_policy(
            project_id,
            environment_id,
            request.required,
            Some(auth.user_id()),
        )
        .await?;
    write_audit(
        &state,
        &ApprovalPolicyUpdatedAudit {
            context: audit_context(&auth, &metadata),
            project_id,
            environment_id,
            required,
        },
    )
    .await;
    Ok(Json(ApprovalPolicyResponse {
        environment_id,
        required,
    }))
}

/// A deployment's approval request
#[utoipa::path(
    tag = "Deployment Approvals",
    get,
    path = "/projects/{project_id}/deployments/{deployment_id}/approval",
    params(
        ("project_id" = i32, Path, description = "Project ID"),
        ("deployment_id" = i32, Path, description = "Deployment ID")
    ),
    responses(
        (status = 200, description = "The request, or null when none was needed", body = DeploymentApprovalStatus),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Deployment not found"),
        (status = 500, description = "Internal server error")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_deployment_approval(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<ApprovalsAppState>>,
    Path((project_id, deployment_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    project_scope_guard!(auth, project_id);
    project_permission_guard!(
        auth,
        DeploymentsRead,
        project_id,
        &state.project_access_checker
    );
    let approval = state
        .approval_service
        .for_deployment(project_id, deployment_id)
        .await?;
    Ok(Json(DeploymentApprovalStatus {
        deployment_id,
        approval: approval.map(ApprovalResponse::from),
    }))
}

/// Approve or reject a waiting deployment
///
/// Approving runs it; rejecting cancels it. Each request is decided once.
#[utoipa::path(
    tag = "Deployment Approvals",
    post,
    path = "/projects/{project_id}/deployments/{deployment_id}/approval",
    params(
        ("project_id" = i32, Path, description = "Project ID"),
        ("deployment_id" = i32, Path, description = "Deployment ID")
    ),
    request_body = DecideApprovalRequest,
    responses(
        (status = 200, description = "ApprovalDecision recorded", body = ApprovalResponse),
        (status = 400, description = "Invalid comment"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "No approval request for this deployment"),
        (status = 409, description = "Already decided"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "The approved deployment could not be queued; it stays pending")
    ),
    security(("bearer_auth" = []))
)]
pub async fn decide_deployment_approval(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<ApprovalsAppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Path((project_id, deployment_id)): Path<(i32, i32)>,
    Json(request): Json<DecideApprovalRequest>,
) -> Result<impl IntoResponse, Problem> {
    project_scope_guard!(auth, project_id);
    project_permission_guard!(
        auth,
        DeploymentsWrite,
        project_id,
        &state.project_access_checker
    );
    let decided = state
        .approval_service
        .decide(
            project_id,
            deployment_id,
            request.decision,
            request.comment,
            Some(auth.user_id()),
        )
        .await?;
    write_audit(
        &state,
        &DeploymentApprovalDecidedAudit {
            context: audit_context(&auth, &metadata),
            project_id,
            deployment_id,
            status: decided.status.clone(),
            comment: decided.comment.clone(),
        },
    )
    .await;
    Ok(Json(ApprovalResponse::from(decided)))
}

/// A project's approval requests, newest first
#[utoipa::path(
    tag = "Deployment Approvals",
    get,
    path = "/projects/{project_id}/approvals",
    params(("project_id" = i32, Path, description = "Project ID"), ListApprovalsQuery),
    responses(
        (status = 200, description = "Approval requests", body = ApprovalListResponse),
        (status = 400, description = "Unknown status filter"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 500, description = "Internal server error")
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_approvals(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<ApprovalsAppState>>,
    Path(project_id): Path<i32>,
    Query(query): Query<ListApprovalsQuery>,
) -> Result<impl IntoResponse, Problem> {
    project_scope_guard!(auth, project_id);
    project_permission_guard!(
        auth,
        DeploymentsRead,
        project_id,
        &state.project_access_checker
    );
    let (items, total) = state
        .approval_service
        .list(
            project_id,
            query.status.as_deref(),
            query.page,
            query.page_size,
        )
        .await?;
    let (page, page_size) = normalize_pagination(query.page, query.page_size);
    Ok(Json(ApprovalListResponse {
        approvals: items.into_iter().map(ApprovalResponse::from).collect(),
        total,
        page,
        page_size,
    }))
}

pub fn configure_routes() -> Router<Arc<ApprovalsAppState>> {
    Router::new()
        .route(
            "/projects/{project_id}/environments/{environment_id}/approval-policy",
            get(get_approval_policy).put(set_approval_policy),
        )
        .route(
            "/projects/{project_id}/deployments/{deployment_id}/approval",
            get(get_deployment_approval).post(decide_deployment_approval),
        )
        .route("/projects/{project_id}/approvals", get(list_approvals))
}

#[derive(OpenApi)]
#[openapi(
    paths(
        get_approval_policy,
        set_approval_policy,
        get_deployment_approval,
        decide_deployment_approval,
        list_approvals,
    ),
    components(schemas(
        ApprovalPolicyResponse,
        SetApprovalPolicyRequest,
        ApprovalResponse,
        DeploymentApprovalStatus,
        DecideApprovalRequest,
        ApprovalListResponse,
        ApprovalDecision,
    )),
    tags((
        name = "Deployment Approvals",
        description = "Hold deployments to an environment until a person approves them"
    ))
)]
pub struct ApprovalsApiDoc;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn problem_status(error: ApprovalError) -> StatusCode {
        Problem::from(error).status_code
    }

    #[test]
    fn every_error_maps_to_a_status_with_its_ids_in_the_detail() {
        assert_eq!(
            problem_status(ApprovalError::ApprovalNotFound {
                project_id: 5,
                deployment_id: 42
            }),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            problem_status(ApprovalError::AlreadyDecided {
                deployment_id: 42,
                status: "approved".into()
            }),
            StatusCode::CONFLICT
        );
        assert_eq!(
            problem_status(ApprovalError::CommentTooLong { len: 600, max: 500 }),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            problem_status(ApprovalError::RecheckNotQueued {
                deployment_id: 42,
                reason: "offline".into()
            }),
            StatusCode::SERVICE_UNAVAILABLE
        );
        let body = serde_json::to_string(
            &Problem::from(ApprovalError::DeploymentNotFound {
                project_id: 5,
                deployment_id: 42,
            })
            .body,
        )
        .unwrap_or_default();
        assert!(body.contains("42") && body.contains('5'), "{body}");
    }

    #[tokio::test]
    async fn every_route_requires_authentication() {
        let state = Arc::new(ApprovalsAppState {
            approval_service: Arc::new(ApprovalService::new(
                Arc::new(
                    sea_orm::MockDatabase::new(sea_orm::DatabaseBackend::Postgres)
                        .into_connection(),
                ),
                Arc::new(NoQueue),
                Arc::new(NoCancel),
            )),
            audit_service: Arc::new(NoAudit),
            project_access_checker: None,
        });
        for (method, uri) in [
            ("GET", "/projects/5/environments/7/approval-policy"),
            ("PUT", "/projects/5/environments/7/approval-policy"),
            ("GET", "/projects/5/deployments/42/approval"),
            ("POST", "/projects/5/deployments/42/approval"),
            ("GET", "/projects/5/approvals"),
        ] {
            let response = configure_routes()
                .with_state(state.clone())
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header("content-type", "application/json")
                        .body(Body::from("{}"))
                        .expect("request"),
                )
                .await
                .expect("router answers");
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri}"
            );
        }
    }

    struct NoQueue;
    #[async_trait::async_trait]
    impl temps_core::JobQueue for NoQueue {
        async fn send(&self, _job: temps_core::Job) -> Result<(), temps_core::QueueError> {
            Ok(())
        }
        fn subscribe(&self) -> Box<dyn temps_core::JobReceiver> {
            unimplemented!("not used")
        }
    }

    struct NoCancel;
    #[async_trait::async_trait]
    impl crate::service::DeploymentCanceller for NoCancel {
        async fn cancel(&self, _project_id: i32, _deployment_id: i32) -> Result<(), String> {
            Ok(())
        }
    }

    struct NoAudit;
    #[async_trait::async_trait]
    impl AuditLogger for NoAudit {
        async fn create_audit_log(
            &self,
            _operation: &dyn AuditOperation,
        ) -> temps_core::AuditResult<()> {
            Ok(())
        }
    }
}
