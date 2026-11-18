// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Approval policy, the deployment gate decision, and approve/reject.

use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use temps_core::{DeploymentGateRecheckJob, GateDecision, Job, JobQueue};
use temps_entities::deployment_approvals::{
    self as approvals, STATUS_APPROVED, STATUS_PENDING, STATUS_REJECTED,
};
use temps_entities::{deployments, environment_approval_policies as policies, environments};
use tracing::{info, warn};
use utoipa::ToSchema;

use crate::error::ApprovalError;

pub const MAX_COMMENT_LEN: usize = 500;
pub const DEFAULT_PAGE_SIZE: u64 = 20;
pub const MAX_PAGE_SIZE: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    Approve,
    Reject,
}

/// Cancels a rejected deployment. Implemented by the deployments service;
/// a trait so the approval logic is testable without the deployment stack.
#[async_trait]
pub trait DeploymentCanceller: Send + Sync {
    async fn cancel(&self, project_id: i32, deployment_id: i32) -> Result<(), String>;
}

#[async_trait]
impl DeploymentCanceller for temps_deployments::DeploymentService {
    async fn cancel(&self, project_id: i32, deployment_id: i32) -> Result<(), String> {
        self.cancel_deployment(project_id, deployment_id)
            .await
            .map_err(|error| error.to_string())
    }
}

pub struct ApprovalService {
    db: Arc<DatabaseConnection>,
    queue: Arc<dyn JobQueue>,
    canceller: Arc<dyn DeploymentCanceller>,
}

pub fn normalize_pagination(page: Option<u64>, page_size: Option<u64>) -> (u64, u64) {
    (
        page.unwrap_or(1).max(1),
        page_size
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE),
    )
}

impl ApprovalService {
    pub fn new(
        db: Arc<DatabaseConnection>,
        queue: Arc<dyn JobQueue>,
        canceller: Arc<dyn DeploymentCanceller>,
    ) -> Self {
        Self {
            db,
            queue,
            canceller,
        }
    }

    async fn ensure_environment(
        &self,
        project_id: i32,
        environment_id: i32,
    ) -> Result<(), ApprovalError> {
        environments::Entity::find_by_id(environment_id)
            .filter(environments::Column::ProjectId.eq(project_id))
            .filter(environments::Column::DeletedAt.is_null())
            .select_only()
            .column(environments::Column::Id)
            .into_tuple::<i32>()
            .one(self.db.as_ref())
            .await?
            .map(|_| ())
            .ok_or(ApprovalError::EnvironmentNotFound {
                project_id,
                environment_id,
            })
    }

    /// The environment of a deployment of this project. Reads one column:
    /// a deployment row carries large JSON this never needs.
    async fn deployment_environment(
        &self,
        project_id: i32,
        deployment_id: i32,
    ) -> Result<i32, ApprovalError> {
        deployments::Entity::find_by_id(deployment_id)
            .filter(deployments::Column::ProjectId.eq(project_id))
            .select_only()
            .column(deployments::Column::EnvironmentId)
            .into_tuple::<i32>()
            .one(self.db.as_ref())
            .await?
            .ok_or(ApprovalError::DeploymentNotFound {
                project_id,
                deployment_id,
            })
    }

    async fn required(&self, environment_id: i32) -> Result<bool, ApprovalError> {
        Ok(policies::Entity::find_by_id(environment_id)
            .one(self.db.as_ref())
            .await?
            .is_some_and(|policy| policy.required))
    }

    /// Whether the environment holds deployments for approval.
    pub async fn policy(
        &self,
        project_id: i32,
        environment_id: i32,
    ) -> Result<bool, ApprovalError> {
        self.ensure_environment(project_id, environment_id).await?;
        self.required(environment_id).await
    }

