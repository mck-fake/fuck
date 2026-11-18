// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Default)]
struct CapturingQueue {
    jobs: Mutex<Vec<Job>>,
    fail: bool,
}

#[async_trait]
impl JobQueue for CapturingQueue {
    async fn send(&self, job: Job) -> Result<(), temps_core::QueueError> {
        if self.fail {
            return Err(temps_core::QueueError::SendError("queue offline".into()));
        }
        self.jobs.lock().expect("jobs lock").push(job);
        Ok(())
    }

    fn subscribe(&self) -> Box<dyn temps_core::JobReceiver> {
        unimplemented!("the approval service only publishes")
    }
}

#[derive(Default)]
struct RecordingCanceller(Mutex<Vec<(i32, i32)>>);

#[async_trait]
impl DeploymentCanceller for RecordingCanceller {
    async fn cancel(&self, project_id: i32, deployment_id: i32) -> Result<(), String> {
        self.0
            .lock()
            .expect("cancel lock")
            .push((project_id, deployment_id));
        Ok(())
    }
}

struct Harness {
    service: ApprovalService,
    queue: Arc<CapturingQueue>,
    canceller: Arc<RecordingCanceller>,
}

fn harness(db: MockDatabase, queue: CapturingQueue) -> Harness {
    let queue = Arc::new(queue);
    let canceller = Arc::new(RecordingCanceller::default());
    Harness {
        service: ApprovalService::new(
            Arc::new(db.into_connection()),
            queue.clone(),
            canceller.clone(),
        ),
        queue,
        canceller,
    }
}

fn environment_of(id: i32) -> Vec<BTreeMap<&'static str, Value>> {
    vec![BTreeMap::from([("environment_id", Value::Int(Some(id)))])]
}

fn policy(required: bool) -> policies::Model {
    let now = chrono::Utc::now();
    policies::Model {
        environment_id: 7,
        required,
        updated_by: None,
        created_at: now,
        updated_at: now,
    }
}

fn request(status: &str) -> approvals::Model {
    let now = chrono::Utc::now();
    approvals::Model {
        id: 3,
        deployment_id: 42,
        project_id: 5,
        environment_id: 7,
        status: status.to_string(),
        decided_by: None,
        decided_at: None,
        comment: None,
        created_at: now,
        updated_at: now,
    }
}

fn exec(rows: u64) -> MockExecResult {
    MockExecResult {
        last_insert_id: 0,
        rows_affected: rows,
    }
}

#[tokio::test]
async fn the_gate_allows_an_environment_without_a_policy() {
    let db = MockDatabase::new(DatabaseBackend::Postgres)
        .append_query_results([environment_of(7)])
        .append_query_results([Vec::<policies::Model>::new()]);
    let h = harness(db, CapturingQueue::default());
    assert_eq!(h.service.gate(5, 42).await.unwrap(), GateDecision::Allow);
}

#[tokio::test]
async fn the_gate_opens_a_pending_request_and_blocks() {
    let db = MockDatabase::new(DatabaseBackend::Postgres)
        .append_query_results([environment_of(7)])
        .append_query_results([vec![policy(true)]])
        .append_query_results([Vec::<approvals::Model>::new()])
        .append_exec_results([exec(1)]);
    let h = harness(db, CapturingQueue::default());
    assert_eq!(
        h.service.gate(5, 42).await.unwrap(),
        GateDecision::Block {
            reason: "Waiting for approval".to_string()
        }
    );
}

#[tokio::test]
async fn the_gate_follows_a_decided_request() {
    for (status, expected) in [
        (STATUS_APPROVED, GateDecision::Allow),
        (
            STATUS_REJECTED,
            GateDecision::Block {
                reason: "The deployment was rejected".to_string(),
            },
        ),
        (
            STATUS_PENDING,
            GateDecision::Block {
                reason: "Waiting for approval".to_string(),
            },
        ),
    ] {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([environment_of(7)])
            .append_query_results([vec![policy(true)]])
            .append_query_results([vec![request(status)]]);
        let h = harness(db, CapturingQueue::default());
        assert_eq!(h.service.gate(5, 42).await.unwrap(), expected, "{status}");
    }
}

