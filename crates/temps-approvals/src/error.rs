// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApprovalError {
    #[error("Environment {environment_id} not found in project {project_id}")]
    EnvironmentNotFound {
        project_id: i32,
        environment_id: i32,
    },
    #[error("Deployment {deployment_id} not found in project {project_id}")]
    DeploymentNotFound { project_id: i32, deployment_id: i32 },
    #[error("Deployment {deployment_id} of project {project_id} has no approval request")]
    ApprovalNotFound { project_id: i32, deployment_id: i32 },
    #[error("The approval request for deployment {deployment_id} was already {status}")]
    AlreadyDecided { deployment_id: i32, status: String },
    #[error("Approval comment is {len} characters; the limit is {max}")]
    CommentTooLong { len: usize, max: usize },
    #[error("Unknown approval status '{value}'; use pending, approved or rejected")]
    InvalidStatus { value: String },
    #[error("The deployment gate was asked about '{value}', which is not a deployment id")]
    InvalidDeploymentId { value: String },
    #[error(
        "Deployment {deployment_id} could not be queued to run, so it was left pending: {reason}. \
         Approve it again"
    )]
    RecheckNotQueued { deployment_id: i32, reason: String },
    #[error("Deployment {deployment_id} was rejected, but cancelling it failed: {reason}")]
    CancelFailed { deployment_id: i32, reason: String },
    #[error("Database error in deployment approvals: {0}")]
    Database(#[from] sea_orm::DbErr),
}
