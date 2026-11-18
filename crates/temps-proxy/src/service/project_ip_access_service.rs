// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Management of per-project / per-environment IP allow/deny lists.
//!
//! This service owns validation and persistence only. Enforcement is done by
//! [`crate::ip_access::ProjectIpAccessGate`], which picks up writes from the
//! database on its own poll, so the service never talks to the proxy.
//!
//! Scope model: `environment_id = None` is the project-wide policy; a row with
//! an environment id overrides it for that environment alone (replace, not
//! merge). Deleting an environment row returns it to inheriting the project.
//! A scope with no row of its own falls back to the instance default
//! (`instance_ip_access_default`), which this service also manages.

use crate::ip_access::{
    parse_cidr, CidrError, CidrSet, IpAccessMode, MAX_CIDRS_PER_POLICY, MAX_DESCRIPTION_LEN,
};
use chrono::{DateTime, Utc};
use ipnetwork::IpNetwork;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect, Set,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::{Arc, OnceLock};
use temps_core::ways_in::{private_allow_list_message, AddressWayIn, EnvironmentWaysIn};
use temps_entities::project_ip_access_rules::{self, IpAccessCidrEntry, IpAccessCidrs};
use temps_entities::{environments, instance_ip_access_default, projects};
use thiserror::Error;
use utoipa::ToSchema;

#[derive(Error, Debug)]
pub enum ProjectIpAccessError {
    #[error("Project {project_id} not found")]
    ProjectNotFound { project_id: i32 },

    #[error("Environment {environment_id} not found in project {project_id}")]
    EnvironmentNotFound {
        project_id: i32,
        environment_id: i32,
    },

    #[error("No IP access policy is configured for {scope}")]
    PolicyNotFound { scope: String },

    #[error("Invalid CIDR in IP access policy for {scope}: {source}")]
    InvalidCidr {
        scope: String,
        #[source]
        source: CidrError,
    },

    #[error("Duplicate CIDR '{cidr}' in IP access policy for {scope}")]
    DuplicateCidr { scope: String, cidr: String },

    #[error("IP access policy for {scope} has {count} entries; the limit is {max}")]
    TooManyCidrs {
        scope: String,
        count: usize,
        max: usize,
    },

    #[error(
        "Description for CIDR '{cidr}' in IP access policy for {scope} is {len} characters; \
         the limit is {max}"
    )]
    DescriptionTooLong {
        scope: String,
        cidr: String,
        len: usize,
        max: usize,
    },

    #[error(
        "An allow list for {scope} with no entries would block every client; add at least one \
         CIDR or set the mode to 'disabled'"
    )]
    EmptyAllowList { scope: String },

    #[error(
        "Saving this policy for {scope} would {effect} your current address {caller_ip}; \
         resubmit with acknowledge_lockout=true to apply it anyway"
    )]
    WouldLockOutCaller {
        scope: String,
        caller_ip: String,
        effect: &'static str,
    },

    #[error("'{input}' is not a valid IPv4 or IPv6 address")]
    InvalidIpAddress { input: String },

    #[error(
        "A draft to test against {scope} needs both 'mode' and 'cidrs'; '{missing}' was not \
         supplied. Send both to preview a draft, or neither to test the saved policy"
    )]
    IncompleteDraft {
        scope: String,
        missing: &'static str,
    },

    #[error(
        "The IP access policy for {scope} was changed by another request at the same time; \
         reload it and try again"
    )]
    ConcurrentUpdate { scope: String },

    /// ADR 064 D9: an allow list of only private ranges on an environment
    /// with an address visitors reach through a tunnel or a `behind_proxy`
    /// ingress. Those hand the proxy the visitor's internet address, so such
    /// a list admits nobody on that address.
    #[error("{}", private_allow_list_message(through, cidr, address))]
    PrivateOnlyAllowListBehindProxy {
        /// `tunnel `homelab``, or the load balancer in front of a worker.
        through: String,
        cidr: String,
        address: String,
    },

    #[error("Database error: {0}")]
    Database(sea_orm::DbErr),
}

/// Whether `error` is a violation of a unique index. Postgres reports it as
/// SQLSTATE 23505, which sea-orm exposes through `sql_err()`; the text match
/// covers drivers and mocks that surface only the message.
fn is_unique_violation(error: &sea_orm::DbErr) -> bool {
    matches!(
        error.sql_err(),
        Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
    ) || {
        let text = error.to_string();
        text.contains("duplicate key value violates unique constraint")
            || text.contains("UNIQUE constraint failed")
    }
}

impl From<sea_orm::DbErr> for ProjectIpAccessError {
    fn from(error: sea_orm::DbErr) -> Self {
        // The only unique index on the table is one policy per scope, so a
        // violation means two writers raced to create the same scope. The
        // scope is not known here; `set` re-labels it before returning.
        if is_unique_violation(&error) {
            return ProjectIpAccessError::ConcurrentUpdate {
                scope: "this scope".to_string(),
            };
        }
        ProjectIpAccessError::Database(error)
    }
}

/// Human-readable scope used in error messages.
fn scope_label(project_id: i32, environment_id: Option<i32>) -> String {
    match environment_id {
        Some(environment_id) => format!("environment {environment_id} of project {project_id}"),
        None => format!("project {project_id}"),
    }
}

/// Request body for `PUT`: the complete policy for one scope.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SetProjectIpAccessRequest {
    /// `disabled` keeps the list but enforces nothing; `allow` admits only
    /// listed clients; `deny` blocks listed clients.
    pub mode: IpAccessMode,
    /// CIDRs (IPv4 or IPv6). A bare address is a single host. At most 256.
    pub cidrs: Vec<IpAccessCidrEntry>,
    /// Apply an allow list that excludes (or a deny list that includes) the
    /// address this request came from. Without it such a save is refused
    /// with 409 so an operator cannot lock themselves out by accident.
    #[serde(default)]
    pub acknowledge_lockout: bool,
}

/// One scope's stored policy.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProjectIpAccessResponse {
    pub project_id: i32,
    /// `null` for the project-wide policy.
    pub environment_id: Option<i32>,
    /// `false` when nothing is stored for this scope. For an environment that
    /// means it inherits the project-wide policy.
    pub configured: bool,
    pub mode: IpAccessMode,
    pub cidrs: Vec<IpAccessCidrEntry>,
    /// Maximum number of CIDRs a policy may hold.
    pub max_cidrs: usize,
    #[schema(value_type = Option<String>, format = "date-time", example = "2026-10-13T09:30:00Z")]
    pub updated_at: Option<DateTime<Utc>>,
    /// Things the operator should know about the saved policy, such as Temps
    /// not being able to tell how visitors arrive (ADR 064 D9).
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Set when neither this scope nor (for an environment) its project has a
    /// policy of its own and the instance default enforces this mode on it.
    /// Saving `disabled` here exempts the scope from the default.
    #[serde(default)]
    pub instance_default_mode: Option<IpAccessMode>,
}

impl ProjectIpAccessResponse {
    fn unconfigured(project_id: i32, environment_id: Option<i32>) -> Self {
        Self {
            project_id,
            environment_id,
            configured: false,
            mode: IpAccessMode::Disabled,
            cidrs: Vec::new(),
            max_cidrs: MAX_CIDRS_PER_POLICY,
            updated_at: None,
            warnings: Vec::new(),
            instance_default_mode: None,
        }
    }
}

impl From<project_ip_access_rules::Model> for ProjectIpAccessResponse {
    fn from(model: project_ip_access_rules::Model) -> Self {
        Self {
            project_id: model.project_id,
            environment_id: model.environment_id,
            configured: true,
            // The CHECK constraint guarantees a known mode; an unknown value
            // is reported as the most restrictive reading, never as open.
            mode: IpAccessMode::parse(&model.mode).unwrap_or(IpAccessMode::Allow),
            cidrs: model.cidrs.0,
            max_cidrs: MAX_CIDRS_PER_POLICY,
            updated_at: Some(model.updated_at),
            warnings: Vec::new(),
            instance_default_mode: None,
        }
    }
}

