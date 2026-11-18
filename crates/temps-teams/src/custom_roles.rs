// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Custom team roles: an operator-named set of project-scoped permissions a
//! membership can hold instead of its fixed `owner`/`admin`/`deployer`/
//! `viewer` role.
//!
//! [`CustomRoleResolver`] answers [`MembershipPermissionResolver`] from
//! `team_member_custom_roles`; core still intersects that answer with the
//! team's grant on each project, so a custom role can narrow a member below
//! their team's access but never widen them past it. Only permissions some
//! fixed role can carry are accepted, so no custom role ever names an
//! instance-admin permission.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use axum::http::StatusCode;
use chrono::Utc;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use temps_auth::permissions::Permission;
use temps_core::problemdetails::{self, Problem};
use temps_core::MembershipPermissionResolver;
use temps_entities::custom_roles::{self, RolePermissions};
use temps_entities::{team_member_custom_roles, team_members, TeamRole};
use thiserror::Error;
use utoipa::ToSchema;

use crate::checker::TeamProjectAccessChecker;
use crate::role_permissions::fixed_role_permissions;
use crate::service::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};

pub const MAX_NAME_LEN: usize = 64;
pub const MAX_DESCRIPTION_LEN: usize = 500;

#[derive(Error, Debug)]
pub enum CustomRoleError {
    #[error("Custom role {role_id} not found")]
    NotFound { role_id: i32 },

    #[error("Team member (team {team_id}, user {user_id}) not found")]
    MemberNotFound { team_id: i32, user_id: i32 },

    #[error("Team member (team {team_id}, user {user_id}) holds no custom role")]
    NotAssigned { team_id: i32, user_id: i32 },

    #[error("A custom role named '{name}' already exists")]
    NameTaken { name: String },

    #[error(
        "Custom role {role_id} is assigned to {members} team member(s); move them to another \
         role first, so nobody silently falls back to their fixed role"
    )]
    InUse { role_id: i32, members: u64 },

    #[error("A custom role needs a name")]
    EmptyName,

    #[error("Custom role name is {len} characters; at most {max} are allowed")]
    NameTooLong { len: usize, max: usize },

    #[error("Custom role description is {len} characters; at most {max} are allowed")]
    DescriptionTooLong { len: usize, max: usize },

    #[error("'{permission}' is not a known permission")]
    UnknownPermission { permission: String },

    #[error(
        "'{permission}' is not a project-scoped permission; a custom role can only carry \
         permissions a team role can hold on a project"
    )]
    NotProjectScoped { permission: String },

    #[error("A custom role needs at least one permission")]
    NoPermissions,

    #[error("Database error: {0}")]
    Database(#[from] sea_orm::DbErr),
}

impl From<CustomRoleError> for Problem {
    fn from(error: CustomRoleError) -> Self {
        match error {
            CustomRoleError::NotFound { .. }
            | CustomRoleError::MemberNotFound { .. }
            | CustomRoleError::NotAssigned { .. } => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Resource Not Found")
                .with_detail(error.to_string()),
            CustomRoleError::NameTaken { .. } | CustomRoleError::InUse { .. } => {
                problemdetails::new(StatusCode::CONFLICT)
                    .with_title("Resource Conflict")
                    .with_detail(error.to_string())
            }
            CustomRoleError::EmptyName
            | CustomRoleError::NameTooLong { .. }
            | CustomRoleError::DescriptionTooLong { .. }
            | CustomRoleError::UnknownPermission { .. }
            | CustomRoleError::NotProjectScoped { .. }
            | CustomRoleError::NoPermissions => problemdetails::new(StatusCode::BAD_REQUEST)
                .with_title("Invalid Custom Role")
                .with_detail(error.to_string()),
            CustomRoleError::Database(_) => problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Internal Server Error")
                .with_detail(error.to_string()),
        }
    }
}

