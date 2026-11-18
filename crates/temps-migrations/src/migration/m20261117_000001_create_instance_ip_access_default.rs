// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The instance-wide default client-IP policy for projects.
//!
//! It applies to every project and environment that has no row of its own in
//! `project_ip_access_rules`; a project or environment row, even one with
//! `mode = 'disabled'`, takes precedence and so is how a project opts out.
//! At most one row exists (`id = 1`); no row means no default, which is what
//! every existing installation gets, so nothing changes on upgrade.
//!
//! A table rather than a key in `settings.data`: the generic settings write
//! path does not know CIDR validation, and the proxy can poll one indexed row
//! without parsing the whole settings document.

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
CREATE TABLE IF NOT EXISTS instance_ip_access_default (
    id INTEGER PRIMARY KEY DEFAULT 1,
    -- 'disabled' keeps the entries but enforces nothing.
    mode VARCHAR(16) NOT NULL DEFAULT 'disabled',
    -- Same shape and validation as project_ip_access_rules.cidrs.
    cidrs JSONB NOT NULL DEFAULT '[]'::jsonb,
    updated_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT instance_ip_access_default_single_row CHECK (id = 1),
    CONSTRAINT instance_ip_access_default_mode_valid
        CHECK (mode IN ('disabled', 'allow', 'deny'))
);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS instance_ip_access_default")
            .await?;
        Ok(())
    }
}
