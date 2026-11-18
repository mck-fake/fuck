// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Registers the approval service, the `DeploymentGate` the deployments
//! plugin consults, and the routes. Must come after `DeploymentsPlugin`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use sea_orm::DatabaseConnection;
use temps_core::plugin::{
    PluginContext, PluginError, PluginRoutes, ServiceRegistrationContext, TempsPlugin,
};
use temps_core::{AuditLogger, JobQueue};
use temps_deployments::DeploymentService;
use utoipa::openapi::OpenApi;
use utoipa::OpenApi as OpenApiTrait;

use crate::handlers::{configure_routes, ApprovalsApiDoc, ApprovalsAppState};
use crate::service::{ApprovalGate, ApprovalService};

pub struct ApprovalsPlugin;

impl ApprovalsPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ApprovalsPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl TempsPlugin for ApprovalsPlugin {
    fn name(&self) -> &'static str {
        "approvals"
    }

    fn register_services<'a>(
        &'a self,
        context: &'a ServiceRegistrationContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), PluginError>> + Send + 'a>> {
        Box::pin(async move {
            let service = Arc::new(ApprovalService::new(
                context.require_service::<DatabaseConnection>(),
                context.require_service::<dyn JobQueue>(),
                context.require_service::<DeploymentService>(),
            ));
            // The deployments plugin picks the gate up in its second phase.
            let gate: Arc<dyn temps_core::DeploymentGate> =
                Arc::new(ApprovalGate::new(service.clone()));
            context.register_service(gate);
            context.register_service(service);
            Ok(())
        })
    }

    fn configure_routes(&self, context: &PluginContext) -> Option<PluginRoutes> {
        let state = Arc::new(ApprovalsAppState {
            approval_service: context.require_service::<ApprovalService>(),
            audit_service: context.require_service::<dyn AuditLogger>(),
            project_access_checker: context.get_service::<dyn temps_core::ProjectAccessChecker>(),
        });
        Some(PluginRoutes::new(configure_routes().with_state(state)))
    }

    fn openapi_schema(&self) -> Option<OpenApi> {
        Some(<ApprovalsApiDoc as OpenApiTrait>::openapi())
    }
}
