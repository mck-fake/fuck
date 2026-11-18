// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Which custom role, if any, a team membership holds instead of its fixed
//! role. One row per membership; deleting the membership removes it, and a
//! role cannot be deleted while assigned (`ON DELETE RESTRICT`).

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "team_member_custom_roles")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub team_member_id: i32,
    pub custom_role_id: i32,
    pub assigned_by: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::custom_roles::Entity",
        from = "Column::CustomRoleId",
        to = "super::custom_roles::Column::Id",
        on_delete = "Restrict"
    )]
    CustomRole,
    #[sea_orm(
        belongs_to = "super::team_members::Entity",
        from = "Column::TeamMemberId",
        to = "super::team_members::Column::Id",
        on_delete = "Cascade"
    )]
    TeamMember,
}

impl Related<super::custom_roles::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::CustomRole.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
