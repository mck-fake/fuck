// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Deployment approvals: an environment can require a person to approve each
//! deployment before it runs.
//!
//! `environment_approval_policies` holds the per-environment switch; no row
//! means no approval is required, which is what every existing environment
//! gets, so nothing changes on upgrade. A table rather than a column on
//! `environments` keeps the policy owned by the approvals plugin.
//!
//! `deployment_approvals` holds one request per gated deployment: created
//! `pending` by the deployment gate, then moved once to `approved` or
//! `rejected`. Rows go with their deployment.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
CREATE TABLE IF NOT EXISTS environment_approval_policies (
    environment_id INTEGER PRIMARY KEY REFERENCES environments(id) ON DELETE CASCADE,
    required BOOLEAN NOT NULL DEFAULT false,
    updated_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS deployment_approvals (
    id SERIAL PRIMARY KEY,
    deployment_id INTEGER NOT NULL UNIQUE REFERENCES deployments(id) ON DELETE CASCADE,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    environment_id INTEGER NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    status VARCHAR(16) NOT NULL DEFAULT 'pending',
    decided_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    decided_at TIMESTAMPTZ,
    comment TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT deployment_approvals_status_valid
        CHECK (status IN ('pending', 'approved', 'rejected'))
);

CREATE INDEX IF NOT EXISTS idx_deployment_approvals_project_status
    ON deployment_approvals (project_id, status, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_deployment_approvals_environment_status
    ON deployment_approvals (environment_id, status);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS deployment_approvals; \
                 DROP TABLE IF EXISTS environment_approval_policies;",
            )
            .await?;
        Ok(())
    }
}
