// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Deployment approvals: an environment can hold every deployment until a
//! person approves it. Implemented on the core `DeploymentGate` hook.

pub mod error;
pub mod handlers;
pub mod plugin;
pub mod service;

pub use plugin::ApprovalsPlugin;