/// Request body for the "try an address against the rules" endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TestProjectIpAccessRequest {
    /// Address to evaluate.
    #[schema(example = "100.101.102.103")]
    pub ip: String,
    /// Evaluate this unsaved draft instead of the stored policy. Both `mode`
    /// and `cidrs` must be given together.
    pub mode: Option<IpAccessMode>,
    pub cidrs: Option<Vec<IpAccessCidrEntry>>,
}

/// Which policy decided a test.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PolicySource {
    /// The unsaved draft supplied in the request.
    Draft,
    /// The environment's own policy.
    Environment,
    /// The project-wide policy (inherited by the environment).
    Project,
    /// The instance default: the project has no policy of its own.
    Instance,
    /// No policy applies: everything is allowed.
    None,
}

/// Scope label of the instance default in errors and logs.
const INSTANCE_DEFAULT_SCOPE: &str = "the instance default";

/// The instance default policy, applied to every project and environment
/// without a policy of its own.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct InstanceIpAccessDefaultResponse {
    /// `false` when no default is stored: projects without a policy of their
    /// own are reachable from any address.
    pub configured: bool,
    pub mode: IpAccessMode,
    pub cidrs: Vec<IpAccessCidrEntry>,
    /// Maximum number of CIDRs a policy may hold.
    pub max_cidrs: usize,
    #[schema(value_type = Option<String>, format = "date-time", example = "2026-11-17T09:30:00Z")]
    pub updated_at: Option<DateTime<Utc>>,
    /// Things the operator should know about the saved policy.
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl From<Option<instance_ip_access_default::Model>> for InstanceIpAccessDefaultResponse {
    fn from(model: Option<instance_ip_access_default::Model>) -> Self {
        match model {
            Some(model) => Self {
                configured: true,
                // As for project rules: an unknown mode reads as the most
                // restrictive one, never as open.
                mode: IpAccessMode::parse(&model.mode).unwrap_or(IpAccessMode::Allow),
                cidrs: model.cidrs.0,
                max_cidrs: MAX_CIDRS_PER_POLICY,
                updated_at: Some(model.updated_at),
                warnings: Vec::new(),
            },
            None => Self {
                configured: false,
                mode: IpAccessMode::Disabled,
                cidrs: Vec::new(),
                max_cidrs: MAX_CIDRS_PER_POLICY,
                updated_at: None,
                warnings: Vec::new(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TestProjectIpAccessResponse {
    /// The address as evaluated (IPv4-mapped IPv6 is shown as IPv4).
    pub ip: String,
    pub allowed: bool,
    pub source: PolicySource,
    pub mode: IpAccessMode,
    /// The listed CIDR the address fell inside, when one did.
    pub matched_cidr: Option<String>,
    /// One-sentence explanation suitable for display.
    pub reason: String,
}

/// A validated, canonical policy ready to persist.
#[derive(Debug)]
struct ValidatedPolicy {
    mode: IpAccessMode,
    entries: Vec<IpAccessCidrEntry>,
    networks: Vec<IpNetwork>,
}

/// Fold an IPv4-mapped IPv6 address into IPv4, as the gate does when matching.
pub fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        ip => ip,
    }
}

/// Parse and canonicalise a request's entries. Pure: no database access.
fn validate_policy(
    scope: &str,
    mode: IpAccessMode,
    cidrs: &[IpAccessCidrEntry],
) -> Result<ValidatedPolicy, ProjectIpAccessError> {
    if cidrs.len() > MAX_CIDRS_PER_POLICY {
        return Err(ProjectIpAccessError::TooManyCidrs {
            scope: scope.to_string(),
            count: cidrs.len(),
            max: MAX_CIDRS_PER_POLICY,
        });
    }

    let mut seen: HashSet<String> = HashSet::with_capacity(cidrs.len());
    let mut entries = Vec::with_capacity(cidrs.len());
    let mut networks = Vec::with_capacity(cidrs.len());
    for entry in cidrs {
        let network =
            parse_cidr(&entry.cidr).map_err(|source| ProjectIpAccessError::InvalidCidr {
                scope: scope.to_string(),
                source,
            })?;
        let canonical = format!("{}/{}", network.network(), network.prefix());
        if !seen.insert(canonical.clone()) {
            return Err(ProjectIpAccessError::DuplicateCidr {
                scope: scope.to_string(),
                cidr: canonical,
            });
        }

        let description = entry
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string);
        if let Some(description) = &description {
            let len = description.chars().count();
            if len > MAX_DESCRIPTION_LEN {
                return Err(ProjectIpAccessError::DescriptionTooLong {
                    scope: scope.to_string(),
                    cidr: canonical,
                    len,
                    max: MAX_DESCRIPTION_LEN,
                });
            }
        }

        entries.push(IpAccessCidrEntry {
            cidr: canonical,
            description,
        });
        networks.push(network);
    }

    if mode == IpAccessMode::Allow && entries.is_empty() {
        return Err(ProjectIpAccessError::EmptyAllowList {
            scope: scope.to_string(),
        });
    }

    Ok(ValidatedPolicy {
        mode,
        entries,
        networks,
    })
}

/// Evaluate one address against a validated policy: `(allowed, matched_cidr)`.
///
/// `entries[i]` must be the entry `networks[i]` was parsed from (see
/// [`aligned_policy`]); the verdict itself comes from the production matcher,
/// the entry only labels which CIDR matched.
fn evaluate(
    mode: IpAccessMode,
    entries: &[IpAccessCidrEntry],
    networks: &[IpNetwork],
    ip: IpAddr,
) -> (bool, Option<String>) {
    let ip = normalize_ip(ip);
    let matched = networks
        .iter()
        .position(|network| network.contains(ip))
        .and_then(|index| entries.get(index))
        .map(|entry| entry.cidr.clone());
    (evaluate_with_set(mode, networks, ip), matched)
}

/// Pair each stored entry with its parsed network, dropping any entry that no
/// longer parses (the gate skips those too), so indexes stay aligned.
fn aligned_policy(stored: Vec<IpAccessCidrEntry>) -> (Vec<IpAccessCidrEntry>, Vec<IpNetwork>) {
    stored
        .into_iter()
        .filter_map(|entry| parse_cidr(&entry.cidr).ok().map(|network| (entry, network)))
        .unzip()
}

/// Refuse a policy that would lock out the address the save came from,
/// unless the operator acknowledged it.
fn check_lockout(
    scope: &str,
    policy: &ValidatedPolicy,
    caller_ip: Option<IpAddr>,
    acknowledge_lockout: bool,
) -> Result<(), ProjectIpAccessError> {
    let Some(caller_ip) = caller_ip.filter(|_| !acknowledge_lockout) else {
        return Ok(());
    };
    let effect = match policy.mode {
        IpAccessMode::Allow
            if !evaluate_with_set(IpAccessMode::Allow, &policy.networks, caller_ip) =>
        {
            Some("exclude")
        }
        IpAccessMode::Deny
            if !evaluate_with_set(IpAccessMode::Deny, &policy.networks, caller_ip) =>
        {
            Some("block")
        }
        _ => None,
    };
    match effect {
        Some(effect) => Err(ProjectIpAccessError::WouldLockOutCaller {
            scope: scope.to_string(),
            caller_ip: normalize_ip(caller_ip).to_string(),
            effect,
        }),
        None => Ok(()),
    }
}

/// The verdict through the production matcher, so the test endpoint cannot
/// drift from what the proxy does.
fn evaluate_with_set(mode: IpAccessMode, networks: &[IpNetwork], ip: IpAddr) -> bool {
    let set = CidrSet::from_networks(networks);
    match mode {
        IpAccessMode::Disabled => true,
        IpAccessMode::Allow => set.contains(ip),
        IpAccessMode::Deny => !set.contains(ip),
    }
}

/// Whether every address in every network is one nobody on the internet
/// has: RFC 1918, carrier-grade NAT (tailnets), IPv6 unique-local, loopback,
/// link-local.
fn only_private_ranges(networks: &[IpNetwork]) -> bool {
    !networks.is_empty()
        && networks
            .iter()
            .all(|network| temps_core::ways_in::cidr_is_private(&network.to_string()) == Some(true))
}

/// ADR 064 D9 for an allow list of only private ranges: refused when any
/// address is served through a tunnel or a `behind_proxy` ingress. An
/// environment's IP rules apply to all of its addresses, so such a rule is
/// never right for a proxied one; limiting one address to a network is what
/// a private address is for.
fn tunnel_check(addresses: &[AddressWayIn], first_cidr: &str) -> Result<(), ProjectIpAccessError> {
    match addresses
        .iter()
        .find_map(|address| Some((address, address.way_in.proxied_through()?)))
    {
        Some((address, through)) => Err(ProjectIpAccessError::PrivateOnlyAllowListBehindProxy {
            through,
            cidr: first_cidr.to_string(),
            address: address.address.clone(),
        }),
        None => Ok(()),
    }
}

pub struct ProjectIpAccessService {
    db: Arc<DatabaseConnection>,
    /// How visitors reach each address, for the D9 check. Attached once the
    /// ingress plugin has registered it; without it the check is skipped.
    ways_in: OnceLock<Arc<dyn EnvironmentWaysIn>>,
}

impl ProjectIpAccessService {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self {
            db,
            ways_in: OnceLock::new(),
        }
    }

    /// Lets `set` check an allow list against how visitors arrive (ADR 064
    /// D9). Later calls are ignored.
    pub fn attach_ways_in(&self, ways_in: Arc<dyn EnvironmentWaysIn>) {
        let _ = self.ways_in.set(ways_in);
    }

    /// The D9 check for a policy about to be saved: refuses it when any
    /// address's visitors come through a tunnel or a load balancer. Returns
    /// what the operator should know when Temps cannot tell.
    async fn tunnel_warnings(
        &self,
        project_id: i32,
        environment_id: Option<i32>,
        policy: &ValidatedPolicy,
    ) -> Result<Vec<String>, ProjectIpAccessError> {
        if policy.mode != IpAccessMode::Allow || !only_private_ranges(&policy.networks) {
            return Ok(Vec::new());
        }
        let Some(ways_in) = self.ways_in.get() else {
            return Ok(Vec::new());
        };
        let mut addresses = match ways_in.ways_in(project_id, environment_id).await {
            Ok(addresses) => addresses,
            Err(error) => {
                return Ok(vec![format!(
                    "{error}. If visitors arrive through a tunnel, this allow list of private \
                     ranges will block them."
                )])
            }
        };
        // The project-wide policy does not apply to an environment with its
        // own policy, so its addresses do not count.
        if environment_id.is_none() {
            let overridden: HashSet<i32> = project_ip_access_rules::Entity::find()
                .filter(project_ip_access_rules::Column::ProjectId.eq(project_id))
                .filter(project_ip_access_rules::Column::EnvironmentId.is_not_null())
                .all(self.db.as_ref())
                .await?
                .into_iter()
                .filter_map(|rule| rule.environment_id)
                .collect();
            addresses.retain(|address| !overridden.contains(&address.environment_id));
        }
        let first_cidr = policy
            .entries
            .first()
            .map(|entry| entry.cidr.clone())
            .unwrap_or_default();
        tunnel_check(&addresses, &first_cidr)?;
        Ok(Vec::new())
    }

    async fn ensure_scope_exists(
        &self,
        project_id: i32,
        environment_id: Option<i32>,
    ) -> Result<(), ProjectIpAccessError> {
        // Only the id is read: this is an existence check, and decoding a
        // whole project row would pull its large JSON config for nothing.
        let project = projects::Entity::find_by_id(project_id)
            .filter(projects::Column::IsDeleted.eq(false))
            .select_only()
            .column(projects::Column::Id)
            .into_tuple::<i32>()
            .one(self.db.as_ref())
            .await?;
        if project.is_none() {
            return Err(ProjectIpAccessError::ProjectNotFound { project_id });
        }
        if let Some(environment_id) = environment_id {
            let environment = environments::Entity::find_by_id(environment_id)
                .filter(environments::Column::ProjectId.eq(project_id))
                .filter(environments::Column::DeletedAt.is_null())
                .select_only()
                .column(environments::Column::Id)
                .into_tuple::<i32>()
                .one(self.db.as_ref())
                .await?;
            if environment.is_none() {
                return Err(ProjectIpAccessError::EnvironmentNotFound {
                    project_id,
                    environment_id,
                });
            }
        }
        Ok(())
    }

    async fn find_row(
        &self,
        project_id: i32,
        environment_id: Option<i32>,
    ) -> Result<Option<project_ip_access_rules::Model>, sea_orm::DbErr> {
        let mut query = project_ip_access_rules::Entity::find()
            .filter(project_ip_access_rules::Column::ProjectId.eq(project_id));
        query = match environment_id {
            Some(environment_id) => {
                query.filter(project_ip_access_rules::Column::EnvironmentId.eq(environment_id))
            }
            None => query.filter(project_ip_access_rules::Column::EnvironmentId.is_null()),
        };
        query.one(self.db.as_ref()).await
    }

    /// The stored policy for one scope, or an unconfigured placeholder.
    pub async fn get(
        &self,
        project_id: i32,
        environment_id: Option<i32>,
    ) -> Result<ProjectIpAccessResponse, ProjectIpAccessError> {
        self.ensure_scope_exists(project_id, environment_id).await?;
        let Some(row) = self.find_row(project_id, environment_id).await? else {
            let mut response = ProjectIpAccessResponse::unconfigured(project_id, environment_id);
            // An environment without a row follows its project's row first.
            let project_has_row = match environment_id {
                Some(_) => self.find_row(project_id, None).await?.is_some(),
                None => false,
            };
            if !project_has_row {
                response.instance_default_mode = self
                    .find_default()
                    .await?
                    .and_then(|default| IpAccessMode::parse(&default.mode))
                    .filter(|mode| *mode != IpAccessMode::Disabled);
            }
            return Ok(response);
        };
        Ok(ProjectIpAccessResponse::from(row))
    }

    /// Validate and store the complete policy for one scope.
    ///
    /// `caller_ip` is the address the management request came from; when it
    /// would be locked out by the new policy and `acknowledge_lockout` is
    /// false the write is refused.
    pub async fn set(
        &self,
        project_id: i32,
        environment_id: Option<i32>,
        request: SetProjectIpAccessRequest,
        caller_ip: Option<IpAddr>,
        updated_by: Option<i32>,
    ) -> Result<ProjectIpAccessResponse, ProjectIpAccessError> {
        let scope = scope_label(project_id, environment_id);
        let policy = validate_policy(&scope, request.mode, &request.cidrs)?;
        check_lockout(&scope, &policy, caller_ip, request.acknowledge_lockout)?;

        self.ensure_scope_exists(project_id, environment_id).await?;
        let warnings = self
            .tunnel_warnings(project_id, environment_id, &policy)
            .await?;

        let cidrs = IpAccessCidrs(policy.entries);
        // Two writers creating the same scope at once both see no row and both
        // insert; the unique index rejects the second. That writer retries
        // once, finds the row the first one made, and updates it.
        let saved = match self
            .upsert(project_id, environment_id, policy.mode, &cidrs, updated_by)
            .await
        {
            Err(ProjectIpAccessError::ConcurrentUpdate { .. }) => {
                self.upsert(project_id, environment_id, policy.mode, &cidrs, updated_by)
                    .await
            }
            other => other,
        }
        .map_err(|error| match error {
            ProjectIpAccessError::ConcurrentUpdate { .. } => {
                ProjectIpAccessError::ConcurrentUpdate {
                    scope: scope.clone(),
                }
            }
            other => other,
        })?;

        let mut response = ProjectIpAccessResponse::from(saved);
        response.warnings = warnings;
        Ok(response)
    }

    /// Insert or update the single row for a scope inside one transaction.
    async fn upsert(
        &self,
        project_id: i32,
        environment_id: Option<i32>,
        mode: IpAccessMode,
        cidrs: &IpAccessCidrs,
        updated_by: Option<i32>,
    ) -> Result<project_ip_access_rules::Model, ProjectIpAccessError> {
        let txn = self.db.begin().await?;
        let mut query = project_ip_access_rules::Entity::find()
            .filter(project_ip_access_rules::Column::ProjectId.eq(project_id));
        query = match environment_id {
            Some(environment_id) => {
                query.filter(project_ip_access_rules::Column::EnvironmentId.eq(environment_id))
            }
            None => query.filter(project_ip_access_rules::Column::EnvironmentId.is_null()),
        };
        let existing = query.one(&txn).await?;

        let saved = match existing {
            Some(row) => {
                let mut active: project_ip_access_rules::ActiveModel = row.into();
                active.mode = Set(mode.as_str().to_string());
                active.cidrs = Set(cidrs.clone());
                active.updated_by = Set(updated_by);
                active.update(&txn).await?
            }
            None => {
                project_ip_access_rules::ActiveModel {
                    project_id: Set(project_id),
                    environment_id: Set(environment_id),
                    mode: Set(mode.as_str().to_string()),
                    cidrs: Set(cidrs.clone()),
                    updated_by: Set(updated_by),
                    ..Default::default()
                }
                .insert(&txn)
                .await?
            }
        };
        txn.commit().await?;
        Ok(saved)
    }

    async fn find_default(
        &self,
    ) -> Result<Option<instance_ip_access_default::Model>, sea_orm::DbErr> {
        instance_ip_access_default::Entity::find_by_id(instance_ip_access_default::SINGLETON_ID)
            .one(self.db.as_ref())
            .await
    }

    /// The instance default, or an unconfigured placeholder.
    pub async fn get_default(
        &self,
    ) -> Result<InstanceIpAccessDefaultResponse, ProjectIpAccessError> {
        Ok(InstanceIpAccessDefaultResponse::from(
            self.find_default().await?,
        ))
    }

    /// Validate and store the instance default. Same validation and lock-out
    /// check as a project policy.
    pub async fn set_default(
        &self,
        request: SetProjectIpAccessRequest,
        caller_ip: Option<IpAddr>,
        updated_by: Option<i32>,
    ) -> Result<InstanceIpAccessDefaultResponse, ProjectIpAccessError> {
        let policy = validate_policy(INSTANCE_DEFAULT_SCOPE, request.mode, &request.cidrs)?;
        check_lockout(
            INSTANCE_DEFAULT_SCOPE,
            &policy,
            caller_ip,
            request.acknowledge_lockout,
        )?;
        // shortcut: the D9 tunnel check is per environment and is not run
        // across every project here; a private-only allow default only warns.
        // Upgrade to a full check if operators hit it.
        let warnings =
            if policy.mode == IpAccessMode::Allow && only_private_ranges(&policy.networks) {
                vec![
                "This allow list names only private ranges. Visitors of any project that arrive \
                 through a tunnel or a load balancer will be blocked; give such projects a policy \
                 of their own."
                    .to_string(),
            ]
            } else {
                Vec::new()
            };

        let now = Utc::now();
        let row = instance_ip_access_default::ActiveModel {
            id: Set(instance_ip_access_default::SINGLETON_ID),
            mode: Set(policy.mode.as_str().to_string()),
            cidrs: Set(IpAccessCidrs(policy.entries)),
            updated_by: Set(updated_by),
            created_at: Set(now),
            updated_at: Set(now),
        };
        let saved = instance_ip_access_default::Entity::insert(row)
            .on_conflict(
                OnConflict::column(instance_ip_access_default::Column::Id)
                    .update_columns([
                        instance_ip_access_default::Column::Mode,
                        instance_ip_access_default::Column::Cidrs,
                        instance_ip_access_default::Column::UpdatedBy,
                        instance_ip_access_default::Column::UpdatedAt,
                    ])
                    .to_owned(),
            )
            .exec_with_returning(self.db.as_ref())
            .await?;
        let mut response = InstanceIpAccessDefaultResponse::from(Some(saved));
        response.warnings = warnings;
        Ok(response)
    }

    /// Remove the instance default: projects without a policy of their own
    /// become reachable from any address again.
    pub async fn delete_default(&self) -> Result<(), ProjectIpAccessError> {
        let result = instance_ip_access_default::Entity::delete_by_id(
            instance_ip_access_default::SINGLETON_ID,
        )
        .exec(self.db.as_ref())
        .await?;
        if result.rows_affected == 0 {
            return Err(ProjectIpAccessError::PolicyNotFound {
                scope: INSTANCE_DEFAULT_SCOPE.to_string(),
            });
        }
        Ok(())
    }

    /// Remove one scope's policy. For an environment this returns it to
    /// inheriting the project-wide policy.
    pub async fn delete(
        &self,
        project_id: i32,
        environment_id: Option<i32>,
    ) -> Result<(), ProjectIpAccessError> {
        self.ensure_scope_exists(project_id, environment_id).await?;
        let row = self.find_row(project_id, environment_id).await?;
        let Some(row) = row else {
            return Err(ProjectIpAccessError::PolicyNotFound {
                scope: scope_label(project_id, environment_id),
            });
        };
        project_ip_access_rules::Entity::delete_by_id(row.id)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    /// Evaluate `request.ip` against an unsaved draft, or against the policy
    /// that governs the scope (its own, else the project's).
    pub async fn test(
        &self,
        project_id: i32,
        environment_id: Option<i32>,
        request: TestProjectIpAccessRequest,
    ) -> Result<TestProjectIpAccessResponse, ProjectIpAccessError> {
        let scope = scope_label(project_id, environment_id);
        let ip = request.ip.trim().parse::<IpAddr>().map_err(|_| {
            ProjectIpAccessError::InvalidIpAddress {
                input: request.ip.chars().take(64).collect(),
            }
        })?;
        let ip = normalize_ip(ip);

        self.ensure_scope_exists(project_id, environment_id).await?;

        // `fail_closed` explains a saved policy the proxy cannot read: the
        // proxy then refuses every address, so the preview must too.
        let mut fail_closed: Option<String> = None;
        let (source, mode, entries, networks) = match (request.mode, request.cidrs) {
            (Some(mode), Some(cidrs)) => {
                // A draft may be an empty allow list while being edited; the
                // preview then correctly shows everything blocked.
                let policy = if mode == IpAccessMode::Allow && cidrs.is_empty() {
                    ValidatedPolicy {
                        mode,
                        entries: Vec::new(),
                        networks: Vec::new(),
                    }
                } else {
                    validate_policy(&scope, mode, &cidrs)?
                };
                (PolicySource::Draft, mode, policy.entries, policy.networks)
            }
            (Some(_), None) => {
                return Err(ProjectIpAccessError::IncompleteDraft {
                    scope,
                    missing: "cidrs",
                });
            }
            (None, Some(_)) => {
                return Err(ProjectIpAccessError::IncompleteDraft {
                    scope,
                    missing: "mode",
                });
            }
            (None, None) => {
                let own = match environment_id {
                    Some(_) => self.find_row(project_id, environment_id).await?,
                    None => None,
                };
                let (source, row) = match own {
                    Some(row) => (PolicySource::Environment, Some(row)),
                    None => (
                        PolicySource::Project,
                        self.find_row(project_id, None).await?,
                    ),
                };
                // No row of its own (a disabled one still counts): the
                // instance default governs, exactly as in the proxy's gate.
                let stored = match row {
                    Some(row) => Some((source, row.mode, row.cidrs.0)),
                    None => self
                        .find_default()
                        .await?
                        .map(|default| (PolicySource::Instance, default.mode, default.cidrs.0)),
                };
                match stored {
                    Some((source, stored_mode_text, stored_cidrs)) => {
                        let stored_mode = IpAccessMode::parse(&stored_mode_text);
                        let total = stored_cidrs.len();
                        let (entries, networks) = aligned_policy(stored_cidrs);
                        let unreadable = total - entries.len();
                        match stored_mode {
                            // Same rules as the proxy's compile step: an
                            // unknown mode, or a deny list with an entry that
                            // no longer parses, refuses everyone.
                            None => {
                                fail_closed = Some(format!(
                                    "Blocked: the stored mode '{}' is not recognised, so every \
                                     address is refused until the policy is saved again.",
                                    stored_mode_text
                                ));
                                (source, IpAccessMode::Allow, Vec::new(), Vec::new())
                            }
                            Some(IpAccessMode::Deny) if unreadable > 0 => {
                                fail_closed = Some(format!(
                                    "Blocked: the stored deny list has {unreadable} unreadable \
                                     entr{}, so every address is refused until the policy is \
                                     saved again.",
                                    if unreadable == 1 { "y" } else { "ies" }
                                ));
                                (source, IpAccessMode::Allow, Vec::new(), Vec::new())
                            }
                            Some(mode) => (source, mode, entries, networks),
                        }
                    }
                    None => (
                        PolicySource::None,
                        IpAccessMode::Disabled,
                        Vec::new(),
                        Vec::new(),
                    ),
                }
            }
        };

        // A deny list that names nothing, and a disabled policy, enforce nothing.
        let source = if mode == IpAccessMode::Disabled {
            if source == PolicySource::Draft {
                PolicySource::Draft
            } else {
                PolicySource::None
            }
        } else {
            source
        };

        let (allowed, matched_cidr) = evaluate(mode, &entries, &networks, ip);
        let from_instance_default = source == PolicySource::Instance;
        let reason = match (mode, &matched_cidr) {
            (IpAccessMode::Allow, _) if fail_closed.is_some() => fail_closed.unwrap_or_default(),
            (IpAccessMode::Disabled, _) => {
                "No IP restriction applies, so every address is allowed.".to_string()
            }
            (IpAccessMode::Allow, Some(cidr)) => {
                format!("Allowed: {ip} is inside {cidr} on the allow list.")
            }
            (IpAccessMode::Allow, None) => {
                format!("Blocked: {ip} is not inside any CIDR on the allow list.")
            }
            (IpAccessMode::Deny, Some(cidr)) => {
                format!("Blocked: {ip} is inside {cidr} on the deny list.")
            }
            (IpAccessMode::Deny, None) => {
                format!("Allowed: {ip} is not inside any CIDR on the deny list.")
            }
        };
        let reason = if from_instance_default {
            format!("{reason} The list is the instance default: no project or environment policy applies.")
        } else {
            reason
        };

        Ok(TestProjectIpAccessResponse {
            ip: ip.to_string(),
            allowed,
            source,
            mode,
            matched_cidr,
            reason,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult};
    use temps_core::ways_in::WayInKind;

    fn entry(cidr: &str) -> IpAccessCidrEntry {
        IpAccessCidrEntry {
            cidr: cidr.to_string(),
            description: None,
        }
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().expect("test IP is valid")
    }

    /// What the existence check selects: a single `id` column.
    fn id_row(id: i32) -> std::collections::BTreeMap<&'static str, sea_orm::Value> {
        std::collections::BTreeMap::from([("id", sea_orm::Value::Int(Some(id)))])
    }

    fn no_rows() -> Vec<std::collections::BTreeMap<&'static str, sea_orm::Value>> {
        Vec::new()
    }

    // ---- pure validation ----------------------------------------------

    #[test]
    fn validate_canonicalises_and_keeps_descriptions() {
        let policy = validate_policy(
            "project 1",
            IpAccessMode::Allow,
            &[
                IpAccessCidrEntry {
                    cidr: " 203.0.113.7 ".into(),
                    description: Some("  Office  ".into()),
                },
                entry("::ffff:10.0.0.0/104"),
            ],
        )
        .expect("valid policy");
        assert_eq!(policy.entries[0].cidr, "203.0.113.7/32");
        assert_eq!(policy.entries[0].description.as_deref(), Some("Office"));
        assert_eq!(policy.entries[1].cidr, "10.0.0.0/8");
    }

    #[test]
    fn validate_rejects_empty_allow_list_with_scope_in_message() {
        let err = validate_policy("project 42", IpAccessMode::Allow, &[]).unwrap_err();
        assert!(matches!(err, ProjectIpAccessError::EmptyAllowList { .. }));
        assert!(err.to_string().contains("project 42"));
    }

    #[test]
    fn validate_allows_empty_deny_and_disabled_lists() {
        assert!(validate_policy("project 1", IpAccessMode::Deny, &[]).is_ok());
        assert!(validate_policy("project 1", IpAccessMode::Disabled, &[]).is_ok());
    }

    #[test]
    fn validate_rejects_host_bits_and_garbage() {
        let err =
            validate_policy("project 1", IpAccessMode::Deny, &[entry("10.0.0.5/8")]).unwrap_err();
        assert!(matches!(
            err,
            ProjectIpAccessError::InvalidCidr {
                source: CidrError::HostBitsSet { .. },
                ..
            }
        ));
        assert!(err.to_string().contains("10.0.0.0/8"));

        let err = validate_policy("project 1", IpAccessMode::Deny, &[entry("nope")]).unwrap_err();
        assert!(matches!(err, ProjectIpAccessError::InvalidCidr { .. }));
    }

    #[test]
    fn validate_rejects_duplicates_after_canonicalisation() {
        let err = validate_policy(
            "project 1",
            IpAccessMode::Deny,
            &[entry("203.0.113.7"), entry("203.0.113.7/32")],
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ProjectIpAccessError::DuplicateCidr { ref cidr, .. } if cidr == "203.0.113.7/32"
        ));
    }

    #[test]
    fn validate_enforces_the_entry_limit() {
        let at_limit: Vec<_> = (0..MAX_CIDRS_PER_POLICY)
            .map(|i| entry(&format!("10.{}.{}.0/24", i / 256, i % 256)))
            .collect();
        assert!(validate_policy("project 1", IpAccessMode::Deny, &at_limit).is_ok());

        let mut over = at_limit;
        over.push(entry("11.0.0.0/8"));
        let err = validate_policy("project 1", IpAccessMode::Deny, &over).unwrap_err();
        assert!(matches!(
            err,
            ProjectIpAccessError::TooManyCidrs {
                count: 257,
                max: 256,
                ..
            }
        ));
    }

    #[test]
    fn validate_enforces_description_length() {
        let err = validate_policy(
            "project 1",
            IpAccessMode::Deny,
            &[IpAccessCidrEntry {
                cidr: "10.0.0.0/8".into(),
                description: Some("x".repeat(MAX_DESCRIPTION_LEN + 1)),
            }],
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ProjectIpAccessError::DescriptionTooLong { .. }
        ));
    }

    #[test]
    fn evaluate_reports_the_matching_cidr() {
        let policy = validate_policy(
            "project 1",
            IpAccessMode::Allow,
            &[entry("100.64.0.0/10"), entry("fd7a:115c:a1e0::/48")],
        )
        .expect("valid");
        let (allowed, matched) = evaluate(
            policy.mode,
            &policy.entries,
            &policy.networks,
            ip("100.100.1.1"),
        );
        assert!(allowed);
        assert_eq!(matched.as_deref(), Some("100.64.0.0/10"));

        let (allowed, matched) = evaluate(
            policy.mode,
            &policy.entries,
            &policy.networks,
            ip("::ffff:8.8.8.8"),
        );
        assert!(!allowed);
        assert_eq!(matched, None);
    }

    #[test]
    fn service_and_gate_matchers_agree() {
        let policy = validate_policy(
            "project 1",
            IpAccessMode::Allow,
            &[
                entry("10.0.0.0/8"),
                entry("2001:db8::/32"),
                entry("0.0.0.0/0"),
            ],
        )
        .expect("valid");
        for candidate in [
            "10.1.1.1",
            "8.8.8.8",
            "2001:db8::1",
            "2001:db9::1",
            "::ffff:10.1.1.1",
            "::1",
        ] {
            let (service, _) = evaluate(
                policy.mode,
                &policy.entries,
                &policy.networks,
                ip(candidate),
            );
            assert_eq!(
                service,
                evaluate_with_set(policy.mode, &policy.networks, ip(candidate)),
                "matchers disagree for {candidate}"
            );
        }
    }

    // ---- service ------------------------------------------------------

    fn rule_row(
        project_id: i32,
        environment_id: Option<i32>,
        mode: &str,
        cidrs: &[&str],
    ) -> project_ip_access_rules::Model {
        let now = chrono::Utc::now();
        project_ip_access_rules::Model {
            id: 1,
            project_id,
            environment_id,
            mode: mode.into(),
            cidrs: IpAccessCidrs(cidrs.iter().map(|c| entry(c)).collect()),
            updated_by: None,
            revision: 1,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn get_unknown_project_is_not_found_with_the_id() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![no_rows()])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let err = service.get(999, None).await.unwrap_err();
        assert!(matches!(
            err,
            ProjectIpAccessError::ProjectNotFound { project_id: 999 }
        ));
        assert!(err.to_string().contains("999"));
    }

    #[tokio::test]
    async fn get_returns_unconfigured_placeholder_when_no_row_exists() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .append_query_results(vec![Vec::<project_ip_access_rules::Model>::new()])
            .append_query_results(vec![Vec::<instance_ip_access_default::Model>::new()])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let response = service.get(5, None).await.expect("get succeeds");
        assert!(!response.configured);
        assert_eq!(response.mode, IpAccessMode::Disabled);
        assert!(response.cidrs.is_empty());
        assert_eq!(response.max_cidrs, MAX_CIDRS_PER_POLICY);
        assert_eq!(response.instance_default_mode, None);
    }

    #[tokio::test]
    async fn get_reports_the_instance_default_a_scope_without_a_policy_follows() {
        // Environment 9 has no row and neither does its project: the default
        // applies and the response says which mode.
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .append_query_results(vec![vec![id_row(9)]])
            .append_query_results(vec![Vec::<project_ip_access_rules::Model>::new()])
            .append_query_results(vec![Vec::<project_ip_access_rules::Model>::new()])
            .append_query_results(vec![vec![default_model("allow", &["10.0.0.0/8"])]])
            .into_connection();
        let response = ProjectIpAccessService::new(Arc::new(db))
            .get(5, Some(9))
            .await
            .expect("get succeeds");
        assert!(!response.configured);
        assert_eq!(response.instance_default_mode, Some(IpAccessMode::Allow));

        // With a project row (even a disabled one) the default does not
        // apply, and is not read.
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .append_query_results(vec![vec![id_row(9)]])
            .append_query_results(vec![Vec::<project_ip_access_rules::Model>::new()])
            .append_query_results(vec![vec![rule_row(5, None, "disabled", &[])]])
            .into_connection();
        let response = ProjectIpAccessService::new(Arc::new(db))
            .get(5, Some(9))
            .await
            .expect("get succeeds");
        assert_eq!(response.instance_default_mode, None);
    }

    #[tokio::test]
    async fn get_returns_the_stored_policy() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .append_query_results(vec![vec![rule_row(5, None, "allow", &["100.64.0.0/10"])]])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let response = service.get(5, None).await.expect("get succeeds");
        assert!(response.configured);
        assert_eq!(response.mode, IpAccessMode::Allow);
        assert_eq!(response.cidrs[0].cidr, "100.64.0.0/10");
    }

    #[tokio::test]
    async fn set_refuses_a_lockout_before_touching_the_database() {
        // No query results are queued: any DB access would error out.
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let err = service
            .set(
                5,
                None,
                SetProjectIpAccessRequest {
                    mode: IpAccessMode::Allow,
                    cidrs: vec![entry("100.64.0.0/10")],
                    acknowledge_lockout: false,
                },
                Some(ip("203.0.113.9")),
                Some(1),
            )
            .await
            .unwrap_err();
        match err {
            ProjectIpAccessError::WouldLockOutCaller {
                ref caller_ip,
                effect,
                ..
            } => {
                assert_eq!(caller_ip, "203.0.113.9");
                assert_eq!(effect, "exclude");
            }
            other => panic!("expected WouldLockOutCaller, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_refuses_a_deny_list_that_blocks_the_caller() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let err = service
            .set(
                5,
                Some(9),
                SetProjectIpAccessRequest {
                    mode: IpAccessMode::Deny,
                    cidrs: vec![entry("203.0.113.0/24")],
                    acknowledge_lockout: false,
                },
                Some(ip("::ffff:203.0.113.9")),
                Some(1),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ProjectIpAccessError::WouldLockOutCaller {
                effect: "block",
                ..
            }
        ));
        assert!(err.to_string().contains("environment 9 of project 5"));
    }

    #[tokio::test]
    async fn set_reports_validation_errors_before_lockout_or_database() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let err = service
            .set(
                5,
                None,
                SetProjectIpAccessRequest {
                    mode: IpAccessMode::Allow,
                    cidrs: vec![],
                    acknowledge_lockout: false,
                },
                Some(ip("203.0.113.9")),
                Some(1),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ProjectIpAccessError::EmptyAllowList { .. }));
    }

    #[tokio::test]
    async fn set_inserts_a_new_policy_when_the_caller_stays_in() {
        let inserted = rule_row(5, None, "allow", &["100.64.0.0/10"]);
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // ensure_scope_exists: project
            .append_query_results(vec![vec![id_row(5)]])
            // lookup of an existing row inside the transaction
            .append_query_results(vec![Vec::<project_ip_access_rules::Model>::new()])
            // INSERT ... RETURNING
            .append_query_results(vec![vec![inserted]])
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 1,
                rows_affected: 1,
            }])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let response = service
            .set(
                5,
                None,
                SetProjectIpAccessRequest {
                    mode: IpAccessMode::Allow,
                    cidrs: vec![entry("100.64.0.0/10")],
                    acknowledge_lockout: false,
                },
                Some(ip("100.100.1.1")),
                Some(1),
            )
            .await
            .expect("set succeeds");
        assert!(response.configured);
        assert_eq!(response.mode, IpAccessMode::Allow);
    }

    struct FixedWaysIn(Vec<AddressWayIn>);

    #[async_trait::async_trait]
    impl EnvironmentWaysIn for FixedWaysIn {
        async fn ways_in(
            &self,
            _project_id: i32,
            _environment_id: Option<i32>,
        ) -> Result<Vec<AddressWayIn>, temps_core::ways_in::WaysInError> {
            Ok(self.0.clone())
        }
    }

    fn tunneled(address: &str) -> AddressWayIn {
        AddressWayIn {
            environment_id: 7,
            address: address.to_string(),
            way_in: WayInKind::Tunnel {
                name: "homelab".to_string(),
            },
        }
    }

    fn direct(address: &str) -> AddressWayIn {
        AddressWayIn {
            environment_id: 7,
            address: address.to_string(),
            way_in: WayInKind::Direct,
        }
    }

    fn networks(cidrs: &[&str]) -> Vec<IpNetwork> {
        cidrs
            .iter()
            .map(|cidr| cidr.parse().expect("test CIDR is valid"))
            .collect()
    }

    #[test]
    fn private_ranges_are_told_apart_from_internet_ones() {
        assert!(only_private_ranges(&networks(&["100.64.0.0/10"])));
        assert!(only_private_ranges(&networks(&[
            "10.0.0.0/8",
            "192.168.1.0/24",
            "fd7a:115c:a1e0::/48",
            "127.0.0.1/32",
        ])));
        // One internet range is enough for a tunnel visitor to match.
        assert!(!only_private_ranges(&networks(&[
            "10.0.0.0/8",
            "203.0.113.0/24"
        ])));
        // A range that only starts private is not private.
        assert!(!only_private_ranges(&networks(&["0.0.0.0/0"])));
        assert!(!only_private_ranges(&networks(&["::/0"])));
        assert!(!only_private_ranges(&[]));
    }

    #[test]
    fn a_private_allow_list_is_refused_when_any_address_is_proxied() {
        let error = tunnel_check(
            &[direct("app.internal.example"), tunneled("app.example.com")],
            "100.64.0.0/10",
        )
        .expect_err("visitors on the tunneled address would be blocked");
        assert_eq!(
            error.to_string(),
            "Visitors through tunnel `homelab` arrive with their internet address, so \
             `100.64.0.0/10` would block everyone on `app.example.com`. To limit this app to \
             your tailnet, make that address private instead."
        );

        let behind = AddressWayIn {
            environment_id: 7,
            address: "shop.example.com".into(),
            way_in: WayInKind::BehindProxy {
                node: "edge-1".into(),
            },
        };
        let error = tunnel_check(&[behind], "10.0.0.0/8").expect_err("load balancer");
        assert!(error
            .to_string()
            .starts_with("Visitors through the load balancer in front of `edge-1`"));

        tunnel_check(&[direct("app.example.com")], "10.0.0.0/8").expect("nothing proxied");
        tunnel_check(&[], "10.0.0.0/8").expect("no addresses");
    }

    #[tokio::test]
    async fn set_refuses_a_tailnet_allow_list_on_a_tunneled_environment() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // ensure_scope_exists: project, then environment
            .append_query_results(vec![vec![id_row(5)]])
            .append_query_results(vec![vec![id_row(7)]])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));
        service.attach_ways_in(Arc::new(FixedWaysIn(vec![tunneled("app.example.com")])));

        let error = service
            .set(
                5,
                Some(7),
                SetProjectIpAccessRequest {
                    mode: IpAccessMode::Allow,
                    cidrs: vec![entry("100.64.0.0/10")],
                    acknowledge_lockout: true,
                },
                None,
                Some(1),
            )
            .await
            .expect_err("the policy is refused before it is saved");
        assert!(matches!(
            error,
            ProjectIpAccessError::PrivateOnlyAllowListBehindProxy { ref through, ref cidr, ref address }
                if through == "tunnel `homelab`" && cidr == "100.64.0.0/10" && address == "app.example.com"
        ));
    }

    #[tokio::test]
    async fn set_ignores_tunnels_for_a_list_with_an_internet_range() {
        let inserted = rule_row(5, None, "allow", &["203.0.113.0/24"]);
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .append_query_results(vec![Vec::<project_ip_access_rules::Model>::new()])
            .append_query_results(vec![vec![inserted]])
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 1,
                rows_affected: 1,
            }])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));
        service.attach_ways_in(Arc::new(FixedWaysIn(vec![tunneled("app.example.com")])));

        let response = service
            .set(
                5,
                None,
                SetProjectIpAccessRequest {
                    mode: IpAccessMode::Allow,
                    cidrs: vec![entry("203.0.113.0/24")],
                    acknowledge_lockout: true,
                },
                None,
                Some(1),
            )
            .await
            .expect("an internet range can match tunnel visitors");
        assert!(response.warnings.is_empty());
    }

    #[test]
    fn aligned_policy_drops_unparsable_entries_without_shifting_labels() {
        let (entries, networks) = aligned_policy(vec![
            entry("bogus"),
            entry("10.0.0.0/8"),
            entry("2001:db8::/32"),
        ]);
        assert_eq!(entries.len(), 2);
        assert_eq!(networks.len(), 2);

        let (allowed, matched) = evaluate(IpAccessMode::Allow, &entries, &networks, ip("10.1.2.3"));
        assert!(allowed);
        assert_eq!(matched.as_deref(), Some("10.0.0.0/8"));

        let (allowed, matched) =
            evaluate(IpAccessMode::Deny, &entries, &networks, ip("2001:db8::7"));
        assert!(!allowed);
        assert_eq!(matched.as_deref(), Some("2001:db8::/32"));
    }

    #[test]
    fn unique_violations_are_recognised_from_the_driver_message() {
        let duplicate = sea_orm::DbErr::Exec(sea_orm::RuntimeErr::Internal(
            "duplicate key value violates unique constraint \"project_ip_access_rules_scope_unique\""
                .to_string(),
        ));
        assert!(is_unique_violation(&duplicate));
        assert!(!is_unique_violation(&sea_orm::DbErr::Custom(
            "connection reset".to_string()
        )));
        assert!(matches!(
            ProjectIpAccessError::from(duplicate),
            ProjectIpAccessError::ConcurrentUpdate { .. }
        ));
    }

    #[tokio::test]
    async fn set_retries_once_when_another_writer_created_the_scope_first() {
        let duplicate = sea_orm::DbErr::Exec(sea_orm::RuntimeErr::Internal(
            "duplicate key value violates unique constraint \"project_ip_access_rules_scope_unique\""
                .to_string(),
        ));
        let stored = rule_row(5, None, "deny", &["198.51.100.0/24"]);
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // ensure_scope_exists: project
            .append_query_results(vec![vec![id_row(5)]])
            // first attempt: no row yet ...
            .append_query_results(vec![Vec::<project_ip_access_rules::Model>::new()])
            // ... and the INSERT loses the race
            .append_query_errors(vec![duplicate])
            // retry: the winner's row is now visible
            .append_query_results(vec![vec![stored.clone()]])
            // UPDATE ... RETURNING
            .append_query_results(vec![vec![stored]])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let response = service
            .set(
                5,
                None,
                SetProjectIpAccessRequest {
                    mode: IpAccessMode::Deny,
                    cidrs: vec![entry("198.51.100.0/24")],
                    acknowledge_lockout: true,
                },
                None,
                Some(1),
            )
            .await
            .expect("the retry updates the row the other writer created");
        assert!(response.configured);
        assert_eq!(response.mode, IpAccessMode::Deny);
    }

    #[tokio::test]
    async fn set_reports_a_repeated_unique_violation_as_a_conflict_with_the_scope() {
        let duplicate = || {
            sea_orm::DbErr::Exec(sea_orm::RuntimeErr::Internal(
                "duplicate key value violates unique constraint".to_string(),
            ))
        };
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            // ensure_scope_exists: environment
            .append_query_results(vec![vec![id_row(9)]])
            .append_query_results(vec![Vec::<project_ip_access_rules::Model>::new()])
            .append_query_errors(vec![duplicate()])
            .append_query_results(vec![Vec::<project_ip_access_rules::Model>::new()])
            .append_query_errors(vec![duplicate()])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let err = service
            .set(
                5,
                Some(9),
                SetProjectIpAccessRequest {
                    mode: IpAccessMode::Deny,
                    cidrs: vec![entry("198.51.100.0/24")],
                    acknowledge_lockout: true,
                },
                None,
                None,
            )
            .await
            .unwrap_err();
        match err {
            ProjectIpAccessError::ConcurrentUpdate { scope } => {
                assert!(scope.contains("environment 9"), "{scope}");
            }
            other => panic!("expected ConcurrentUpdate, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_surfaces_database_errors() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_errors(vec![sea_orm::DbErr::Custom("connection reset".into())])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let err = service
            .set(
                5,
                None,
                SetProjectIpAccessRequest {
                    mode: IpAccessMode::Deny,
                    cidrs: vec![entry("198.51.100.0/24")],
                    acknowledge_lockout: true,
                },
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ProjectIpAccessError::Database(_)));
        assert!(err.to_string().contains("connection reset"));
    }

    #[tokio::test]
    async fn delete_without_a_stored_policy_is_not_found_with_the_scope() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .append_query_results(vec![Vec::<project_ip_access_rules::Model>::new()])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let err = service.delete(5, None).await.unwrap_err();
        assert!(matches!(err, ProjectIpAccessError::PolicyNotFound { .. }));
        assert!(err.to_string().contains("project 5"));
    }

    #[tokio::test]
    async fn test_rejects_an_unparsable_address() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let err = service
            .test(
                5,
                None,
                TestProjectIpAccessRequest {
                    ip: "999.1.1.1".into(),
                    mode: None,
                    cidrs: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ProjectIpAccessError::InvalidIpAddress { .. }));
    }

    #[tokio::test]
    async fn test_evaluates_an_unsaved_draft() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let response = service
            .test(
                5,
                None,
                TestProjectIpAccessRequest {
                    ip: "100.100.1.1".into(),
                    mode: Some(IpAccessMode::Allow),
                    cidrs: Some(vec![entry("100.64.0.0/10")]),
                },
            )
            .await
            .expect("test succeeds");
        assert!(response.allowed);
        assert_eq!(response.source, PolicySource::Draft);
        assert_eq!(response.matched_cidr.as_deref(), Some("100.64.0.0/10"));
    }

    #[tokio::test]
    async fn test_with_an_empty_allow_draft_blocks_everything_instead_of_erroring() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let response = service
            .test(
                5,
                None,
                TestProjectIpAccessRequest {
                    ip: "100.100.1.1".into(),
                    mode: Some(IpAccessMode::Allow),
                    cidrs: Some(vec![]),
                },
            )
            .await
            .expect("test succeeds");
        assert!(!response.allowed);
    }

    #[tokio::test]
    async fn test_without_any_policy_allows_and_says_so() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .append_query_results(vec![Vec::<project_ip_access_rules::Model>::new()])
            .append_query_results(vec![Vec::<instance_ip_access_default::Model>::new()])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let response = service
            .test(
                5,
                None,
                TestProjectIpAccessRequest {
                    ip: "8.8.8.8".into(),
                    mode: None,
                    cidrs: None,
                },
            )
            .await
            .expect("test succeeds");
        assert!(response.allowed);
        assert_eq!(response.source, PolicySource::None);
    }

    fn probe(ip: &str) -> TestProjectIpAccessRequest {
        TestProjectIpAccessRequest {
            ip: ip.into(),
            mode: None,
            cidrs: None,
        }
    }

    #[tokio::test]
    async fn test_with_only_a_mode_is_rejected_not_answered_from_the_saved_list() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let err = service
            .test(
                5,
                None,
                TestProjectIpAccessRequest {
                    mode: Some(IpAccessMode::Deny),
                    ..probe("8.8.8.8")
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ProjectIpAccessError::IncompleteDraft {
                missing: "cidrs",
                ..
            }
        ));
        assert!(err.to_string().contains("project 5"));
    }

    #[tokio::test]
    async fn test_with_only_cidrs_is_rejected_not_answered_from_the_saved_list() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let err = service
            .test(
                5,
                None,
                TestProjectIpAccessRequest {
                    cidrs: Some(vec![entry("10.0.0.0/8")]),
                    ..probe("8.8.8.8")
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ProjectIpAccessError::IncompleteDraft {
                missing: "mode",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn test_a_saved_deny_list_with_an_unreadable_entry_refuses_everyone_like_the_proxy() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .append_query_results(vec![vec![rule_row(
                5,
                None,
                "deny",
                &["10.0.0.0/8", "not-a-cidr"],
            )]])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let response = service
            .test(5, None, probe("8.8.8.8"))
            .await
            .expect("test succeeds");
        assert!(!response.allowed);
        assert_eq!(response.source, PolicySource::Project);
        assert!(
            response.reason.contains("unreadable"),
            "{}",
            response.reason
        );
    }

    #[tokio::test]
    async fn test_a_saved_deny_list_of_only_unreadable_entries_refuses_everyone() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .append_query_results(vec![vec![rule_row(5, None, "deny", &["not-a-cidr"])]])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let response = service
            .test(5, None, probe("8.8.8.8"))
            .await
            .expect("test succeeds");
        assert!(!response.allowed);
    }

    #[tokio::test]
    async fn test_a_saved_allow_list_still_skips_an_unreadable_entry() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .append_query_results(vec![vec![rule_row(
                5,
                None,
                "allow",
                &["not-a-cidr", "10.0.0.0/8"],
            )]])
            .into_connection();
        let service = ProjectIpAccessService::new(Arc::new(db));

        let response = service
            .test(5, None, probe("10.1.2.3"))
            .await
            .expect("test succeeds");
        assert!(response.allowed);
        assert_eq!(response.matched_cidr.as_deref(), Some("10.0.0.0/8"));
    }

    // ---- instance default ----------------------------------------------

    fn default_model(mode: &str, cidrs: &[&str]) -> instance_ip_access_default::Model {
        let now = chrono::Utc::now();
        instance_ip_access_default::Model {
            id: instance_ip_access_default::SINGLETON_ID,
            mode: mode.into(),
            cidrs: IpAccessCidrs(cidrs.iter().map(|c| entry(c)).collect()),
            updated_by: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn set_request(mode: IpAccessMode, cidrs: &[&str]) -> SetProjectIpAccessRequest {
        SetProjectIpAccessRequest {
            mode,
            cidrs: cidrs.iter().map(|c| entry(c)).collect(),
            acknowledge_lockout: false,
        }
    }

    #[tokio::test]
    async fn test_falls_back_to_the_instance_default_like_the_gate() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .append_query_results(vec![Vec::<project_ip_access_rules::Model>::new()])
            .append_query_results(vec![vec![default_model("allow", &["10.0.0.0/8"])]])
            .into_connection();
        let response = ProjectIpAccessService::new(Arc::new(db))
            .test(5, None, probe("8.8.8.8"))
            .await
            .expect("test succeeds");
        assert_eq!(response.source, PolicySource::Instance);
        assert!(!response.allowed);
    }

    #[tokio::test]
    async fn a_disabled_project_policy_opts_out_of_the_instance_default() {
        // No default query is queued: reading it would fail the test.
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![id_row(5)]])
            .append_query_results(vec![vec![rule_row(5, None, "disabled", &[])]])
            .into_connection();
        let response = ProjectIpAccessService::new(Arc::new(db))
            .test(5, None, probe("8.8.8.8"))
            .await
            .expect("test succeeds");
        assert_eq!(response.source, PolicySource::None);
        assert!(response.allowed);
    }

    #[tokio::test]
    async fn get_default_is_an_unconfigured_placeholder_without_a_row() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<instance_ip_access_default::Model>::new()])
            .into_connection();
        let response = ProjectIpAccessService::new(Arc::new(db))
            .get_default()
            .await
            .expect("get succeeds");
        assert!(!response.configured);
        assert_eq!(response.mode, IpAccessMode::Disabled);
        assert_eq!(response.max_cidrs, MAX_CIDRS_PER_POLICY);
    }

    #[tokio::test]
    async fn set_default_validates_and_checks_lockout_before_the_database() {
        // An empty mock: any query would fail the call with a database error.
        let service = ProjectIpAccessService::new(Arc::new(
            MockDatabase::new(DatabaseBackend::Postgres).into_connection(),
        ));
        let invalid = service
            .set_default(
                set_request(IpAccessMode::Allow, &["not-a-cidr"]),
                None,
                Some(1),
            )
            .await;
        assert!(
            matches!(invalid, Err(ProjectIpAccessError::InvalidCidr { ref scope, .. }) if scope == INSTANCE_DEFAULT_SCOPE),
            "{invalid:?}"
        );
        let empty = service
            .set_default(set_request(IpAccessMode::Allow, &[]), None, Some(1))
            .await;
        assert!(matches!(
            empty,
            Err(ProjectIpAccessError::EmptyAllowList { .. })
        ));
        let locked_out = service
            .set_default(
                set_request(IpAccessMode::Allow, &["10.0.0.0/8"]),
                Some(ip("203.0.113.9")),
                Some(1),
            )
            .await;
        assert!(
            matches!(
                locked_out,
                Err(ProjectIpAccessError::WouldLockOutCaller {
                    effect: "exclude",
                    ..
                })
            ),
            "{locked_out:?}"
        );
    }

    #[tokio::test]
    async fn set_default_upserts_the_single_row_and_warns_on_private_only_lists() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![default_model("allow", &["100.64.0.0/10"])]])
            .into_connection();
        let db = Arc::new(db);
        let response = ProjectIpAccessService::new(Arc::clone(&db))
            .set_default(
                set_request(IpAccessMode::Allow, &["100.64.0.0/10"]),
                Some(ip("100.64.1.2")),
                Some(1),
            )
            .await
            .expect("saved");
        assert!(response.configured);
        assert_eq!(response.mode, IpAccessMode::Allow);
        assert_eq!(response.warnings.len(), 1, "{:?}", response.warnings);
        let log = Arc::try_unwrap(db)
            .map_err(|_| "service still holds the database")
            .expect("released")
            .into_transaction_log();
        let sql = format!("{log:?}");
        assert!(sql.contains("ON CONFLICT"), "single-row upsert: {sql}");
    }

    #[tokio::test]
    async fn delete_default_without_a_row_is_not_found() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_exec_results(vec![sea_orm::MockExecResult {
                last_insert_id: 0,
                rows_affected: 0,
            }])
            .into_connection();
        let result = ProjectIpAccessService::new(Arc::new(db))
            .delete_default()
            .await;
        assert!(
            matches!(result, Err(ProjectIpAccessError::PolicyNotFound { ref scope }) if scope == INSTANCE_DEFAULT_SCOPE),
            "{result:?}"
        );
    }
}
