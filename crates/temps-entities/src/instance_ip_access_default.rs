// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The instance-wide default client-IP policy: applies to every project and
//! environment without a `project_ip_access_rules` row of its own. At most one
//! row (`id = 1`). Validation lives in the proxy's IP access service, as for
//! project rules.

use crate::project_ip_access_rules::IpAccessCidrs;
use async_trait::async_trait;
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue::Set, ConnectionTrait, DbErr};
use serde::{Deserialize, Serialize};
use temps_core::DBDateTime;

/// The only id the table accepts.
pub const SINGLETON_ID: i32 = 1;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "instance_ip_access_default")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i32,
    /// `disabled`, `allow` or `deny`.
    pub mode: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub cidrs: IpAccessCidrs,
    pub updated_by: Option<i32>,
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
        // The proxy detects a change by `updated_at`, so every save bumps it.
        self.updated_at = Set(now);
        Ok(self)
    }
}
