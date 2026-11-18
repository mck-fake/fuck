// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Management of project edge rules (ADR-089).
//!
//! The service owns validation, ordering and persistence. Enforcement is
//! [`crate::edge_rules::EdgeRuleGate`], which picks writes up from the change
//! feed on its own poll, so a save is enforced by every proxy within seconds
//! and the service never talks to the proxy.
//!
//! Writes to one project's rules are serialised by a transaction-scoped
//! advisory lock keyed on the project, so the 100-rule limit and the
//! positions cannot be raced. Edits carry the rule's `revision`; a stale one is
//! refused with [`EdgeRuleServiceError::RevisionConflict`] instead of
//! overwriting someone else's change.
//!
//! Every write also stores, in the same transaction, a numbered snapshot of
//! the whole rule set in `project_edge_rule_revisions` (ADR-089 A1); the last
//! [`MAX_REVISIONS_PER_PROJECT`] are kept and any of them can be restored.

use crate::edge_rules::definition_from_model;
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    DatabaseTransaction, EntityTrait, FromQueryResult, QueryFilter, QueryOrder, Set, Statement,
    TransactionTrait,
};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use temps_edge_rules::describe::describe_rule;
use temps_edge_rules::{
    auto_name, limits, prepare_expression, validate_rule, Action, Decision, Group, Mode,
    ParsedSample, PatternUsage, Program, RuleDefinition, RuleIssue, RuleOutcome, SampleRequest,
};
use temps_entities::{environments, project_edge_rule_revisions, project_edge_rules, projects};
use thiserror::Error;
use tracing::warn;
use utoipa::ToSchema;

#[derive(Error, Debug)]
pub enum EdgeRuleServiceError {
    #[error("Project {project_id} not found")]
    ProjectNotFound { project_id: i32 },

    #[error("Environment {environment_id} not found in project {project_id}")]
    EnvironmentNotFound {
        project_id: i32,
        environment_id: i32,
    },

    #[error("Edge rule {rule_id} not found in project {project_id}")]
    RuleNotFound { project_id: i32, rule_id: i32 },

    #[error(
        "Edge rule '{rule}' in project {project_id} is invalid: {}",
        summarize(issues)
    )]
    Invalid {
        project_id: i32,
        rule: String,
        issues: Vec<RuleIssue>,
    },

    #[error("Project {project_id} already has {max} edge rules, the maximum")]
    TooManyRules { project_id: i32, max: usize },

    #[error(
        "Edge rule {rule_id} in project {project_id} was changed by someone else (you edited \
         revision {expected}, it is now at {current}); reload it and apply your change again"
    )]
    RevisionConflict {
        project_id: i32,
        rule_id: i32,
        expected: i64,
        current: i64,
    },

    #[error("Cannot reorder the edge rules of project {project_id}: {reason}")]
    InvalidOrder { project_id: i32, reason: String },

    #[error(
        "Edge rule {rule_id} in project {project_id} is stored in a form this version cannot \
         read ({reason}); delete it or save it again"
    )]
    Unreadable {
        project_id: i32,
        rule_id: i32,
        reason: String,
    },

    #[error("Edge rule revision {number} not found in project {project_id}")]
    RevisionNotFound { project_id: i32, number: i32 },

    #[error("Edge rule revision {number} of project {project_id} cannot be restored ({reason})")]
    UnreadableRevision {
        project_id: i32,
        number: i32,
        reason: String,
    },

    #[error("Database error while working on the edge rules of project {project_id}: {source}")]
    Database {
        project_id: i32,
        #[source]
        source: sea_orm::DbErr,
    },
}

fn summarize(issues: &[RuleIssue]) -> String {
    issues
        .iter()
        .map(|i| format!("{}: {}", i.target, i.message))
        .collect::<Vec<_>>()
        .join("; ")
}

fn db_error(project_id: i32) -> impl FnOnce(sea_orm::DbErr) -> EdgeRuleServiceError {
    move |source| EdgeRuleServiceError::Database { project_id, source }
}

/// `Some(None)` for an explicit `null`, `None` when the field is absent.
fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

// ---- API types ---------------------------------------------------------

/// One stored rule.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EdgeRuleResponse {
    pub id: i32,
    pub project_id: i32,
    /// `null` applies the rule to every environment of the project.
    pub environment_id: Option<i32>,
    /// 0-based evaluation order. The first rule that decides wins.
    pub position: i32,
    pub name: String,
    pub enabled: bool,
    pub mode: Mode,
    pub expression: Group,
    pub action: Action,
    /// Edit counter. Send it back on update so a concurrent change is
    /// refused instead of overwritten.
    pub revision: i64,
    /// The rule in one sentence, e.g. "Block with 403 when path starts with
    /// /admin".
    pub summary: String,
    /// Requests this rule has matched, across every proxy. Counters are
    /// flushed every few seconds.
    pub hits: i64,
    #[schema(value_type = Option<String>, format = "date-time", example = "2026-10-31T09:30:00Z")]
    pub last_hit_at: Option<DateTime<Utc>>,
    #[schema(value_type = String, format = "date-time", example = "2026-10-31T09:30:00Z")]
    pub created_at: DateTime<Utc>,
    #[schema(value_type = String, format = "date-time", example = "2026-10-31T09:30:00Z")]
    pub updated_at: DateTime<Utc>,
}

/// A project's rules in evaluation order. The list is bounded by
/// `max_rules`, so it is returned whole rather than paginated: order is the
/// point, and a page of an ordered rule set cannot be reasoned about.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EdgeRuleListResponse {
    pub project_id: i32,
    pub rules: Vec<EdgeRuleResponse>,
    /// Maximum number of rules a project may hold.
    pub max_rules: usize,
}

/// Body for creating a rule.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateEdgeRuleRequest {
    /// Up to 80 characters. Left empty, a name is generated from the rule,
    /// e.g. "Block when path starts with /admin".
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// `enforce` acts on matches; `log_only` (the default) records them
    /// without acting, so a new rule can be watched before it enforces.
    #[serde(default = "default_create_mode")]
    pub mode: Mode,
    #[serde(default)]
    pub environment_id: Option<i32>,
    pub expression: Group,
    pub action: Action,
    /// Where to insert the rule (0 = first). Defaults to the end.
    #[serde(default)]
    pub position: Option<i32>,
}

fn default_true() -> bool {
    true
}

/// Dry-run drafts act as they would once enforced.
fn default_mode() -> Mode {
    Mode::Enforce
}

/// New rules start by only recording what they match.
fn default_create_mode() -> Mode {
    Mode::LogOnly
}

/// Body for changing a rule. Absent fields keep their value.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct UpdateEdgeRuleRequest {
    /// The revision the change was made against. When given and the rule has
    /// moved on, the update is refused with 409.
    #[serde(default)]
    pub revision: Option<i64>,
    /// An empty name regenerates it from the rule.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub mode: Option<Mode>,
    /// `null` applies the rule to every environment.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<i32>)]
    pub environment_id: Option<Option<i32>>,
    #[serde(default)]
    pub expression: Option<Group>,
    #[serde(default)]
    pub action: Option<Action>,
}

/// Body for reordering: every rule id of the project, in the new order.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReorderEdgeRulesRequest {
    pub rule_ids: Vec<i32>,
}

/// A rule supplied to the dry run instead of the saved ones.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[schema(as = EvaluateEdgeRuleInput)]
pub struct EvaluateRuleInput {
    /// Saved rule id, or omit for an unsaved draft.
    #[serde(default)]
    pub id: Option<i32>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_mode")]
    pub mode: Mode,
    #[serde(default)]
    pub environment_id: Option<i32>,
    pub expression: Group,
    pub action: Action,
}

/// Body for the dry run.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EvaluateEdgeRulesRequest {
    pub request: SampleRequest,
    /// Rules to evaluate in order instead of the saved ones, e.g. an edited
    /// rule set before saving it.
    #[serde(default)]
    pub rules: Option<Vec<EvaluateRuleInput>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[schema(as = EvaluateEdgeRuleTraceEntry)]
pub struct EvaluateTraceEntry {
    /// Saved rule id, or a negative placeholder for an unsaved draft
    /// (`-1` for the first draft, `-2` for the second, ...).
    pub rule_id: i32,
    pub name: String,
    pub outcome: RuleOutcome,
}

/// What the proxy would do with the request.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EvaluateEdgeRulesResponse {
    pub decision: Decision,
    /// The rule the proxy log would record: the deciding rule, or the first
    /// rule that matched in log-only mode.
    pub matched_rule_id: Option<i32>,
    pub matched_mode: Option<Mode>,
    /// The decision in one sentence.
    pub summary: String,
    pub trace: Vec<EvaluateTraceEntry>,
}

