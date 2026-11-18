// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Immutable snapshots of a project's edge rules (ADR-089 A1), one per write,
//! numbered per project. `rules` is the whole set in evaluation order, kept
//! as raw JSON for the same reason as `project_edge_rules`: the service
//! decodes and validates it when a revision is restored.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use temps_core::DBDateTime;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "project_edge_rule_revisions")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub project_id: i32,
    pub number: i32,
    #[sea_orm(column_type = "JsonBinary")]
    pub rules: Json,
    pub change: String,
    pub created_by: Option<i32>,
    pub created_at: DBDateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "crate::projects::Entity",
        from = "Column::ProjectId",
        to = "crate::projects::Column::Id",
        on_delete = "Cascade"
    )]
    Project,
}

impl Related<crate::projects::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Project.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
