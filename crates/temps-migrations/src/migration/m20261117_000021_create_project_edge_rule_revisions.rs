// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Version history of each project's edge rules (ADR-089 adjustment A1).
//!
//! Every write to a project's rules stores, in the same transaction, an
//! immutable snapshot of the whole rule set in evaluation order, numbered per
//! project. Restoring a revision writes its rules back and records a new
//! revision. Only the last 100 revisions per project are kept: the service
//! prunes older ones on every write, so the table stays bounded at
//! 100 x (at most 100 rules) per project.
//!
//! Existing projects get revision 1, a snapshot of their rules as they are
//! when this migration runs, so history starts from today's state rather than
//! from the first edit after the upgrade. Nothing reads this table on the
//! request path.

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
CREATE TABLE IF NOT EXISTS project_edge_rule_revisions (
    id SERIAL PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    -- 1, 2, 3... per project.
    number INTEGER NOT NULL,
    -- The whole rule set in evaluation order: [{id, environment_id, name,
    -- enabled, mode, expression, action}].
    rules JSONB NOT NULL,
    -- What the write did, e.g. "Created rule 'Block /admin'".
    change VARCHAR(200) NOT NULL,
    created_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT project_edge_rule_revisions_number_unique UNIQUE (project_id, number),
    CONSTRAINT project_edge_rule_revisions_rules_is_array CHECK (jsonb_typeof(rules) = 'array')
);

INSERT INTO project_edge_rule_revisions (project_id, number, rules, change)
SELECT project_id, 1,
       jsonb_agg(jsonb_build_object(
           'id', id, 'environment_id', environment_id, 'name', name,
           'enabled', enabled, 'mode', mode, 'expression', expression, 'action', action)
         ORDER BY position, id),
       'Rules as they were when version history started'
FROM project_edge_rules
GROUP BY project_id
ON CONFLICT (project_id, number) DO NOTHING;
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS project_edge_rule_revisions")
            .await?;
        Ok(())
    }
}