/// Revisions kept per project; older ones are pruned on every write.
pub const MAX_REVISIONS_PER_PROJECT: i32 = 100;

/// One revision in the history list.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, FromQueryResult)]
pub struct EdgeRuleRevisionSummary {
    /// 1, 2, 3... per project; the highest is the rule set in force.
    pub number: i32,
    /// What the write did, e.g. "Created rule 'Block /admin'".
    pub change: String,
    pub rule_count: i32,
    pub created_by: Option<i32>,
    pub created_by_name: Option<String>,
    #[schema(value_type = String, format = "date-time", example = "2026-11-17T09:30:00Z")]
    pub created_at: DateTime<Utc>,
}

/// A page of a project's revisions, newest first.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EdgeRuleRevisionListResponse {
    pub project_id: i32,
    pub revisions: Vec<EdgeRuleRevisionSummary>,
    pub total: i64,
    pub page: u64,
    pub per_page: u64,
    /// How many revisions are kept; older ones are pruned.
    pub max_revisions: i32,
}

/// One rule as a revision stored it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EdgeRuleRevisionRule {
    pub id: i32,
    pub environment_id: Option<i32>,
    pub name: String,
    pub enabled: bool,
    pub mode: Mode,
    pub expression: Group,
    pub action: Action,
}

/// A revision with its whole rule set, in evaluation order.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EdgeRuleRevisionResponse {
    pub project_id: i32,
    #[serde(flatten)]
    pub revision: EdgeRuleRevisionSummary,
    pub rules: Vec<EdgeRuleRevisionRule>,
}

/// The snapshot of a project's rules a write leaves behind, built in SQL
/// from the raw columns so a rule this version cannot decode never blocks
/// an unrelated write. `$1` project, `$2` change, `$3` author.
const RECORD_REVISION_SQL: &str = "INSERT INTO project_edge_rule_revisions \
     (project_id, number, rules, change, created_by) \
     SELECT $1, COALESCE((SELECT MAX(number) FROM project_edge_rule_revisions \
     WHERE project_id = $1), 0) + 1, \
     COALESCE((SELECT jsonb_agg(jsonb_build_object('id', id, 'environment_id', \
     environment_id, 'name', name, 'enabled', enabled, 'mode', mode, 'expression', \
     expression, 'action', action) ORDER BY position, id) FROM project_edge_rules \
     WHERE project_id = $1), '[]'::jsonb), $2, $3";

const PRUNE_REVISIONS_SQL: &str = "DELETE FROM project_edge_rule_revisions \
     WHERE project_id = $1 AND number <= (SELECT MAX(number) FROM \
     project_edge_rule_revisions WHERE project_id = $1) - $2";

const REVISION_SUMMARY_SQL: &str = "SELECT r.number, r.change, \
     jsonb_array_length(r.rules)::int AS rule_count, r.created_by, \
     u.name AS created_by_name, r.created_at \
     FROM project_edge_rule_revisions r LEFT JOIN users u ON u.id = r.created_by \
     WHERE r.project_id = $1";

// ---- service -----------------------------------------------------------

#[derive(Debug, FromQueryResult)]
struct StatsRow {
    rule_id: i32,
    hits: i64,
    last_hit_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    count: i64,
}

pub struct EdgeRuleService {
    db: Arc<DatabaseConnection>,
}

/// A validated rule, ready to store.
struct Prepared {
    name: String,
    expression: Group,
    action: Action,
}

impl EdgeRuleService {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    async fn ensure_project<C: ConnectionTrait>(
        &self,
        conn: &C,
        project_id: i32,
    ) -> Result<(), EdgeRuleServiceError> {
        let project = projects::Entity::find_by_id(project_id)
            .one(conn)
            .await
            .map_err(db_error(project_id))?;
        match project {
            Some(p) if !p.is_deleted => Ok(()),
            _ => Err(EdgeRuleServiceError::ProjectNotFound { project_id }),
        }
    }

    async fn ensure_environment<C: ConnectionTrait>(
        &self,
        conn: &C,
        project_id: i32,
        environment_id: Option<i32>,
    ) -> Result<(), EdgeRuleServiceError> {
        let Some(environment_id) = environment_id else {
            return Ok(());
        };
        let env = environments::Entity::find_by_id(environment_id)
            .filter(environments::Column::ProjectId.eq(project_id))
            .filter(environments::Column::DeletedAt.is_null())
            .one(conn)
            .await
            .map_err(db_error(project_id))?;
        match env {
            Some(_) => Ok(()),
            None => Err(EdgeRuleServiceError::EnvironmentNotFound {
                project_id,
                environment_id,
            }),
        }
    }