/// Body of create and update: the whole role.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CustomRoleRequest {
    #[schema(example = "Release manager")]
    pub name: String,
    pub description: Option<String>,
    /// Permission strings, e.g. `deployments:create`. Must be project-scoped.
    #[schema(example = json!(["projects:read", "deployments:read", "deployments:create"]))]
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AssignCustomRoleRequest {
    pub custom_role_id: i32,
}

/// One membership's custom role, for showing it next to the fixed role.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CustomRoleAssignment {
    pub team_member_id: i32,
    pub user_id: i32,
    pub custom_role_id: i32,
    pub custom_role_name: String,
}

/// Every permission a custom role may carry: the owner role's set, which
/// every other fixed role is a subset of. Sorted, as permission strings.
pub fn assignable_permissions() -> Vec<String> {
    let set: BTreeSet<String> = fixed_role_permissions(TeamRole::Owner)
        .into_iter()
        .map(|p| p.to_string())
        .collect();
    set.into_iter().collect()
}

struct Validated {
    name: String,
    description: Option<String>,
    permissions: Vec<String>,
}

/// Trim, bound and canonicalise a request. Pure: no database access.
fn validate(req: CustomRoleRequest) -> Result<Validated, CustomRoleError> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(CustomRoleError::EmptyName);
    }
    let len = name.chars().count();
    if len > MAX_NAME_LEN {
        return Err(CustomRoleError::NameTooLong {
            len,
            max: MAX_NAME_LEN,
        });
    }
    let description = req
        .description
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());
    if let Some(d) = &description {
        let len = d.chars().count();
        if len > MAX_DESCRIPTION_LEN {
            return Err(CustomRoleError::DescriptionTooLong {
                len,
                max: MAX_DESCRIPTION_LEN,
            });
        }
    }
    let assignable: BTreeSet<String> = assignable_permissions().into_iter().collect();
    let mut permissions = BTreeSet::new();
    for raw in req.permissions {
        let raw = raw.trim().to_string();
        let Some(permission) = Permission::from_str(&raw) else {
            return Err(CustomRoleError::UnknownPermission { permission: raw });
        };
        let canonical = permission.to_string();
        if !assignable.contains(&canonical) {
            return Err(CustomRoleError::NotProjectScoped {
                permission: canonical,
            });
        }
        permissions.insert(canonical);
    }
    if permissions.is_empty() {
        return Err(CustomRoleError::NoPermissions);
    }
    Ok(Validated {
        name,
        description,
        permissions: permissions.into_iter().collect(),
    })
}

pub struct CustomRoleService {
    db: Arc<DatabaseConnection>,
    checker: Arc<TeamProjectAccessChecker>,
}

impl CustomRoleService {
    pub fn new(db: Arc<DatabaseConnection>, checker: Arc<TeamProjectAccessChecker>) -> Self {
        Self { db, checker }
    }

    /// Newest first. `page` is 1-indexed; `page_size` defaults to 20, max 100.
    pub async fn list(
        &self,
        page: Option<u64>,
        page_size: Option<u64>,
    ) -> Result<(Vec<custom_roles::Model>, u64), CustomRoleError> {
        let page = page.unwrap_or(1).max(1);
        let page_size = page_size
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        let paginator = custom_roles::Entity::find()
            .order_by_desc(custom_roles::Column::CreatedAt)
            .paginate(self.db.as_ref(), page_size);
        let total = paginator.num_items().await?;
        let items = paginator.fetch_page(page - 1).await?;
        Ok((items, total))
    }

    pub async fn get(&self, role_id: i32) -> Result<custom_roles::Model, CustomRoleError> {
        custom_roles::Entity::find_by_id(role_id)
            .one(self.db.as_ref())
            .await?
            .ok_or(CustomRoleError::NotFound { role_id })
    }