    /// Turn the requirement on or off. Turning it off releases the
    /// deployments still waiting there: their requests are dropped and each
    /// is queued to run.
    pub async fn set_policy(
        &self,
        project_id: i32,
        environment_id: i32,
        required: bool,
        updated_by: Option<i32>,
    ) -> Result<bool, ApprovalError> {
        self.ensure_environment(project_id, environment_id).await?;
        let now = chrono::Utc::now();
        policies::Entity::insert(policies::ActiveModel {
            environment_id: Set(environment_id),
            required: Set(required),
            updated_by: Set(updated_by),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .on_conflict(
            OnConflict::column(policies::Column::EnvironmentId)
                .update_columns([
                    policies::Column::Required,
                    policies::Column::UpdatedBy,
                    policies::Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(self.db.as_ref())
        .await?;

        if !required {
            let waiting = approvals::Entity::find()
                .filter(approvals::Column::EnvironmentId.eq(environment_id))
                .filter(approvals::Column::Status.eq(STATUS_PENDING))
                .all(self.db.as_ref())
                .await?;
            for request in waiting {
                approvals::Entity::delete_by_id(request.id)
                    .exec(self.db.as_ref())
                    .await?;
                // Best effort: a deployment whose recheck is lost is retried
                // by redeploying; the policy change itself has succeeded.
                if let Err(error) = self.queue_recheck(request.deployment_id).await {
                    warn!(
                        deployment_id = request.deployment_id,
                        environment_id,
                        "Approval no longer required, but the waiting deployment could not be queued: {error}"
                    );
                }
            }
        }
        info!(
            project_id,
            environment_id, required, "Deployment approval policy updated"
        );
        Ok(required)
    }

    async fn queue_recheck(&self, deployment_id: i32) -> Result<(), ApprovalError> {
        self.queue
            .send(Job::DeploymentGateRecheck(DeploymentGateRecheckJob {
                deployment_id,
            }))
            .await
            .map_err(|error| ApprovalError::RecheckNotQueued {
                deployment_id,
                reason: error.to_string(),
            })
    }

    /// The gate's answer for one deployment. When approval is required and
    /// nothing is decided yet, this opens the pending request.
    pub async fn gate(
        &self,
        project_id: i32,
        deployment_id: i32,
    ) -> Result<GateDecision, ApprovalError> {
        let environment_id = self
            .deployment_environment(project_id, deployment_id)
            .await?;
        if !self.required(environment_id).await? {
            return Ok(GateDecision::Allow);
        }
        let existing = approvals::Entity::find()
            .filter(approvals::Column::DeploymentId.eq(deployment_id))
            .one(self.db.as_ref())
            .await?;
        match existing.as_ref().map(|request| request.status.as_str()) {
            Some(STATUS_APPROVED) => return Ok(GateDecision::Allow),
            Some(STATUS_REJECTED) => {
                return Ok(GateDecision::Block {
                    reason: "The deployment was rejected".to_string(),
                })
            }
            Some(_) => {}
            None => {
                let now = chrono::Utc::now();
                approvals::Entity::insert(approvals::ActiveModel {
                    deployment_id: Set(deployment_id),
                    project_id: Set(project_id),
                    environment_id: Set(environment_id),
                    status: Set(STATUS_PENDING.to_string()),
                    created_at: Set(now),
                    updated_at: Set(now),
                    ..Default::default()
                })
                // A recheck racing the first check must not fail on the
                // unique deployment_id.
                .on_conflict(
                    OnConflict::column(approvals::Column::DeploymentId)
                        .do_nothing()
                        .to_owned(),
                )
                .exec_without_returning(self.db.as_ref())
                .await?;
                info!(project_id, deployment_id, "Deployment waiting for approval");
            }
        }
        Ok(GateDecision::Block {
            reason: "Waiting for approval".to_string(),
        })
    }

    /// The approval request of a deployment, if it has one.
    pub async fn for_deployment(
        &self,
        project_id: i32,
        deployment_id: i32,
    ) -> Result<Option<approvals::Model>, ApprovalError> {
        self.deployment_environment(project_id, deployment_id)
            .await?;
        Ok(approvals::Entity::find()
            .filter(approvals::Column::DeploymentId.eq(deployment_id))
            .one(self.db.as_ref())
            .await?)
    }

    /// Approve or reject a pending request, exactly once. An approval is
    /// queued to run inside the same transaction, so a failure to queue
    /// leaves the request pending; a rejection cancels the deployment.
    // shortcut: anyone with deployments:write may approve, including whoever
    // started the deployment; deployments do not record who triggered them.
    // Add a self-approval check once they do.
    pub async fn decide(
        &self,
        project_id: i32,
        deployment_id: i32,
        decision: ApprovalDecision,
        comment: Option<String>,
        decided_by: Option<i32>,
    ) -> Result<approvals::Model, ApprovalError> {
        let comment = comment
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty());
        if let Some(len) = comment.as_ref().map(|c| c.chars().count()) {
            if len > MAX_COMMENT_LEN {
                return Err(ApprovalError::CommentTooLong {
                    len,
                    max: MAX_COMMENT_LEN,
                });
            }
        }
        let request = approvals::Entity::find()
            .filter(approvals::Column::DeploymentId.eq(deployment_id))
            .filter(approvals::Column::ProjectId.eq(project_id))
            .one(self.db.as_ref())
            .await?
            .ok_or(ApprovalError::ApprovalNotFound {
                project_id,
                deployment_id,
            })?;
        if request.status != STATUS_PENDING {
            return Err(ApprovalError::AlreadyDecided {
                deployment_id,
                status: request.status,
            });
        }
        let status = match decision {
            ApprovalDecision::Approve => STATUS_APPROVED,
            ApprovalDecision::Reject => STATUS_REJECTED,
        };

        let txn = self.db.begin().await?;
        // Guarded on `pending` so two people deciding at once cannot both win.
        let updated = approvals::Entity::update_many()
            .col_expr(approvals::Column::Status, Expr::value(status))
            .col_expr(approvals::Column::DecidedBy, Expr::value(decided_by))
            .col_expr(
                approvals::Column::DecidedAt,
                Expr::value(Some(chrono::Utc::now())),
            )
            .col_expr(approvals::Column::Comment, Expr::value(comment))
            .col_expr(
                approvals::Column::UpdatedAt,
                Expr::value(chrono::Utc::now()),
            )
            .filter(approvals::Column::Id.eq(request.id))
            .filter(approvals::Column::Status.eq(STATUS_PENDING))
            .exec(&txn)
            .await?;
        if updated.rows_affected == 0 {
            return Err(ApprovalError::AlreadyDecided {
                deployment_id,
                status: "decided by someone else".to_string(),
            });
        }
        if decision == ApprovalDecision::Approve {
            self.queue_recheck(deployment_id).await?;
        }
        txn.commit().await?;

        if decision == ApprovalDecision::Reject {
            self.canceller
                .cancel(project_id, deployment_id)
                .await
                .map_err(|reason| ApprovalError::CancelFailed {
                    deployment_id,
                    reason,
                })?;
        }
        info!(
            project_id,
            deployment_id, status, "Deployment approval decided"
        );
        approvals::Entity::find_by_id(request.id)
            .one(self.db.as_ref())
            .await?
            .ok_or(ApprovalError::ApprovalNotFound {
                project_id,
                deployment_id,
            })
    }

    /// A project's approval requests, newest first, optionally by status.
    pub async fn list(
        &self,
        project_id: i32,
        status: Option<&str>,
        page: Option<u64>,
        page_size: Option<u64>,
    ) -> Result<(Vec<approvals::Model>, u64), ApprovalError> {
        let mut query = approvals::Entity::find()
            .filter(approvals::Column::ProjectId.eq(project_id))
            .order_by_desc(approvals::Column::CreatedAt);
        if let Some(status) = status {
            if ![STATUS_PENDING, STATUS_APPROVED, STATUS_REJECTED].contains(&status) {
                return Err(ApprovalError::InvalidStatus {
                    value: status.chars().take(32).collect(),
                });
            }
            query = query.filter(approvals::Column::Status.eq(status));
        }
        let (page, page_size) = normalize_pagination(page, page_size);
        let paginator = query.paginate(self.db.as_ref(), page_size);
        let total = paginator.num_items().await?;
        let items = paginator.fetch_page(page - 1).await?;
        Ok((items, total))
    }
}

/// The `DeploymentGate` the deployments plugin consults before a deployment
/// runs. Errors block (the core treats them as fail-closed).
pub struct ApprovalGate {
    service: Arc<ApprovalService>,
}

impl ApprovalGate {
    pub fn new(service: Arc<ApprovalService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl temps_core::DeploymentGate for ApprovalGate {
    async fn check(
        &self,
        project_id: i32,
        _environment_name: &str,
        deployment_id: &str,
    ) -> Result<GateDecision, Box<dyn std::error::Error + Send + Sync>> {
        let deployment_id: i32 =
            deployment_id
                .parse()
                .map_err(|_| ApprovalError::InvalidDeploymentId {
                    value: deployment_id.chars().take(32).collect(),
                })?;
        Ok(self.service.gate(project_id, deployment_id).await?)
    }
}

#[cfg(test)]
mod tests;