#[tokio::test]
async fn the_gate_fails_closed_for_an_unknown_deployment() {
    let db = MockDatabase::new(DatabaseBackend::Postgres)
        .append_query_results([Vec::<BTreeMap<&'static str, Value>>::new()]);
    let h = harness(db, CapturingQueue::default());
    assert!(matches!(
        h.service.gate(5, 42).await,
        Err(ApprovalError::DeploymentNotFound {
            project_id: 5,
            deployment_id: 42
        })
    ));
}

#[tokio::test]
async fn approving_queues_the_deployment_to_run() {
    let db = MockDatabase::new(DatabaseBackend::Postgres)
        .append_query_results([vec![request(STATUS_PENDING)]])
        .append_exec_results([exec(1)])
        .append_query_results([vec![request(STATUS_APPROVED)]]);
    let h = harness(db, CapturingQueue::default());
    let decided = h
        .service
        .decide(
            5,
            42,
            ApprovalDecision::Approve,
            Some("  looks good ".into()),
            Some(1),
        )
        .await
        .unwrap();
    assert_eq!(decided.status, STATUS_APPROVED);
    let jobs = h.queue.jobs.lock().unwrap();
    assert!(matches!(
        jobs.as_slice(),
        [Job::DeploymentGateRecheck(DeploymentGateRecheckJob {
            deployment_id: 42
        })]
    ));
    assert!(h.canceller.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_queue_leaves_the_request_pending() {
    let db = MockDatabase::new(DatabaseBackend::Postgres)
        .append_query_results([vec![request(STATUS_PENDING)]])
        .append_exec_results([exec(1)]);
    let h = harness(
        db,
        CapturingQueue {
            fail: true,
            ..Default::default()
        },
    );
    let result = h
        .service
        .decide(5, 42, ApprovalDecision::Approve, None, Some(1))
        .await;
    assert!(
        matches!(
            result,
            Err(ApprovalError::RecheckNotQueued {
                deployment_id: 42,
                ..
            })
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn rejecting_cancels_the_deployment() {
    let db = MockDatabase::new(DatabaseBackend::Postgres)
        .append_query_results([vec![request(STATUS_PENDING)]])
        .append_exec_results([exec(1)])
        .append_query_results([vec![request(STATUS_REJECTED)]]);
    let h = harness(db, CapturingQueue::default());
    let decided = h
        .service
        .decide(5, 42, ApprovalDecision::Reject, None, Some(1))
        .await
        .unwrap();
    assert_eq!(decided.status, STATUS_REJECTED);
    assert_eq!(*h.canceller.0.lock().unwrap(), vec![(5, 42)]);
    assert!(h.queue.jobs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_request_is_decided_only_once() {
    // Already decided when read.
    let db = MockDatabase::new(DatabaseBackend::Postgres)
        .append_query_results([vec![request(STATUS_APPROVED)]]);
    let h = harness(db, CapturingQueue::default());
    assert!(matches!(
        h.service
            .decide(5, 42, ApprovalDecision::Reject, None, Some(1))
            .await,
        Err(ApprovalError::AlreadyDecided {
            deployment_id: 42,
            ..
        })
    ));

    // Decided by someone else between the read and the guarded update.
    let db = MockDatabase::new(DatabaseBackend::Postgres)
        .append_query_results([vec![request(STATUS_PENDING)]])
        .append_exec_results([exec(0)]);
    let h = harness(db, CapturingQueue::default());
    assert!(matches!(
        h.service
            .decide(5, 42, ApprovalDecision::Approve, None, Some(1))
            .await,
        Err(ApprovalError::AlreadyDecided { .. })
    ));
    assert!(h.queue.jobs.lock().unwrap().is_empty(), "nothing queued");
}

#[tokio::test]
async fn validation_runs_before_the_database() {
    let h = harness(
        MockDatabase::new(DatabaseBackend::Postgres),
        CapturingQueue::default(),
    );
    let long = "x".repeat(MAX_COMMENT_LEN + 1);
    assert!(matches!(
        h.service
            .decide(5, 42, ApprovalDecision::Approve, Some(long), Some(1))
            .await,
        Err(ApprovalError::CommentTooLong {
            max: MAX_COMMENT_LEN,
            ..
        })
    ));
    assert!(matches!(
        h.service.list(5, Some("maybe"), None, None).await,
        Err(ApprovalError::InvalidStatus { .. })
    ));
}

#[tokio::test]
async fn deciding_without_a_request_names_the_deployment() {
    let db = MockDatabase::new(DatabaseBackend::Postgres)
        .append_query_results([Vec::<approvals::Model>::new()]);
    let h = harness(db, CapturingQueue::default());
    assert!(matches!(
        h.service
            .decide(5, 42, ApprovalDecision::Approve, None, Some(1))
            .await,
        Err(ApprovalError::ApprovalNotFound {
            project_id: 5,
            deployment_id: 42
        })
    ));
}

#[test]
fn pagination_defaults_to_twenty_and_caps_at_a_hundred() {
    assert_eq!(normalize_pagination(None, None), (1, 20));
    assert_eq!(normalize_pagination(Some(0), Some(500)), (1, 100));
}