    async fn ensure_name_free(
        &self,
        name: &str,
        except: Option<i32>,
    ) -> Result<(), CustomRoleError> {
        let mut query = custom_roles::Entity::find().filter(
            sea_orm::sea_query::Expr::cust_with_values("lower(name) = lower($1)", [name]),
        );
        if let Some(id) = except {
            query = query.filter(custom_roles::Column::Id.ne(id));
        }
        if query.one(self.db.as_ref()).await?.is_some() {
            return Err(CustomRoleError::NameTaken {
                name: name.to_string(),
            });
        }
        Ok(())
    }

    pub async fn create(
        &self,
        req: CustomRoleRequest,
    ) -> Result<custom_roles::Model, CustomRoleError> {
        let v = validate(req)?;
        self.ensure_name_free(&v.name, None).await?;
        let now = Utc::now();
        let saved = custom_roles::ActiveModel {
            name: Set(v.name.clone()),
            description: Set(v.description),
            permissions: Set(RolePermissions(v.permissions)),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(self.db.as_ref())
        .await
        .map_err(|e| name_conflict(e, &v.name))?;
        Ok(saved)
    }

    /// Replace a role. Every member holding it resolves to the new set at
    /// once: the checker's caches are flushed.
    pub async fn update(
        &self,
        role_id: i32,
        req: CustomRoleRequest,
    ) -> Result<custom_roles::Model, CustomRoleError> {
        let v = validate(req)?;
        let existing = self.get(role_id).await?;
        self.ensure_name_free(&v.name, Some(role_id)).await?;
        let mut active: custom_roles::ActiveModel = existing.into();
        active.name = Set(v.name.clone());
        active.description = Set(v.description);
        active.permissions = Set(RolePermissions(v.permissions));
        active.updated_at = Set(Utc::now());
        let saved = active
            .update(self.db.as_ref())
            .await
            .map_err(|e| name_conflict(e, &v.name))?;
        self.checker.invalidate_all();
        Ok(saved)
    }

    /// Refused while any membership holds the role (see [`CustomRoleError::InUse`]).
    pub async fn delete(&self, role_id: i32) -> Result<(), CustomRoleError> {
        self.get(role_id).await?;
        let members = team_member_custom_roles::Entity::find()
            .filter(team_member_custom_roles::Column::CustomRoleId.eq(role_id))
            .count(self.db.as_ref())
            .await?;
        if members > 0 {
            return Err(CustomRoleError::InUse { role_id, members });
        }
        custom_roles::Entity::delete_by_id(role_id)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn membership(
        &self,
        team_id: i32,
        user_id: i32,
    ) -> Result<team_members::Model, CustomRoleError> {
        team_members::Entity::find()
            .filter(team_members::Column::TeamId.eq(team_id))
            .filter(team_members::Column::UserId.eq(user_id))
            .one(self.db.as_ref())
            .await?
            .ok_or(CustomRoleError::MemberNotFound { team_id, user_id })
    }

    /// Give a membership a custom role (replacing any previous one).
    pub async fn assign(
        &self,
        team_id: i32,
        user_id: i32,
        role_id: i32,
        assigned_by: i32,
    ) -> Result<CustomRoleAssignment, CustomRoleError> {
        let member = self.membership(team_id, user_id).await?;
        let role = self.get(role_id).await?;
        team_member_custom_roles::Entity::insert(team_member_custom_roles::ActiveModel {
            team_member_id: Set(member.id),
            custom_role_id: Set(role.id),
            assigned_by: Set(Some(assigned_by)),
            created_at: Set(Utc::now()),
        })
        .on_conflict(
            OnConflict::column(team_member_custom_roles::Column::TeamMemberId)
                .update_columns([
                    team_member_custom_roles::Column::CustomRoleId,
                    team_member_custom_roles::Column::AssignedBy,
                ])
                .to_owned(),
        )
        .exec_without_returning(self.db.as_ref())
        .await?;
        self.checker.invalidate_user(user_id).await;
        Ok(CustomRoleAssignment {
            team_member_id: member.id,
            user_id,
            custom_role_id: role.id,
            custom_role_name: role.name,
        })
    }

    /// Return a membership to its fixed role.
    pub async fn unassign(&self, team_id: i32, user_id: i32) -> Result<(), CustomRoleError> {
        let member = self.membership(team_id, user_id).await?;
        let result = team_member_custom_roles::Entity::delete_by_id(member.id)
            .exec(self.db.as_ref())
            .await?;
        if result.rows_affected == 0 {
            return Err(CustomRoleError::NotAssigned { team_id, user_id });
        }
        self.checker.invalidate_user(user_id).await;
        Ok(())
    }

    /// The custom roles held in one team, two queries regardless of size.
    pub async fn team_assignments(
        &self,
        team_id: i32,
    ) -> Result<Vec<CustomRoleAssignment>, CustomRoleError> {
        let members = team_members::Entity::find()
            .filter(team_members::Column::TeamId.eq(team_id))
            .all(self.db.as_ref())
            .await?;
        if members.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i32> = members.iter().map(|m| m.id).collect();
        let rows = team_member_custom_roles::Entity::find()
            .filter(team_member_custom_roles::Column::TeamMemberId.is_in(ids))
            .find_also_related(custom_roles::Entity)
            .all(self.db.as_ref())
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(assignment, role)| {
                let role = role?;
                let member = members.iter().find(|m| m.id == assignment.team_member_id)?;
                Some(CustomRoleAssignment {
                    team_member_id: member.id,
                    user_id: member.user_id,
                    custom_role_id: role.id,
                    custom_role_name: role.name,
                })
            })
            .collect())
    }
}