    /// Serialises writers to one project's rules until the transaction ends.
    async fn lock_project(
        txn: &DatabaseTransaction,
        project_id: i32,
    ) -> Result<(), EdgeRuleServiceError> {
        txn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtext('project_edge_rules'), $1)",
            [project_id.into()],
        ))
        .await
        .map_err(db_error(project_id))?;
        Ok(())
    }

    /// Refuses a write that would take the project's rules over their
    /// pattern budget ([`PatternUsage`]): each regex, glob or
    /// case-insensitive substring condition is its own automaton in every
    /// proxy's memory. `except` is the rule being replaced. Runs under the
    /// project lock, so concurrent writes cannot both pass.
    /// Wakes the worker route sync when a write may change whether the
    /// project has an enabled rule (ADR-089 §8): workers serve no public
    /// route while any project has one, and the NOTIFY makes them pick that
    /// up now rather than at the end of their current long poll (up to 25 s
    /// later). Sent inside the write's transaction, so it is delivered only
    /// if the write commits.
    async fn announce_route_change(
        txn: &DatabaseTransaction,
        project_id: i32,
    ) -> Result<(), EdgeRuleServiceError> {
        let payload = serde_json::json!({
            "action": "UPDATE",
            "project_id": project_id,
            "field": "edge_rules.enabled",
        })
        .to_string();
        txn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_notify('project_route_change', $1)",
            [payload.into()],
        ))
        .await
        .map_err(db_error(project_id))?;
        Ok(())
    }

    /// Stores the rule set as it stands in `txn` as the next revision and
    /// prunes the oldest beyond [`MAX_REVISIONS_PER_PROJECT`]. Called by
    /// every write under the project lock, so numbers never race.
    async fn record_revision(
        txn: &DatabaseTransaction,
        project_id: i32,
        user_id: Option<i32>,
        change: String,
    ) -> Result<(), EdgeRuleServiceError> {
        let change: String = change.chars().take(200).collect();
        txn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            RECORD_REVISION_SQL,
            [project_id.into(), change.into(), user_id.into()],
        ))
        .await
        .map_err(db_error(project_id))?;
        txn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            PRUNE_REVISIONS_SQL,
            [project_id.into(), MAX_REVISIONS_PER_PROJECT.into()],
        ))
        .await
        .map_err(db_error(project_id))?;
        Ok(())
    }

    async fn check_pattern_budget(
        txn: &DatabaseTransaction,
        project_id: i32,
        except: Option<i32>,
        rule: &str,
        expression: &Group,
    ) -> Result<(), EdgeRuleServiceError> {
        let adding = PatternUsage::of(expression);
        if adding.is_empty() {
            return Ok(());
        }
        // At most `limits::RULES_PER_PROJECT` rows, read on a write only.
        let mut total = adding;
        for model in Self::rules(txn, project_id).await? {
            if Some(model.id) == except {
                continue;
            }
            let other =
                definition_from_model(&model).map_err(|e| EdgeRuleServiceError::Unreadable {
                    project_id,
                    rule_id: model.id,
                    reason: e.to_string(),
                })?;
            total = total.plus(PatternUsage::of(&other.expression));
        }
        Self::refuse_over_budget(project_id, rule, total)
    }

    fn refuse_over_budget(
        project_id: i32,
        rule: &str,
        usage: PatternUsage,
    ) -> Result<(), EdgeRuleServiceError> {
        match usage.over_budget() {
            None => Ok(()),
            Some(message) => Err(EdgeRuleServiceError::Invalid {
                project_id,
                rule: rule.to_string(),
                issues: vec![RuleIssue {
                    target: "expression".into(),
                    message,
                }],
            }),
        }
    }

    fn prepare(
        project_id: i32,
        name: Option<&str>,
        mut expression: Group,
        action: Action,
    ) -> Result<Prepared, EdgeRuleServiceError> {
        prepare_expression(&mut expression);
        let name = match name.map(str::trim) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => auto_name(&expression, &action),
        };
        let issues = validate_rule(&name, &expression, &action);
        if !issues.is_empty() {
            return Err(EdgeRuleServiceError::Invalid {
                project_id,
                rule: name,
                issues,
            });
        }
        // Validation and compilation share their checks; compiling here as
        // well means a rule the proxy could not load is never stored.
        let probe = RuleDefinition {
            id: 0,
            revision: 0,
            name: name.clone(),
            enabled: true,
            mode: Mode::Enforce,
            environment_id: None,
            expression: expression.clone(),
            action: action.clone(),
        };
        if let Err(e) = Program::compile(std::slice::from_ref(&probe)) {
            return Err(EdgeRuleServiceError::Invalid {
                project_id,
                rule: name,
                issues: vec![RuleIssue {
                    target: "expression".into(),
                    message: e.to_string(),
                }],
            });
        }
        Ok(Prepared {
            name,
            expression,
            action,
        })
    }

    fn to_response(
        model: &project_edge_rules::Model,
        stats: Option<&StatsRow>,
    ) -> Result<EdgeRuleResponse, EdgeRuleServiceError> {
        let definition =
            definition_from_model(model).map_err(|e| EdgeRuleServiceError::Unreadable {
                project_id: model.project_id,
                rule_id: model.id,
                reason: e.to_string(),
            })?;
        Ok(EdgeRuleResponse {
            id: model.id,
            project_id: model.project_id,
            environment_id: model.environment_id,
            position: model.position,
            name: model.name.clone(),
            enabled: model.enabled,
            mode: definition.mode,
            summary: describe_rule(&definition.expression, &definition.action),
            expression: definition.expression,
            action: definition.action,
            revision: model.revision,
            hits: stats.map_or(0, |s| s.hits),
            last_hit_at: stats.and_then(|s| s.last_hit_at),
            created_at: model.created_at,
            updated_at: model.updated_at,
        })
    }

    async fn stats<C: ConnectionTrait>(
        conn: &C,
        project_id: i32,
    ) -> Result<HashMap<i32, StatsRow>, EdgeRuleServiceError> {
        let rows = StatsRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT s.rule_id, s.hits, s.last_hit_at FROM project_edge_rule_stats s \
             JOIN project_edge_rules r ON r.id = s.rule_id WHERE r.project_id = $1",
            [project_id.into()],
        ))
        .all(conn)
        .await
        .map_err(db_error(project_id))?;
        Ok(rows.into_iter().map(|r| (r.rule_id, r)).collect())
    }

    async fn rules<C: ConnectionTrait>(
        conn: &C,
        project_id: i32,
    ) -> Result<Vec<project_edge_rules::Model>, EdgeRuleServiceError> {
        project_edge_rules::Entity::find()
            .filter(project_edge_rules::Column::ProjectId.eq(project_id))
            .order_by_asc(project_edge_rules::Column::Position)
            .order_by_asc(project_edge_rules::Column::Id)
            .all(conn)
            .await
            .map_err(db_error(project_id))
    }

    async fn find_rule<C: ConnectionTrait>(
        conn: &C,
        project_id: i32,
        rule_id: i32,
    ) -> Result<project_edge_rules::Model, EdgeRuleServiceError> {
        project_edge_rules::Entity::find_by_id(rule_id)
            .filter(project_edge_rules::Column::ProjectId.eq(project_id))
            .one(conn)
            .await
            .map_err(db_error(project_id))?
            .ok_or(EdgeRuleServiceError::RuleNotFound {
                project_id,
                rule_id,
            })
    }

    /// Checks a rule without storing it. Returns the name it would be saved
    /// with and its one-line summary.
    pub fn validate(
        &self,
        project_id: i32,
        name: Option<&str>,
        expression: Group,
        action: Action,
    ) -> Result<(String, String), EdgeRuleServiceError> {
        let prepared = Self::prepare(project_id, name, expression, action)?;
        let summary = describe_rule(&prepared.expression, &prepared.action);
        Ok((prepared.name, summary))
    }

    /// Every rule of the project in evaluation order.
    pub async fn list(
        &self,
        project_id: i32,
    ) -> Result<EdgeRuleListResponse, EdgeRuleServiceError> {
        let conn = self.db.as_ref();
        self.ensure_project(conn, project_id).await?;
        let rules = Self::rules(conn, project_id).await?;
        let stats = Self::stats(conn, project_id).await?;
        let rules = rules
            .iter()
            .map(|m| Self::to_response(m, stats.get(&m.id)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EdgeRuleListResponse {
            project_id,
            rules,
            max_rules: limits::RULES_PER_PROJECT,
        })
    }

    pub async fn get(
        &self,
        project_id: i32,
        rule_id: i32,
    ) -> Result<EdgeRuleResponse, EdgeRuleServiceError> {
        let conn = self.db.as_ref();
        self.ensure_project(conn, project_id).await?;
        let model = Self::find_rule(conn, project_id, rule_id).await?;
        let stats = Self::stats(conn, project_id).await?;
        Self::to_response(&model, stats.get(&rule_id))
    }

    pub async fn create(
        &self,
        project_id: i32,
        request: CreateEdgeRuleRequest,
        user_id: Option<i32>,
    ) -> Result<EdgeRuleResponse, EdgeRuleServiceError> {
        let prepared = Self::prepare(
            project_id,
            request.name.as_deref(),
            request.expression,
            request.action,
        )?;
        let txn = self.db.begin().await.map_err(db_error(project_id))?;
        self.ensure_project(&txn, project_id).await?;
        self.ensure_environment(&txn, project_id, request.environment_id)
            .await?;
        Self::lock_project(&txn, project_id).await?;

        let count = CountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT COUNT(*)::bigint AS count FROM project_edge_rules WHERE project_id = $1",
            [project_id.into()],
        ))
        .one(&txn)
        .await
        .map_err(db_error(project_id))?
        .map_or(0, |r| r.count);
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        if count >= limits::RULES_PER_PROJECT {
            return Err(EdgeRuleServiceError::TooManyRules {
                project_id,
                max: limits::RULES_PER_PROJECT,
            });
        }
        Self::check_pattern_budget(&txn, project_id, None, &prepared.name, &prepared.expression)
            .await?;
        let end = i32::try_from(count).unwrap_or(i32::MAX);
        let position = request.position.map_or(end, |p| p.clamp(0, end));
        if position < end {
            // Make room. The position constraint is deferred, so the
            // transient duplicates are fine until commit.
            txn.execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE project_edge_rules SET position = position + 1 \
                 WHERE project_id = $1 AND position >= $2",
                [project_id.into(), position.into()],
            ))
            .await
            .map_err(db_error(project_id))?;
        }

        let model = project_edge_rules::ActiveModel {
            project_id: Set(project_id),
            environment_id: Set(request.environment_id),
            position: Set(position),
            name: Set(prepared.name),
            enabled: Set(request.enabled),
            mode: Set(request.mode.as_str().to_string()),
            expression: Set(to_json(project_id, &prepared.expression)?),
            action: Set(to_json(project_id, &prepared.action)?),
            revision: Set(1),
            created_by: Set(user_id),
            updated_by: Set(user_id),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .map_err(db_error(project_id))?;
        if model.enabled {
            Self::announce_route_change(&txn, project_id).await?;
        }
        Self::record_revision(
            &txn,
            project_id,
            user_id,
            format!("Created rule '{}'", model.name),
        )
        .await?;
        txn.commit().await.map_err(db_error(project_id))?;
        Self::to_response(&model, None)
    }

    /// Applies `request` and returns the rule before and after.
    pub async fn update(
        &self,
        project_id: i32,
        rule_id: i32,
        request: UpdateEdgeRuleRequest,
        user_id: Option<i32>,
    ) -> Result<(EdgeRuleResponse, EdgeRuleResponse), EdgeRuleServiceError> {
        let txn = self.db.begin().await.map_err(db_error(project_id))?;
        self.ensure_project(&txn, project_id).await?;
        Self::lock_project(&txn, project_id).await?;
        let current = Self::find_rule(&txn, project_id, rule_id).await?;
        if let Some(expected) = request.revision {
            if expected != current.revision {
                return Err(EdgeRuleServiceError::RevisionConflict {
                    project_id,
                    rule_id,
                    expected,
                    current: current.revision,
                });
            }
        }
        let stats = Self::stats(&txn, project_id).await?;
        let before = Self::to_response(&current, stats.get(&rule_id))?;

        let expression_changed = request.expression.is_some();
        let expression = request
            .expression
            .unwrap_or_else(|| before.expression.clone());
        let action = request.action.unwrap_or_else(|| before.action.clone());
        let name = request.name.as_deref().or(Some(before.name.as_str()));
        let prepared = Self::prepare(project_id, name, expression, action)?;
        if expression_changed {
            Self::check_pattern_budget(
                &txn,
                project_id,
                Some(rule_id),
                &prepared.name,
                &prepared.expression,
            )
            .await?;
        }
        let environment_id = request.environment_id.unwrap_or(current.environment_id);
        self.ensure_environment(&txn, project_id, environment_id)
            .await?;

        let mut active: project_edge_rules::ActiveModel = current.clone().into();
        active.name = Set(prepared.name);
        active.enabled = Set(request.enabled.unwrap_or(current.enabled));
        active.mode = Set(request.mode.unwrap_or(before.mode).as_str().to_string());
        active.environment_id = Set(environment_id);
        active.expression = Set(to_json(project_id, &prepared.expression)?);
        active.action = Set(to_json(project_id, &prepared.action)?);
        active.revision = Set(current.revision + 1);
        active.updated_by = Set(user_id);
        active.updated_at = Set(Utc::now());
        let model = active.update(&txn).await.map_err(db_error(project_id))?;
        if model.enabled != current.enabled {
            Self::announce_route_change(&txn, project_id).await?;
        }
        Self::record_revision(
            &txn,
            project_id,
            user_id,
            format!("Edited rule '{}'", model.name),
        )
        .await?;
        txn.commit().await.map_err(db_error(project_id))?;
        let after = Self::to_response(&model, stats.get(&rule_id))?;
        Ok((before, after))
    }

    /// Deletes a rule and closes the gap it leaves in the order. Returns the
    /// deleted rule. With `expected_revision`, a rule that has changed since
    /// is refused with [`EdgeRuleServiceError::RevisionConflict`], so a
    /// confirmed deletion removes the rule that was reviewed.
    pub async fn delete(
        &self,
        project_id: i32,
        rule_id: i32,
        expected_revision: Option<i64>,
        user_id: Option<i32>,
    ) -> Result<EdgeRuleResponse, EdgeRuleServiceError> {
        let txn = self.db.begin().await.map_err(db_error(project_id))?;
        self.ensure_project(&txn, project_id).await?;
        Self::lock_project(&txn, project_id).await?;
        let current = Self::find_rule(&txn, project_id, rule_id).await?;
        if let Some(expected) = expected_revision {
            if expected != current.revision {
                return Err(EdgeRuleServiceError::RevisionConflict {
                    project_id,
                    rule_id,
                    expected,
                    current: current.revision,
                });
            }
        }
        // A rule stored in an unreadable form must still be deletable.
        let response = Self::to_response(&current, None).unwrap_or_else(|e| {
            warn!(project_id, rule_id, error = %e, "Deleting an unreadable edge rule");
            unreadable_placeholder(&current)
        });
        project_edge_rules::Entity::delete_by_id(rule_id)
            .exec(&txn)
            .await
            .map_err(db_error(project_id))?;
        txn.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE project_edge_rules SET position = position - 1 \
             WHERE project_id = $1 AND position > $2",
            [project_id.into(), current.position.into()],
        ))
        .await
        .map_err(db_error(project_id))?;
        if current.enabled {
            Self::announce_route_change(&txn, project_id).await?;
        }
        Self::record_revision(
            &txn,
            project_id,
            user_id,
            format!("Deleted rule '{}'", current.name),
        )
        .await?;
        txn.commit().await.map_err(db_error(project_id))?;
        Ok(response)
    }

    /// Puts the project's rules in the order of `rule_ids`, which must name
    /// every rule exactly once.
    pub async fn reorder(
        &self,
        project_id: i32,
        request: ReorderEdgeRulesRequest,
        user_id: Option<i32>,
    ) -> Result<EdgeRuleListResponse, EdgeRuleServiceError> {
        let txn = self.db.begin().await.map_err(db_error(project_id))?;
        self.ensure_project(&txn, project_id).await?;
        Self::lock_project(&txn, project_id).await?;
        let rules = Self::rules(&txn, project_id).await?;
        let existing: HashSet<i32> = rules.iter().map(|r| r.id).collect();
        let mut seen = HashSet::new();
        for id in &request.rule_ids {
            if !seen.insert(*id) {
                return Err(EdgeRuleServiceError::InvalidOrder {
                    project_id,
                    reason: format!("rule {id} is listed more than once"),
                });
            }
            if !existing.contains(id) {
                return Err(EdgeRuleServiceError::InvalidOrder {
                    project_id,
                    reason: format!("rule {id} does not belong to this project"),
                });
            }
        }
        if let Some(missing) = rules.iter().find(|r| !seen.contains(&r.id)) {
            return Err(EdgeRuleServiceError::InvalidOrder {
                project_id,
                reason: format!(
                    "rule {} is missing; list every rule of the project in its new order",
                    missing.id
                ),
            });
        }
        let current: HashMap<i32, i32> = rules.iter().map(|r| (r.id, r.position)).collect();
        for (index, id) in request.rule_ids.iter().enumerate() {
            let position = i32::try_from(index).unwrap_or(i32::MAX);
            if current.get(id) == Some(&position) {
                continue;
            }
            txn.execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE project_edge_rules SET position = $1 WHERE id = $2 AND project_id = $3",
                [position.into(), (*id).into(), project_id.into()],
            ))
            .await
            .map_err(db_error(project_id))?;
        }
        Self::record_revision(&txn, project_id, user_id, "Reordered the rules".into()).await?;
        txn.commit().await.map_err(db_error(project_id))?;
        self.list(project_id).await
    }

    /// What the proxy would do with `request.request`, against the saved
    /// rules or the ones supplied.
    pub async fn evaluate(
        &self,
        project_id: i32,
        request: EvaluateEdgeRulesRequest,
    ) -> Result<EvaluateEdgeRulesResponse, EdgeRuleServiceError> {
        let conn = self.db.as_ref();
        self.ensure_project(conn, project_id).await?;
        let definitions: Vec<RuleDefinition> = match request.rules {
            Some(inputs) => {
                let mut next_draft = -1;
                let mut out = Vec::with_capacity(inputs.len());
                if inputs.len() > limits::RULES_PER_PROJECT {
                    return Err(EdgeRuleServiceError::TooManyRules {
                        project_id,
                        max: limits::RULES_PER_PROJECT,
                    });
                }
                // Before anything is compiled: a dry run is open to readers,
                // so it gets the same budget as a write.
                let usage = inputs
                    .iter()
                    .map(|input| PatternUsage::of(&input.expression))
                    .fold(PatternUsage::default(), PatternUsage::plus);
                Self::refuse_over_budget(project_id, "rule set", usage)?;
                for input in inputs {
                    let id = input.id.unwrap_or_else(|| {
                        let id = next_draft;
                        next_draft -= 1;
                        id
                    });
                    let prepared = Self::prepare(
                        project_id,
                        input.name.as_deref(),
                        input.expression,
                        input.action,
                    )?;
                    out.push(RuleDefinition {
                        id,
                        revision: 0,
                        name: prepared.name,
                        enabled: input.enabled,
                        mode: input.mode,
                        environment_id: input.environment_id,
                        expression: prepared.expression,
                        action: prepared.action,
                    });
                }
                out
            }
            None => Self::rules(conn, project_id)
                .await?
                .iter()
                .map(|m| {
                    definition_from_model(m).map_err(|e| EdgeRuleServiceError::Unreadable {
                        project_id,
                        rule_id: m.id,
                        reason: e.to_string(),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        };
        let names: HashMap<i32, String> =
            definitions.iter().map(|d| (d.id, d.name.clone())).collect();
        let program =
            Program::compile(&definitions).map_err(|e| EdgeRuleServiceError::Invalid {
                project_id,
                rule: "rule set".into(),
                issues: vec![RuleIssue {
                    target: "expression".into(),
                    message: e.to_string(),
                }],
            })?;
        let sample = ParsedSample::new(request.request);
        let (evaluation, trace) = program.evaluate_traced(&sample);
        let summary = summarize_decision(&evaluation.decision, &names);
        Ok(EvaluateEdgeRulesResponse {
            summary,
            matched_rule_id: evaluation.first_match.map(|m| m.rule_id),
            matched_mode: evaluation.first_match.map(|m| m.mode),
            decision: evaluation.decision,
            trace: trace
                .into_iter()
                .map(|t| EvaluateTraceEntry {
                    name: names.get(&t.rule_id).cloned().unwrap_or_default(),
                    rule_id: t.rule_id,
                    outcome: t.outcome,
                })
                .collect(),
        })
    }

    /// A page of the project's revisions, newest first.
    pub async fn list_revisions(
        &self,
        project_id: i32,
        page: u64,
        per_page: u64,
    ) -> Result<EdgeRuleRevisionListResponse, EdgeRuleServiceError> {
        let conn = self.db.as_ref();
        self.ensure_project(conn, project_id).await?;
        let per_page = per_page.clamp(1, 100);
        let page = page.max(1);
        let total = CountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT COUNT(*)::bigint AS count FROM project_edge_rule_revisions \
             WHERE project_id = $1",
            [project_id.into()],
        ))
        .one(conn)
        .await
        .map_err(db_error(project_id))?
        .map_or(0, |r| r.count);
        let offset = i64::try_from((page - 1).saturating_mul(per_page)).unwrap_or(i64::MAX);
        let limit = i64::try_from(per_page).unwrap_or(100);
        let revisions = EdgeRuleRevisionSummary::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!("{REVISION_SUMMARY_SQL} ORDER BY r.number DESC LIMIT $2 OFFSET $3"),
            [project_id.into(), limit.into(), offset.into()],
        ))
        .all(conn)
        .await
        .map_err(db_error(project_id))?;
        Ok(EdgeRuleRevisionListResponse {
            project_id,
            revisions,
            total,
            page,
            per_page,
            max_revisions: MAX_REVISIONS_PER_PROJECT,
        })
    }

    /// One revision with its whole rule set.
    pub async fn get_revision(
        &self,
        project_id: i32,
        number: i32,
    ) -> Result<EdgeRuleRevisionResponse, EdgeRuleServiceError> {
        let conn = self.db.as_ref();
        self.ensure_project(conn, project_id).await?;
        let revision = EdgeRuleRevisionSummary::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!("{REVISION_SUMMARY_SQL} AND r.number = $2"),
            [project_id.into(), number.into()],
        ))
        .one(conn)
        .await
        .map_err(db_error(project_id))?
        .ok_or(EdgeRuleServiceError::RevisionNotFound { project_id, number })?;
        let rules = Self::revision_rules(conn, project_id, number).await?;
        Ok(EdgeRuleRevisionResponse {
            project_id,
            revision,
            rules,
        })
    }

    async fn revision_rules<C: ConnectionTrait>(
        conn: &C,
        project_id: i32,
        number: i32,
    ) -> Result<Vec<EdgeRuleRevisionRule>, EdgeRuleServiceError> {
        let row = project_edge_rule_revisions::Entity::find()
            .filter(project_edge_rule_revisions::Column::ProjectId.eq(project_id))
            .filter(project_edge_rule_revisions::Column::Number.eq(number))
            .one(conn)
            .await
            .map_err(db_error(project_id))?
            .ok_or(EdgeRuleServiceError::RevisionNotFound { project_id, number })?;
        decode_revision_rules(project_id, number, row.rules)
    }

    /// Puts the project's rules back to revision `number` and records that
    /// as a new revision. Rules keep their ids (and so their hit counters and
    /// log attribution); rules added since are deleted. Every restored rule is
    /// validated against today's engine and limits first, so a revision that
    /// no longer fits is refused whole instead of applied in part.
    pub async fn restore(
        &self,
        project_id: i32,
        number: i32,
        user_id: Option<i32>,
    ) -> Result<EdgeRuleListResponse, EdgeRuleServiceError> {
        let txn = self.db.begin().await.map_err(db_error(project_id))?;
        self.ensure_project(&txn, project_id).await?;
        Self::lock_project(&txn, project_id).await?;
        let snapshot = Self::revision_rules(&txn, project_id, number).await?;
        if snapshot.len() > limits::RULES_PER_PROJECT {
            return Err(EdgeRuleServiceError::TooManyRules {
                project_id,
                max: limits::RULES_PER_PROJECT,
            });
        }
        // A rule scoped to an environment deleted since is refused, not
        // silently widened to every environment. One check per environment.
        let environments: HashSet<i32> = snapshot.iter().filter_map(|r| r.environment_id).collect();
        for environment_id in environments {
            self.ensure_environment(&txn, project_id, Some(environment_id))
                .await?;
        }
        let mut usage = PatternUsage::default();
        let mut prepared = Vec::with_capacity(snapshot.len());
        for rule in snapshot {
            let ready = Self::prepare(
                project_id,
                Some(rule.name.as_str()),
                rule.expression.clone(),
                rule.action.clone(),
            )?;
            usage = usage.plus(PatternUsage::of(&ready.expression));
            prepared.push((rule, ready));
        }
        Self::refuse_over_budget(project_id, "restored rule set", usage)?;

        let keep: Vec<i32> = prepared.iter().map(|(rule, _)| rule.id).collect();
        project_edge_rules::Entity::delete_many()
            .filter(project_edge_rules::Column::ProjectId.eq(project_id))
            .filter(project_edge_rules::Column::Id.is_not_in(keep))
            .exec(&txn)
            .await
            .map_err(db_error(project_id))?;
        let existing: HashMap<i32, project_edge_rules::Model> = Self::rules(&txn, project_id)
            .await?
            .into_iter()
            .map(|m| (m.id, m))
            .collect();
        for (index, (rule, ready)) in prepared.into_iter().enumerate() {
            let position = i32::try_from(index).unwrap_or(i32::MAX);
            let expression = to_json(project_id, &ready.expression)?;
            let action = to_json(project_id, &ready.action)?;
            match existing.get(&rule.id) {
                Some(current) => {
                    let mut active: project_edge_rules::ActiveModel = current.clone().into();
                    active.position = Set(position);
                    active.name = Set(ready.name);
                    active.enabled = Set(rule.enabled);
                    active.mode = Set(rule.mode.as_str().to_string());
                    active.environment_id = Set(rule.environment_id);
                    active.expression = Set(expression);
                    active.action = Set(action);
                    // Bumped so a challenge clearance or an open editor of
                    // the old rule is invalidated like on any edit.
                    active.revision = Set(current.revision + 1);
                    active.updated_by = Set(user_id);
                    active.updated_at = Set(Utc::now());
                    active.update(&txn).await.map_err(db_error(project_id))?;
                }
                // Ids are never reused by the sequence, so the old id is free.
                None => {
                    project_edge_rules::ActiveModel {
                        id: Set(rule.id),
                        project_id: Set(project_id),
                        environment_id: Set(rule.environment_id),
                        position: Set(position),
                        name: Set(ready.name),
                        enabled: Set(rule.enabled),
                        mode: Set(rule.mode.as_str().to_string()),
                        expression: Set(expression),
                        action: Set(action),
                        revision: Set(1),
                        created_by: Set(user_id),
                        updated_by: Set(user_id),
                        ..Default::default()
                    }
                    .insert(&txn)
                    .await
                    .map_err(db_error(project_id))?;
                }
            }
        }
        Self::announce_route_change(&txn, project_id).await?;
        Self::record_revision(
            &txn,
            project_id,
            user_id,
            format!("Restored revision {number}"),
        )
        .await?;
        txn.commit().await.map_err(db_error(project_id))?;
        self.list(project_id).await
    }
}

