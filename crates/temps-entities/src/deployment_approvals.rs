// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! One approval request per gated deployment: `pending` until a person moves
//! it once to `approved` or `rejected`.

use async_trait::async_trait;
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue::Set, ConnectionTrait, DbErr};
use serde::{Deserialize, Serialize};
use temps_core::DBDateTime;

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_APPROVED: &str = "approved";
pub const STATUS_REJECTED: &str = "rejected";

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "deployment_approvals")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub deployment_id: i32,
    pub project_id: i32,
    pub environment_id: i32,
    /// `pending`, `approved` or `rejected`.
    pub status: String,
    pub decided_by: Option<i32>,
    pub decided_at: Option<DBDateTime>,
    pub comment: Option<String>,
    pub created_at: DBDateTime,
    pub updated_at: DBDateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

#[async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(mut self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let now = chrono::Utc::now();
        if insert && self.created_at.is_not_set() {
            self.created_at = Set(now);
        }
        self.updated_at = Set(now);
        Ok(self)
    }
}