/// A unique-index violation on insert/update is a name taken by a concurrent
/// writer between the check and the write.
fn name_conflict(error: sea_orm::DbErr, name: &str) -> CustomRoleError {
    if crate::service::is_unique_violation(&error) {
        CustomRoleError::NameTaken {
            name: name.to_string(),
        }
    } else {
        CustomRoleError::Database(error)
    }
}

/// Answers [`MembershipPermissionResolver`] from `team_member_custom_roles`.
pub struct CustomRoleResolver {
    db: Arc<DatabaseConnection>,
}

impl CustomRoleResolver {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MembershipPermissionResolver for CustomRoleResolver {
    async fn membership_permissions(
        &self,
        team_member_id: i32,
    ) -> Result<Option<Vec<String>>, Box<dyn std::error::Error + Send + Sync>> {
        let row = team_member_custom_roles::Entity::find_by_id(team_member_id)
            .find_also_related(custom_roles::Entity)
            .one(self.db.as_ref())
            .await?;
        Ok(match row {
            None => None,
            // An assignment whose role is gone (the FK makes this a manual
            // edit) narrows to nothing rather than widening to the fixed role.
            Some((_, None)) => Some(Vec::new()),
            Some((_, Some(role))) => Some(role.permissions.0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase};

    fn request(name: &str, permissions: &[&str]) -> CustomRoleRequest {
        CustomRoleRequest {
            name: name.to_string(),
            description: None,
            permissions: permissions.iter().map(|p| p.to_string()).collect(),
        }
    }

    fn role(id: i32, name: &str, permissions: &[&str]) -> custom_roles::Model {
        custom_roles::Model {
            id,
            name: name.to_string(),
            description: None,
            permissions: RolePermissions(permissions.iter().map(|p| p.to_string()).collect()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn assignment(team_member_id: i32, role_id: i32) -> team_member_custom_roles::Model {
        team_member_custom_roles::Model {
            team_member_id,
            custom_role_id: role_id,
            assigned_by: Some(1),
            created_at: Utc::now(),
        }
    }

    fn service(db: DatabaseConnection) -> CustomRoleService {
        let db = Arc::new(db);
        CustomRoleService::new(db.clone(), Arc::new(TeamProjectAccessChecker::new(db)))
    }

    #[test]
    fn validation_trims_dedupes_and_sorts_project_scoped_permissions() {
        let v = validate(request(
            "  Release manager ",
            &[
                "deployments:create",
                " deployments:read",
                "deployments:create",
            ],
        ))
        .expect("valid");
        assert_eq!(v.name, "Release manager");
        assert_eq!(v.permissions, ["deployments:create", "deployments:read"]);
    }

    #[test]
    fn validation_refuses_empty_unknown_and_instance_admin_permissions() {
        assert!(matches!(
            validate(request("  ", &["projects:read"])),
            Err(CustomRoleError::EmptyName)
        ));
        assert!(matches!(
            validate(request(&"x".repeat(MAX_NAME_LEN + 1), &["projects:read"])),
            Err(CustomRoleError::NameTooLong { .. })
        ));
        assert!(matches!(
            validate(request("r", &["projects:fly"])),
            Err(CustomRoleError::UnknownPermission { ref permission }) if permission == "projects:fly"
        ));
        // Instance-admin permissions are never project-scoped.
        assert!(matches!(
            validate(request("r", &["users:write"])),
            Err(CustomRoleError::NotProjectScoped { .. })
        ));
        assert!(matches!(
            validate(request("r", &[])),
            Err(CustomRoleError::NoPermissions)
        ));
    }

    #[test]
    fn assignable_permissions_are_exactly_the_owner_set() {
        let all = assignable_permissions();
        assert!(all.contains(&"projects:delete".to_string()));
        assert!(!all
            .iter()
            .any(|p| p.starts_with("users:") || p.starts_with("settings:")));
    }

    #[tokio::test]
    async fn create_refuses_a_name_already_taken_case_insensitively() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([vec![role(3, "Release manager", &["projects:read"])]])
            .into_connection();
        let result = service(db)
            .create(request("release MANAGER", &["projects:read"]))
            .await;
        assert!(
            matches!(result, Err(CustomRoleError::NameTaken { ref name }) if name == "release MANAGER"),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn delete_refuses_a_role_still_assigned() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([vec![role(3, "R", &["projects:read"])]])
            .append_query_results([vec![std::collections::BTreeMap::from([(
                "num_items",
                sea_orm::Value::BigInt(Some(2)),
            )])]])
            .into_connection();
        let result = service(db).delete(3).await;
        assert!(
            matches!(
                result,
                Err(CustomRoleError::InUse {
                    role_id: 3,
                    members: 2
                })
            ),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn get_unknown_role_is_not_found_with_its_id() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([Vec::<custom_roles::Model>::new()])
            .into_connection();
        let result = service(db).get(42).await;
        assert!(matches!(
            result,
            Err(CustomRoleError::NotFound { role_id: 42 })
        ));
        let problem = Problem::from(CustomRoleError::NotFound { role_id: 42 });
        assert_eq!(problem.status_code, StatusCode::NOT_FOUND);
        assert!(format!("{:?}", problem.body).contains("42"));
    }

    #[tokio::test]
    async fn resolver_answers_the_assigned_roles_permissions() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([vec![(
                assignment(7, 3),
                Some(role(3, "R", &["deployments:read"])),
            )]])
            .into_connection();
        let perms = CustomRoleResolver::new(Arc::new(db))
            .membership_permissions(7)
            .await
            .expect("resolves");
        assert_eq!(perms, Some(vec!["deployments:read".to_string()]));
    }

    #[tokio::test]
    async fn resolver_falls_through_without_an_assignment() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([Vec::<(
                team_member_custom_roles::Model,
                Option<custom_roles::Model>,
            )>::new()])
            .into_connection();
        let perms = CustomRoleResolver::new(Arc::new(db))
            .membership_permissions(7)
            .await
            .expect("resolves");
        assert_eq!(perms, None);
    }

    #[tokio::test]
    async fn resolver_propagates_a_database_error() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_errors([sea_orm::DbErr::Custom("connection reset".into())])
            .into_connection();
        assert!(CustomRoleResolver::new(Arc::new(db))
            .membership_permissions(7)
            .await
            .is_err());
    }
}
