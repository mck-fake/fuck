// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! An operator-named set of project-scoped permission strings that a team
//! membership can hold instead of its fixed role. Validation lives in
//! `temps_teams::custom_roles`.

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use sea_orm::FromJsonQueryResult;
use serde::{Deserialize, Serialize};

/// JSON column wrapper for the permission strings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, FromJsonQueryResult)]
pub struct RolePermissions(pub Vec<String>);

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "custom_roles")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    #[sea_orm(column_type = "JsonBinary")]
    pub permissions: RolePermissions,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