/// Decodes a stored snapshot; any rule this version cannot read makes the
/// whole revision unreadable rather than silently dropping that rule.
fn decode_revision_rules(
    project_id: i32,
    number: i32,
    rules: serde_json::Value,
) -> Result<Vec<EdgeRuleRevisionRule>, EdgeRuleServiceError> {
    #[derive(Deserialize)]
    struct Stored {
        id: i32,
        environment_id: Option<i32>,
        name: String,
        enabled: bool,
        mode: String,
        expression: Group,
        action: Action,
    }
    let unreadable = |reason: String| EdgeRuleServiceError::UnreadableRevision {
        project_id,
        number,
        reason,
    };
    let stored: Vec<Stored> =
        serde_json::from_value(rules).map_err(|e| unreadable(e.to_string()))?;
    stored
        .into_iter()
        .map(|rule| {
            let mode = Mode::parse(&rule.mode).ok_or_else(|| {
                unreadable(format!("rule {} has unknown mode '{}'", rule.id, rule.mode))
            })?;
            Ok(EdgeRuleRevisionRule {
                id: rule.id,
                environment_id: rule.environment_id,
                name: rule.name,
                enabled: rule.enabled,
                mode,
                expression: rule.expression,
                action: rule.action,
            })
        })
        .collect()
}

