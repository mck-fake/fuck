// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Custom team roles: an operator-named set of project-scoped permission
//! strings that a team membership can hold instead of its fixed role.
//!
//! The assignment lives in `team_member_custom_roles`, not as a column on
//! `team_members`: core keeps the fixed role as the membership's own answer
//! and lets the `MembershipPermissionResolver` override it, keyed on the
//! membership id (see the `team_members` entity doc).
//!
//! `custom_role_id` is `ON DELETE RESTRICT`: deleting a role still assigned
//! is refused rather than silently dropping members back to their fixed
//! role, which could widen what they may do. Deleting a membership cascades
//! its assignment away. No existing rows are affected.

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
CREATE TABLE IF NOT EXISTS custom_roles (
    id SERIAL PRIMARY KEY,
    name VARCHAR(64) NOT NULL,
    description TEXT,
    -- JSON array of permission strings, e.g. ["deployments:read"]; the
    -- service validates every entry against the project-scoped set.
    permissions JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX IF NOT EXISTS custom_roles_name_unique ON custom_roles (lower(name));

CREATE TABLE IF NOT EXISTS team_member_custom_roles (
    team_member_id INTEGER PRIMARY KEY REFERENCES team_members(id) ON DELETE CASCADE,
    custom_role_id INTEGER NOT NULL REFERENCES custom_roles(id) ON DELETE RESTRICT,
    assigned_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS team_member_custom_roles_role ON team_member_custom_roles (custom_role_id);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS team_member_custom_roles; DROP TABLE IF EXISTS custom_roles;",
            )
            .await?;
        Ok(())
    }
}