fn to_json<T: Serialize>(
    project_id: i32,
    value: &T,
) -> Result<serde_json::Value, EdgeRuleServiceError> {
    serde_json::to_value(value).map_err(|e| EdgeRuleServiceError::Invalid {
        project_id,
        rule: "rule".into(),
        issues: vec![RuleIssue {
            target: "expression".into(),
            message: format!("could not be encoded: {e}"),
        }],
    })
}

fn unreadable_placeholder(model: &project_edge_rules::Model) -> EdgeRuleResponse {
    EdgeRuleResponse {
        id: model.id,
        project_id: model.project_id,
        environment_id: model.environment_id,
        position: model.position,
        name: model.name.clone(),
        enabled: model.enabled,
        mode: Mode::parse(&model.mode).unwrap_or(Mode::LogOnly),
        expression: Group {
            id: String::new(),
            combinator: temps_edge_rules::Combinator::All,
            children: Vec::new(),
        },
        action: Action::Log,
        revision: model.revision,
        summary: "Unreadable rule".into(),
        hits: 0,
        last_hit_at: None,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn summarize_decision(decision: &Decision, names: &HashMap<i32, String>) -> String {
    let rule = |id: &i32| match names.get(id) {
        Some(name) if !name.is_empty() => format!("rule '{name}'"),
        _ => format!("rule {id}"),
    };
    match decision {
        Decision::Forward => "No rule decided; the request continues to the app.".into(),
        Decision::Block {
            rule_id, status, ..
        } => format!("Blocked with {status} by {}.", rule(rule_id)),
        Decision::Allow { rule_id } => format!(
            "Allowed by {}; the remaining edge rules are skipped.",
            rule(rule_id)
        ),
        Decision::Challenge { rule_id } => {
            format!("Challenged by {} before reaching the app.", rule(rule_id))
        }
        Decision::Redirect {
            rule_id,
            status,
            location,
        } => format!(
            "Redirected with {status} to {location} by {}.",
            rule(rule_id)
        ),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use temps_test_utils::{is_container_runtime_unavailable, TestDatabase};

    pub(crate) async fn database() -> Option<TestDatabase> {
        match TestDatabase::with_migrations().await {
            Ok(db) => Some(db),
            Err(error) if is_container_runtime_unavailable(&error.to_string()) => {
                eprintln!("Docker not available, skipping edge rule database test");
                None
            }
            Err(error) => panic!("test database: {error}"),
        }
    }

    /// A project with one environment; returns `(project_id, environment_id)`.
    pub(crate) async fn project(db: &TestDatabase, slug: &str) -> (i32, i32) {
        let rows = db
            .query_sql(&format!(
                "WITH p AS (INSERT INTO projects (name, repo_name, repo_owner, directory, \
                 main_branch, preset, slug, created_at, updated_at) VALUES ('{slug}', '{slug}', \
                 'owner', '.', 'main', 'dockerfile', '{slug}', NOW(), NOW()) RETURNING id) \
                 INSERT INTO environments (name, slug, subdomain, host, upstreams, project_id, \
                 created_at, updated_at) SELECT 'production', 'production', '{slug}', \
                 '{slug}.example.test', '[]'::json, p.id, NOW(), NOW() FROM p \
                 RETURNING project_id, id"
            ))
            .await
            .expect("insert project and environment");
        let row = rows.first().expect("one environment row");
        (
            row.try_get("", "project_id").expect("project_id"),
            row.try_get("", "id").expect("environment id"),
        )
    }

    pub(crate) fn block_admin() -> CreateEdgeRuleRequest {
        serde_json::from_value(serde_json::json!({
            "mode": "enforce",
            "expression": {"combinator": "all", "children": [
                {"kind": "condition", "field": "path", "op": "starts_with",
                 "values": ["/admin"], "case_insensitive": true}
            ]},
            "action": {"type": "block", "status": 403}
        }))
        .expect("valid request")
    }

    fn rule_on(path: &str, action: serde_json::Value) -> CreateEdgeRuleRequest {
        serde_json::from_value(serde_json::json!({
            "mode": "enforce",
            "expression": {"combinator": "all", "children": [
                {"kind": "condition", "field": "path", "op": "starts_with", "values": [path]}
            ]},
            "action": action
        }))
        .expect("valid request")
    }

    fn sample(url: &str) -> SampleRequest {
        SampleRequest {
            url: url.into(),
            ..Default::default()
        }
    }

    async fn revision_numbers(service: &EdgeRuleService, project_id: i32) -> Vec<(i32, String)> {
        service
            .list_revisions(project_id, 1, 100)
            .await
            .expect("list revisions")
            .revisions
            .into_iter()
            .map(|r| (r.number, r.change))
            .collect()
    }

    #[tokio::test]
    async fn every_write_records_a_numbered_revision_and_restore_brings_rules_back() {
        let Some(db) = database().await else {
            return;
        };
        let (project_id, _) = project(&db, "edge-revisions").await;
        let service = EdgeRuleService::new(db.connection_arc());

        let first = service
            .create(project_id, block_admin(), None)
            .await
            .expect("create first");
        let second = service
            .create(
                project_id,
                rule_on("/api", serde_json::json!({"type": "log"})),
                None,
            )
            .await
            .expect("create second");
        service
            .delete(project_id, first.id, None, None)
            .await
            .expect("delete first");

        let history = revision_numbers(&service, project_id).await;
        assert_eq!(
            history.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            [3, 2, 1],
            "newest first, one per write: {history:?}"
        );
        assert!(history[0].1.starts_with("Deleted rule"), "{history:?}");

        let two = service
            .get_revision(project_id, 2)
            .await
            .expect("revision 2");
        assert_eq!(
            two.rules.iter().map(|r| r.id).collect::<Vec<_>>(),
            [first.id, second.id]
        );

        let restored = service
            .restore(project_id, 2, None)
            .await
            .expect("restore revision 2");
        assert_eq!(
            restored.rules.iter().map(|r| r.id).collect::<Vec<_>>(),
            [first.id, second.id],
            "the deleted rule comes back with its id, in its old position"
        );
        assert_eq!(restored.rules[0].name, first.name);
        let history = revision_numbers(&service, project_id).await;
        assert_eq!(history[0], (4, "Restored revision 2".to_string()));

        // Restoring revision 1 removes the rule added after it.
        let only_first = service
            .restore(project_id, 1, None)
            .await
            .expect("restore revision 1");
        assert_eq!(
            only_first.rules.iter().map(|r| r.id).collect::<Vec<_>>(),
            [first.id]
        );
        assert!(matches!(
            service.restore(project_id, 99, None).await,
            Err(EdgeRuleServiceError::RevisionNotFound { number: 99, .. })
        ));
    }

    #[tokio::test]
    async fn only_the_last_revisions_are_kept() {
        let Some(db) = database().await else {
            return;
        };
        let (project_id, _) = project(&db, "edge-revisions-cap").await;
        let service = EdgeRuleService::new(db.connection_arc());
        let rule = service
            .create(project_id, block_admin(), None)
            .await
            .expect("create");
        let writes = MAX_REVISIONS_PER_PROJECT + 4;
        for i in 1..writes {
            service
                .update(
                    project_id,
                    rule.id,
                    UpdateEdgeRuleRequest {
                        enabled: Some(i % 2 == 0),
                        ..Default::default()
                    },
                    None,
                )
                .await
                .expect("update");
        }
        let page = service
            .list_revisions(project_id, 1, 1)
            .await
            .expect("list revisions");
        assert_eq!(page.total, i64::from(MAX_REVISIONS_PER_PROJECT));
        assert_eq!(page.revisions[0].number, writes, "the newest is kept");
        let oldest = service
            .list_revisions(
                project_id,
                u64::try_from(MAX_REVISIONS_PER_PROJECT).unwrap_or(100),
                1,
            )
            .await
            .expect("last page");
        assert_eq!(
            oldest.revisions[0].number,
            writes - MAX_REVISIONS_PER_PROJECT + 1
        );
    }

    #[test]
    fn a_revision_with_an_unknown_mode_is_unreadable_not_partially_restored() {
        let rules = serde_json::json!([{
            "id": 1, "environment_id": null, "name": "r", "enabled": true, "mode": "panic",
            "expression": {"combinator": "all", "children": []},
            "action": {"type": "log"}
        }]);
        assert!(matches!(
            decode_revision_rules(3, 7, rules),
            Err(EdgeRuleServiceError::UnreadableRevision {
                project_id: 3,
                number: 7,
                ..
            })
        ));
    }

    #[test]
    fn invalid_rules_are_refused_with_every_issue() {
        let request: CreateEdgeRuleRequest = serde_json::from_value(serde_json::json!({
            "expression": {"combinator": "all", "children": [
                {"kind": "condition", "field": "ip", "op": "in_cidr", "values": ["not-an-ip"]},
                {"kind": "condition", "field": "path", "op": "regex", "values": ["("]}
            ]},
            "action": {"type": "block", "status": 200}
        }))
        .expect("parses");
        match EdgeRuleService::prepare(7, None, request.expression, request.action) {
            Err(EdgeRuleServiceError::Invalid {
                project_id: 7,
                issues,
                ..
            }) => {
                assert!(issues.len() >= 3, "{issues:?}");
                assert!(issues.iter().any(|i| i.target == "action"));
            }
            Err(other) => panic!("expected Invalid, got {other}"),
            Ok(_) => panic!("expected Invalid, got a prepared rule"),
        }
    }

    #[test]
    fn an_empty_name_is_generated_from_the_rule() {
        let request = block_admin();
        let prepared = EdgeRuleService::prepare(1, Some("  "), request.expression, request.action)
            .unwrap_or_else(|e| panic!("valid rule: {e}"));
        assert_eq!(
            prepared.name,
            "Block when path starts with /admin (ignoring case)"
        );
        // Node ids are assigned so issues and the editor can address them.
        assert!(!prepared.expression.id.is_empty());
    }

    #[test]
    fn update_distinguishes_a_null_environment_from_an_absent_one() {
        let absent: UpdateEdgeRuleRequest =
            serde_json::from_str(r#"{"enabled": false}"#).expect("parses");
        assert_eq!(absent.environment_id, None);
        let null: UpdateEdgeRuleRequest =
            serde_json::from_str(r#"{"environment_id": null}"#).expect("parses");
        assert_eq!(null.environment_id, Some(None));
        let set: UpdateEdgeRuleRequest =
            serde_json::from_str(r#"{"environment_id": 4}"#).expect("parses");
        assert_eq!(set.environment_id, Some(Some(4)));
    }

    /// The `project_route_change` notifications naming `project_id`'s
    /// edge rules that arrive within a short window.
    async fn route_announcements(
        listener: &mut sqlx::postgres::PgListener,
        project_id: i32,
    ) -> usize {
        let mut count = 0;
        while let Ok(received) =
            tokio::time::timeout(std::time::Duration::from_millis(400), listener.recv()).await
        {
            let notification = received.unwrap_or_else(|e| panic!("receive notification: {e}"));
            let payload: serde_json::Value = serde_json::from_str(notification.payload())
                .unwrap_or_else(|e| panic!("route change payload: {e}"));
            if payload["field"] == "edge_rules.enabled" && payload["project_id"] == project_id {
                count += 1;
            }
        }
        count
    }

    #[tokio::test]
    async fn writes_that_change_enabled_rules_wake_the_worker_route_sync() {
        let Some(test_db) = database().await else {
            return;
        };
        let service = EdgeRuleService::new(test_db.connection_arc());
        let (project_id, _) = project(&test_db, "edge-notify").await;
        let mut listener = sqlx::postgres::PgListener::connect(&test_db.database_url)
            .await
            .unwrap_or_else(|e| panic!("connect listener: {e}"));
        listener
            .listen("project_route_change")
            .await
            .unwrap_or_else(|e| panic!("listen: {e}"));

        // A disabled rule does not change what workers may serve.
        let mut disabled = block_admin();
        disabled.enabled = false;
        let disabled = service
            .create(project_id, disabled, None)
            .await
            .unwrap_or_else(|e| panic!("create disabled: {e}"));
        assert_eq!(route_announcements(&mut listener, project_id).await, 0);
        let update = |patch: serde_json::Value| -> UpdateEdgeRuleRequest {
            serde_json::from_value(patch).unwrap_or_else(|e| panic!("update request: {e}"))
        };
        service
            .update(
                project_id,
                disabled.id,
                update(serde_json::json!({"name": "Renamed"})),
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("rename: {e}"));
        assert_eq!(route_announcements(&mut listener, project_id).await, 0);

        // Enabling, creating enabled, and deleting an enabled rule all do.
        service
            .update(
                project_id,
                disabled.id,
                update(serde_json::json!({"enabled": true})),
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("enable: {e}"));
        assert_eq!(route_announcements(&mut listener, project_id).await, 1);
        let enabled = service
            .create(
                project_id,
                rule_on("/api", serde_json::json!({"type": "log"})),
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("create enabled: {e}"));
        assert_eq!(route_announcements(&mut listener, project_id).await, 1);
        service
            .delete(project_id, enabled.id, None, None)
            .await
            .unwrap_or_else(|e| panic!("delete enabled: {e}"));
        assert_eq!(route_announcements(&mut listener, project_id).await, 1);
        service
            .update(
                project_id,
                disabled.id,
                update(serde_json::json!({"enabled": false})),
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("disable: {e}"));
        assert_eq!(route_announcements(&mut listener, project_id).await, 1);

        // A refused write announces nothing: the NOTIFY rolls back with it.
        let refused = service
            .update(
                project_id,
                disabled.id,
                update(serde_json::json!({"enabled": true, "revision": 1})),
                None,
            )
            .await;
        assert!(matches!(
            refused,
            Err(EdgeRuleServiceError::RevisionConflict { .. })
        ));
        service
            .delete(project_id, disabled.id, None, None)
            .await
            .unwrap_or_else(|e| panic!("delete disabled: {e}"));
        assert_eq!(route_announcements(&mut listener, project_id).await, 0);
    }

    #[tokio::test]
    async fn rules_are_created_ordered_updated_and_deleted() {
        let Some(test_db) = database().await else {
            return;
        };
        let service = EdgeRuleService::new(test_db.connection_arc());
        let (project_id, environment_id) = project(&test_db, "edge-crud").await;

        let first = service
            .create(project_id, block_admin(), None)
            .await
            .unwrap_or_else(|e| panic!("create: {e}"));
        assert_eq!(first.position, 0);
        assert_eq!(first.revision, 1);
        assert_eq!(
            first.summary,
            "Block with 403 when path starts with /admin (ignoring case)"
        );

        let mut second = rule_on("/api", serde_json::json!({"type": "log"}));
        second.position = Some(0);
        second.environment_id = Some(environment_id);
        let second = service
            .create(project_id, second, None)
            .await
            .unwrap_or_else(|e| panic!("create at the top: {e}"));
        let listed = service
            .list(project_id)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let order: Vec<i32> = listed.rules.iter().map(|r| r.id).collect();
        assert_eq!(order, vec![second.id, first.id]);
        assert_eq!(listed.max_rules, 100);

        // A stale revision is refused; the current one applies.
        let stale = service
            .update(
                project_id,
                first.id,
                UpdateEdgeRuleRequest {
                    revision: Some(0),
                    enabled: Some(false),
                    ..Default::default()
                },
                None,
            )
            .await;
        assert!(matches!(
            stale,
            Err(EdgeRuleServiceError::RevisionConflict {
                expected: 0,
                current: 1,
                ..
            })
        ));
        let (before, after) = service
            .update(
                project_id,
                first.id,
                UpdateEdgeRuleRequest {
                    revision: Some(1),
                    mode: Some(Mode::LogOnly),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("update: {e}"));
        assert_eq!(before.mode, Mode::Enforce);
        assert_eq!(after.mode, Mode::LogOnly);
        assert_eq!(after.revision, 2);
        assert_eq!(after.name, before.name);

        let reordered = service
            .reorder(
                project_id,
                ReorderEdgeRulesRequest {
                    rule_ids: vec![first.id, second.id],
                },
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("reorder: {e}"));
        let order: Vec<(i32, i32)> = reordered.rules.iter().map(|r| (r.id, r.position)).collect();
        assert_eq!(order, vec![(first.id, 0), (second.id, 1)]);

        let incomplete = service
            .reorder(
                project_id,
                ReorderEdgeRulesRequest {
                    rule_ids: vec![first.id],
                },
                None,
            )
            .await;
        assert!(matches!(
            incomplete,
            Err(EdgeRuleServiceError::InvalidOrder { .. })
        ));

        // A deletion bound to a revision refuses a rule that changed since.
        let current = service
            .get(project_id, first.id)
            .await
            .unwrap_or_else(|e| panic!("get: {e}"));
        let stale = service
            .delete(project_id, first.id, Some(current.revision - 1), None)
            .await;
        assert!(matches!(
            stale,
            Err(EdgeRuleServiceError::RevisionConflict { .. })
        ));
        service
            .delete(project_id, first.id, Some(current.revision), None)
            .await
            .unwrap_or_else(|e| panic!("delete: {e}"));
        let listed = service
            .list(project_id)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(listed.rules.len(), 1);
        assert_eq!(listed.rules[0].position, 0, "the gap is closed");

        let missing = service.delete(project_id, first.id, None, None).await;
        assert!(matches!(
            missing,
            Err(EdgeRuleServiceError::RuleNotFound { rule_id, .. }) if rule_id == first.id
        ));
    }

    #[tokio::test]
    async fn writes_are_scoped_to_the_project_and_its_environments() {
        let Some(test_db) = database().await else {
            return;
        };
        let service = EdgeRuleService::new(test_db.connection_arc());
        let (project_a, _) = project(&test_db, "edge-scope-a").await;
        let (project_b, environment_b) = project(&test_db, "edge-scope-b").await;

        let mut foreign_env = block_admin();
        foreign_env.environment_id = Some(environment_b);
        assert!(matches!(
            service.create(project_a, foreign_env, None).await,
            Err(EdgeRuleServiceError::EnvironmentNotFound { .. })
        ));

        let rule = service
            .create(project_b, block_admin(), None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(
            service.get(project_a, rule.id).await,
            Err(EdgeRuleServiceError::RuleNotFound { .. })
        ));
        assert!(matches!(
            service.list(999_999).await,
            Err(EdgeRuleServiceError::ProjectNotFound {
                project_id: 999_999
            })
        ));
    }

    #[tokio::test]
    async fn the_rule_limit_is_enforced() {
        let Some(test_db) = database().await else {
            return;
        };
        let service = EdgeRuleService::new(test_db.connection_arc());
        let (project_id, _) = project(&test_db, "edge-limit").await;
        for _ in 0..limits::RULES_PER_PROJECT {
            service
                .create(project_id, block_admin(), None)
                .await
                .unwrap_or_else(|e| panic!("{e}"));
        }
        assert!(matches!(
            service.create(project_id, block_admin(), None).await,
            Err(EdgeRuleServiceError::TooManyRules { max: 100, .. })
        ));
    }

    /// Regex conditions cost memory in every proxy, so a project has a
    /// budget across all of its rules, counted through nested groups.
    #[tokio::test]
    async fn regex_conditions_are_capped_per_project() {
        let Some(test_db) = database().await else {
            return;
        };
        let service = EdgeRuleService::new(test_db.connection_arc());
        let (project_id, _) = project(&test_db, "edge-regex").await;
        let regexes = |n: usize| -> CreateEdgeRuleRequest {
            let children: Vec<serde_json::Value> = (0..n)
                .map(|i| serde_json::json!({"kind": "condition", "field": "path", "op": "regex", "values": [format!("^/r{i}/")]}))
                .collect();
            serde_json::from_value(serde_json::json!({
                "expression": {"combinator": "any", "children": [
                    {"kind": "group", "combinator": "any", "children": children}
                ]},
                "action": {"type": "log"}
            }))
            .expect("valid request")
        };
        let per_rule = limits::REGEX_CONDITIONS_PER_PROJECT / 2;
        let first = service
            .create(project_id, regexes(per_rule), None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(first.mode, Mode::LogOnly, "new rules start log-only");
        service
            .create(project_id, regexes(per_rule), None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        match service.create(project_id, regexes(1), None).await {
            Err(EdgeRuleServiceError::Invalid { issues, .. }) => {
                assert!(issues[0].message.contains("regex conditions"), "{issues:?}");
            }
            other => panic!("expected the regex budget to refuse it, got {other:?}"),
        }
        // Rules without a regex are unaffected, and so is a change that
        // keeps a rule's own regex count.
        service
            .create(project_id, block_admin(), None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let same = regexes(per_rule);
        service
            .update(
                project_id,
                first.id,
                UpdateEdgeRuleRequest {
                    expression: Some(same.expression),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("an update replaces the rule's own budget: {e}"));
    }

    fn conditions(field_op: (&str, &str), n: usize, offset: usize) -> serde_json::Value {
        let (field, op) = field_op;
        let children: Vec<serde_json::Value> = (0..n)
            .map(|i| serde_json::json!({"kind": "condition", "field": field, "op": op, "values": [format!("/p{}/*", i + offset)]}))
            .collect();
        serde_json::json!({"combinator": "any", "children": children})
    }

    #[tokio::test]
    async fn globs_and_case_insensitive_matches_share_a_pattern_budget() {
        let Some(test_db) = database().await else {
            return;
        };
        let service = EdgeRuleService::new(test_db.connection_arc());
        let (project_id, _) = project(&test_db, "edge-patterns").await;
        let per_rule = limits::CONDITIONS_PER_RULE;
        let full_rules = limits::PATTERN_CONDITIONS_PER_PROJECT / per_rule;
        let rule = |n: usize, offset: usize| -> CreateEdgeRuleRequest {
            serde_json::from_value(serde_json::json!({
                "expression": conditions(("path", "glob"), n, offset),
                "action": {"type": "log"}
            }))
            .expect("valid request")
        };
        for r in 0..full_rules {
            service
                .create(project_id, rule(per_rule, r * per_rule), None)
                .await
                .unwrap_or_else(|e| panic!("rule {r} is within the budget: {e}"));
        }
        match service.create(project_id, rule(1, 10_000), None).await {
            Err(EdgeRuleServiceError::Invalid { issues, .. }) => {
                assert!(
                    issues[0].message.contains("pattern conditions"),
                    "{issues:?}"
                );
            }
            other => panic!("expected the pattern budget to refuse it, got {other:?}"),
        }
        // `in` compares strings without an automaton, so it is not budgeted.
        let exact: CreateEdgeRuleRequest = serde_json::from_value(serde_json::json!({
            "expression": {"combinator": "all", "children": [
                {"kind": "condition", "field": "path", "op": "in", "values": ["/a", "/b"]}
            ]},
            "action": {"type": "log"}
        }))
        .expect("valid request");
        service
            .create(project_id, exact, None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    }

    #[tokio::test]
    async fn the_dry_run_refuses_drafts_over_the_pattern_budget() {
        let Some(test_db) = database().await else {
            return;
        };
        let service = EdgeRuleService::new(test_db.connection_arc());
        let (project_id, _) = project(&test_db, "edge-dry-run-budget").await;
        let half = limits::REGEX_CONDITIONS_PER_PROJECT / 2 + 1;
        let drafts: Vec<EvaluateRuleInput> = serde_json::from_value(serde_json::json!([
            {"expression": conditions(("path", "regex"), half, 0), "action": {"type": "log"}},
            {"expression": conditions(("path", "regex"), half, half), "action": {"type": "log"}}
        ]))
        .expect("drafts parse");
        match service
            .evaluate(
                project_id,
                EvaluateEdgeRulesRequest {
                    request: sample("https://app.example.test/"),
                    rules: Some(drafts),
                },
            )
            .await
        {
            Err(EdgeRuleServiceError::Invalid { issues, .. }) => {
                assert!(issues[0].message.contains("regex conditions"), "{issues:?}");
            }
            other => panic!("expected the dry run to refuse the drafts, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_dry_run_evaluates_saved_rules_and_drafts() {
        let Some(test_db) = database().await else {
            return;
        };
        let service = EdgeRuleService::new(test_db.connection_arc());
        let (project_id, _) = project(&test_db, "edge-dry-run").await;
        let saved = service
            .create(project_id, block_admin(), None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));

        // Paths ignore case by default for this rule.
        let result = service
            .evaluate(
                project_id,
                EvaluateEdgeRulesRequest {
                    request: sample("https://app.example.test/ADMIN/users"),
                    rules: None,
                },
            )
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            result.decision,
            Decision::Block {
                rule_id: saved.id,
                status: 403,
                body: String::new()
            }
        );
        assert_eq!(result.matched_rule_id, Some(saved.id));
        assert!(
            result.summary.starts_with("Blocked with 403"),
            "{}",
            result.summary
        );

        let drafts: Vec<EvaluateRuleInput> = serde_json::from_value(serde_json::json!([
            {"expression": {"combinator": "all", "children": [
                {"kind": "condition", "field": "path", "op": "eq", "values": ["/old"]}
            ]}, "action": {"type": "redirect", "target": "/new?{query}", "status": 308}}
        ]))
        .expect("drafts parse");
        let result = service
            .evaluate(
                project_id,
                EvaluateEdgeRulesRequest {
                    request: sample("https://app.example.test/old?x=1"),
                    rules: Some(drafts),
                },
            )
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            result.decision,
            Decision::Redirect {
                rule_id: -1,
                status: 308,
                location: "/new?x=1".into()
            }
        );
        assert_eq!(result.trace.len(), 1);
        assert_eq!(result.trace[0].outcome, RuleOutcome::Decided);
    }
}
