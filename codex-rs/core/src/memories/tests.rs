use super::control::clear_memory_root_contents;
use super::storage::rebuild_raw_memories_file_from_memories;
use super::storage::sync_rollout_summaries_from_memories;
use crate::memories::ensure_layout;
use crate::memories::memory_root;
use crate::memories::raw_memories_file;
use crate::memories::rollout_summaries_dir;
use chrono::TimeZone;
use chrono::Utc;
use codex_config::types::DEFAULT_MEMORIES_MAX_RAW_MEMORIES_FOR_CONSOLIDATION;
use codex_protocol::ThreadId;
use codex_state::Stage1Output;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn memory_root_uses_shared_global_path() {
    let codex_home = AbsolutePathBuf::current_dir().expect("cwd").join("codex");
    assert_eq!(memory_root(&codex_home), codex_home.join("memories"));
}

#[test]
fn stage_one_output_schema_requires_rollout_slug_and_keeps_it_nullable() {
    let schema = crate::memories::phase1::output_schema();
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .expect("properties object");
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .expect("required array");

    let mut required_keys = required
        .iter()
        .map(|key| key.as_str().expect("required key string"))
        .collect::<Vec<_>>();
    required_keys.sort_unstable();

    assert!(
        properties.contains_key("rollout_slug"),
        "schema should declare rollout_slug"
    );

    let rollout_slug_type = properties
        .get("rollout_slug")
        .and_then(Value::as_object)
        .and_then(|schema| schema.get("type"))
        .and_then(Value::as_array)
        .expect("rollout_slug type array");
    let mut rollout_slug_types = rollout_slug_type
        .iter()
        .map(|entry| entry.as_str().expect("type entry string"))
        .collect::<Vec<_>>();
    rollout_slug_types.sort_unstable();

    assert_eq!(
        required_keys,
        vec!["raw_memory", "rollout_slug", "rollout_summary"]
    );
    assert_eq!(rollout_slug_types, vec!["null", "string"]);
}

#[tokio::test]
async fn clear_memory_root_contents_preserves_root_directory() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().join("memory");
    let nested_dir = root.join("rollout_summaries");
    tokio::fs::create_dir_all(&nested_dir)
        .await
        .expect("create rollout summaries dir");
    tokio::fs::write(root.join("MEMORY.md"), "stale memory index\n")
        .await
        .expect("write memory index");
    tokio::fs::write(nested_dir.join("rollout.md"), "stale rollout\n")
        .await
        .expect("write rollout summary");

    clear_memory_root_contents(&root)
        .await
        .expect("clear memory root contents");

    assert!(
        tokio::fs::try_exists(&root)
            .await
            .expect("check memory root existence"),
        "memory root should still exist after clearing contents"
    );
    let mut entries = tokio::fs::read_dir(&root)
        .await
        .expect("read memory root after clear");
    assert!(
        entries
            .next_entry()
            .await
            .expect("read next entry")
            .is_none(),
        "memory root should be empty after clearing contents"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn clear_memory_root_contents_rejects_symlinked_root() {
    let dir = tempdir().expect("tempdir");
    let target = dir.path().join("outside");
    tokio::fs::create_dir_all(&target)
        .await
        .expect("create symlink target dir");
    let target_file = target.join("keep.txt");
    tokio::fs::write(&target_file, "keep\n")
        .await
        .expect("write target file");

    let root = dir.path().join("memory");
    std::os::unix::fs::symlink(&target, &root).expect("create memory root symlink");

    let err = clear_memory_root_contents(&root)
        .await
        .expect_err("symlinked memory root should be rejected");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        tokio::fs::try_exists(&target_file)
            .await
            .expect("check target file existence"),
        "rejecting a symlinked memory root should not delete the symlink target"
    );
}

#[tokio::test]
async fn sync_rollout_summaries_and_raw_memories_file_keeps_latest_memories_only() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().join("memory");
    ensure_layout(&root).await.expect("ensure layout");

    let keep_id = ThreadId::default().to_string();
    let drop_id = ThreadId::default().to_string();
    let keep_path = rollout_summaries_dir(&root).join(format!("{keep_id}.md"));
    let drop_path = rollout_summaries_dir(&root).join(format!("{drop_id}.md"));
    tokio::fs::write(&keep_path, "keep")
        .await
        .expect("write keep");
    tokio::fs::write(&drop_path, "drop")
        .await
        .expect("write drop");

    let memories = vec![Stage1Output {
        thread_id: ThreadId::try_from(keep_id.clone()).expect("thread id"),
        source_updated_at: Utc.timestamp_opt(100, 0).single().expect("timestamp"),
        raw_memory: "raw memory".to_string(),
        rollout_summary: "short summary".to_string(),
        rollout_slug: None,
        rollout_path: PathBuf::from("/tmp/rollout-100.jsonl"),
        cwd: PathBuf::from("/tmp/workspace"),
        git_branch: None,
        generated_at: Utc.timestamp_opt(101, 0).single().expect("timestamp"),
    }];

    sync_rollout_summaries_from_memories(
        &root,
        &memories,
        DEFAULT_MEMORIES_MAX_RAW_MEMORIES_FOR_CONSOLIDATION,
    )
    .await
    .expect("sync rollout summaries");
    rebuild_raw_memories_file_from_memories(
        &root,
        &memories,
        DEFAULT_MEMORIES_MAX_RAW_MEMORIES_FOR_CONSOLIDATION,
    )
    .await
    .expect("rebuild raw memories");

    assert!(
        !tokio::fs::try_exists(&keep_path)
            .await
            .expect("check stale keep path"),
        "sync should prune stale filename that used thread id only"
    );
    assert!(
        !tokio::fs::try_exists(&drop_path)
            .await
            .expect("check stale drop path"),
        "sync should prune stale filename for dropped thread"
    );

    let mut dir = tokio::fs::read_dir(rollout_summaries_dir(&root))
        .await
        .expect("open rollout summaries dir");
    let mut files = Vec::new();
    while let Some(entry) = dir.next_entry().await.expect("read dir entry") {
        files.push(entry.file_name().to_string_lossy().to_string());
    }
    files.sort_unstable();
    assert_eq!(files.len(), 1);
    let canonical_rollout_summary_file = &files[0];

    let raw_memories = tokio::fs::read_to_string(raw_memories_file(&root))
        .await
        .expect("read raw memories");
    assert!(raw_memories.contains("raw memory"));
    assert!(raw_memories.contains(&keep_id));
    assert!(raw_memories.contains("cwd: /tmp/workspace"));
    assert!(raw_memories.contains("rollout_path: /tmp/rollout-100.jsonl"));
    assert!(raw_memories.contains(&format!(
        "rollout_summary_file: {canonical_rollout_summary_file}"
    )));
    let thread_header = format!("## Thread `{keep_id}`");
    let thread_pos = raw_memories
        .find(&thread_header)
        .expect("thread header should exist");
    let updated_pos = raw_memories[thread_pos..]
        .find("updated_at: ")
        .map(|offset| thread_pos + offset)
        .expect("updated_at should exist after thread header");
    let cwd_pos = raw_memories[thread_pos..]
        .find("cwd: /tmp/workspace")
        .map(|offset| thread_pos + offset)
        .expect("cwd should exist after thread header");
    let rollout_path_pos = raw_memories[thread_pos..]
        .find("rollout_path: /tmp/rollout-100.jsonl")
        .map(|offset| thread_pos + offset)
        .expect("rollout_path should exist after thread header");
    let file_pos = raw_memories[thread_pos..]
        .find(&format!(
            "rollout_summary_file: {canonical_rollout_summary_file}"
        ))
        .map(|offset| thread_pos + offset)
        .expect("rollout_summary_file should exist after thread header");
    assert!(thread_pos < updated_pos);
    assert!(updated_pos < cwd_pos);
    assert!(cwd_pos < rollout_path_pos);
    assert!(rollout_path_pos < file_pos);
}

#[tokio::test]
async fn sync_rollout_summaries_uses_timestamp_hash_and_sanitized_slug_filename() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().join("memory");
    ensure_layout(&root).await.expect("ensure layout");

    let thread_id = ThreadId::new();
    let stale_unslugged_path = rollout_summaries_dir(&root).join(format!("{thread_id}.md"));
    let stale_old_slug_path =
        rollout_summaries_dir(&root).join(format!("{thread_id}--old-slug.md"));
    tokio::fs::write(&stale_unslugged_path, "stale")
        .await
        .expect("write stale unslugged file");
    tokio::fs::write(&stale_old_slug_path, "stale")
        .await
        .expect("write stale old-slug file");

    let memories = vec![Stage1Output {
        thread_id,
        source_updated_at: Utc.timestamp_opt(200, 0).single().expect("timestamp"),
        raw_memory: "raw memory".to_string(),
        rollout_summary: "short summary".to_string(),
        rollout_slug: Some("Unsafe Slug/With Spaces & Symbols + EXTRA_LONG_12345".to_string()),
        rollout_path: PathBuf::from("/tmp/rollout-200.jsonl"),
        cwd: PathBuf::from("/tmp/workspace"),
        git_branch: Some("feature/memory-branch".to_string()),
        generated_at: Utc.timestamp_opt(201, 0).single().expect("timestamp"),
    }];

    sync_rollout_summaries_from_memories(
        &root,
        &memories,
        DEFAULT_MEMORIES_MAX_RAW_MEMORIES_FOR_CONSOLIDATION,
    )
    .await
    .expect("sync rollout summaries");

    let mut dir = tokio::fs::read_dir(rollout_summaries_dir(&root))
        .await
        .expect("open rollout summaries dir");
    let mut files = Vec::new();
    while let Some(entry) = dir.next_entry().await.expect("read dir entry") {
        files.push(entry.file_name().to_string_lossy().to_string());
    }
    files.sort_unstable();

    assert_eq!(files.len(), 1);
    let file_name = &files[0];
    let stem = file_name
        .strip_suffix(".md")
        .expect("rollout summary file should end with .md");
    let (prefix, slug) = stem
        .rsplit_once('-')
        .expect("rollout summary filename should include slug");
    let (timestamp, short_hash) = prefix
        .rsplit_once('-')
        .expect("rollout summary filename should include short hash");

    assert_eq!(timestamp.len(), 19, "timestamp should be second precision");
    let parsed_timestamp = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H-%M-%S");
    assert!(
        parsed_timestamp.is_ok(),
        "timestamp should use YYYY-MM-DDThh-mm-ss"
    );
    assert_eq!(short_hash.len(), 4, "short hash should be exactly 4 chars");
    assert!(
        short_hash.chars().all(|ch| ch.is_ascii_alphanumeric()),
        "short hash should use only alphanumeric chars"
    );
    assert!(slug.len() <= 60, "slug should be capped at 60 chars");
    assert!(
        slug.chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_'),
        "slug should be file-safe lowercase ascii with underscores"
    );

    let summary = tokio::fs::read_to_string(rollout_summaries_dir(&root).join(file_name))
        .await
        .expect("read rollout summary");
    assert!(summary.contains(&format!("thread_id: {thread_id}")));
    assert!(summary.contains("rollout_path: /tmp/rollout-200.jsonl"));
    assert!(summary.contains("git_branch: feature/memory-branch"));
    assert!(
        !tokio::fs::try_exists(&stale_unslugged_path)
            .await
            .expect("check stale unslugged path"),
        "slugged sync should prune stale unslugged filename for same thread"
    );
    assert!(
        !tokio::fs::try_exists(&stale_old_slug_path)
            .await
            .expect("check stale old slug path"),
        "slugged sync should prune stale slugged filename for same thread"
    );
}

#[tokio::test]
async fn rebuild_raw_memories_file_adds_canonical_rollout_summary_file_header() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().join("memory");
    ensure_layout(&root).await.expect("ensure layout");

    let thread_id =
        ThreadId::try_from("0194f5a6-89ab-7cde-8123-456789abcdef").expect("valid thread id");
    let memories = vec![Stage1Output {
        thread_id,
        source_updated_at: Utc.timestamp_opt(200, 0).single().expect("timestamp"),
        raw_memory: "\
---
description: Added a migration test
keywords: codex-state, migrations
---
### Task 1: migration-test
task: add-migration-test
task_group: codex-state
task_outcome: success
- Added regression coverage for migration uniqueness.

### Task 2: validate-migration
task: validate-migration-ordering
task_group: codex-state
task_outcome: success
- Confirmed no ordering regressions."
            .to_string(),
        rollout_summary: "short summary".to_string(),
        rollout_slug: Some("Unsafe Slug/With Spaces & Symbols + EXTRA_LONG_12345".to_string()),
        rollout_path: PathBuf::from("/tmp/rollout-200.jsonl"),
        cwd: PathBuf::from("/tmp/workspace"),
        git_branch: None,
        generated_at: Utc.timestamp_opt(201, 0).single().expect("timestamp"),
    }];

    sync_rollout_summaries_from_memories(
        &root,
        &memories,
        DEFAULT_MEMORIES_MAX_RAW_MEMORIES_FOR_CONSOLIDATION,
    )
    .await
    .expect("sync rollout summaries");
    rebuild_raw_memories_file_from_memories(
        &root,
        &memories,
        DEFAULT_MEMORIES_MAX_RAW_MEMORIES_FOR_CONSOLIDATION,
    )
    .await
    .expect("rebuild raw memories");

    let mut dir = tokio::fs::read_dir(rollout_summaries_dir(&root))
        .await
        .expect("open rollout summaries dir");
    let mut files = Vec::new();
    while let Some(entry) = dir.next_entry().await.expect("read dir entry") {
        files.push(entry.file_name().to_string_lossy().to_string());
    }
    files.sort_unstable();
    assert_eq!(files.len(), 1);
    let canonical_rollout_summary_file = &files[0];

    let raw_memories = tokio::fs::read_to_string(raw_memories_file(&root))
        .await
        .expect("read raw memories");
    let summary = tokio::fs::read_to_string(
        rollout_summaries_dir(&root).join(canonical_rollout_summary_file),
    )
    .await
    .expect("read rollout summary");
    assert!(summary.contains("rollout_path: /tmp/rollout-200.jsonl"));
    assert!(raw_memories.contains(&format!(
        "rollout_summary_file: {canonical_rollout_summary_file}"
    )));
    assert!(raw_memories.contains("description: Added a migration test"));
    assert!(raw_memories.contains("### Task 1: migration-test"));
    assert!(raw_memories.contains("task: add-migration-test"));
    assert!(raw_memories.contains("task_group: codex-state"));
    assert!(raw_memories.contains("task_outcome: success"));
}

mod phase2 {
    use crate::ThreadManager;
    use crate::agent::AgentControl;
    use crate::config::Config;
    use crate::config::test_config;
    use crate::memories::memory_root;
    use crate::memories::phase2;
    use crate::memories::raw_memories_file;
    use crate::memories::rollout_summaries_dir;
    use crate::session::session::Session;
    use crate::session::tests::make_session_and_context;
    use chrono::Duration as ChronoDuration;
    use chrono::Utc;
    use codex_config::Constrained;
    use codex_features::Feature;
    use codex_login::CodexAuth;
    use codex_protocol::AgentPath;
    use codex_protocol::ThreadId;
    use codex_protocol::permissions::FileSystemSandboxPolicy;
    use codex_protocol::permissions::NetworkSandboxPolicy;
    use codex_protocol::protocol::AskForApproval;
    use codex_protocol::protocol::Op;
    use codex_protocol::protocol::SandboxPolicy;
    use codex_protocol::protocol::SessionSource;
    use codex_state::Phase2JobClaimOutcome;
    use codex_state::Stage1Output;
    use codex_state::ThreadMetadataBuilder;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn stage1_output_with_source_updated_at(source_updated_at: i64) -> Stage1Output {
        Stage1Output {
            thread_id: ThreadId::new(),
            source_updated_at: chrono::DateTime::<Utc>::from_timestamp(source_updated_at, 0)
                .expect("valid source_updated_at timestamp"),
            raw_memory: "raw memory".to_string(),
            rollout_summary: "rollout summary".to_string(),
            rollout_slug: None,
            rollout_path: PathBuf::from("/tmp/rollout-summary.jsonl"),
            cwd: PathBuf::from("/tmp/workspace"),
            git_branch: None,
            generated_at: chrono::DateTime::<Utc>::from_timestamp(source_updated_at + 1, 0)
                .expect("valid generated_at timestamp"),
        }
    }

    struct DispatchHarness {
        _codex_home: TempDir,
        config: Arc<Config>,
        session: Arc<Session>,
        manager: ThreadManager,
        state_db: Arc<codex_state::StateRuntime>,
    }

    impl DispatchHarness {
        async fn new() -> Self {
            let codex_home = tempfile::tempdir().expect("create temp codex home");
            let mut config = test_config().await;
            config.codex_home =
                codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(codex_home.path())
                    .expect("codex home is absolute");
            config.cwd = config.codex_home.clone();
            config.permissions.file_system_sandbox_policy = FileSystemSandboxPolicy::unrestricted();
            config.permissions.network_sandbox_policy = NetworkSandboxPolicy::Enabled;
            let config = Arc::new(config);

            let state_db = codex_state::StateRuntime::init(
                config.codex_home.to_path_buf(),
                config.model_provider_id.clone(),
            )
            .await
            .expect("initialize state db");

            let manager = ThreadManager::with_models_provider_and_home_for_tests(
                CodexAuth::from_api_key("dummy"),
                config.model_provider.clone(),
                config.codex_home.to_path_buf(),
                std::sync::Arc::new(codex_exec_server::EnvironmentManager::default_for_tests()),
            );
            let (mut session, _turn_context) = make_session_and_context().await;
            session.services.state_db = Some(Arc::clone(&state_db));
            session.services.agent_control = manager.agent_control();

            Self {
                _codex_home: codex_home,
                config,
                session: Arc::new(session),
                manager,
                state_db,
            }
        }

        async fn seed_stage1_output(&self, source_updated_at: i64) {
            let thread_id = ThreadId::new();
            let mut metadata_builder = ThreadMetadataBuilder::new(
                thread_id,
                self.config
                    .codex_home
                    .join(format!("rollout-{thread_id}.jsonl"))
                    .to_path_buf(),
                Utc::now(),
                SessionSource::Cli,
            );
            metadata_builder.cwd = self.config.cwd.to_path_buf();
            metadata_builder.model_provider = Some(self.config.model_provider_id.clone());
            let metadata = metadata_builder.build(&self.config.model_provider_id);

            self.state_db
                .upsert_thread(&metadata)
                .await
                .expect("upsert thread metadata");

            let claim = self
                .state_db
                .try_claim_stage1_job(
                    thread_id,
                    self.session.conversation_id,
                    source_updated_at,
                    /*lease_seconds*/ 3_600,
                    /*max_running_jobs*/ 64,
                )
                .await
                .expect("claim stage-1 job");
            let ownership_token = match claim {
                codex_state::Stage1JobClaimOutcome::Claimed { ownership_token } => ownership_token,
                other => panic!("unexpected stage-1 claim outcome: {other:?}"),
            };
            assert!(
                self.state_db
                    .mark_stage1_job_succeeded(
                        thread_id,
                        &ownership_token,
                        source_updated_at,
                        "raw memory",
                        "rollout summary",
                        /*rollout_slug*/ None,
                    )
                    .await
                    .expect("mark stage-1 success"),
                "stage-1 success should enqueue global consolidation"
            );
        }

        async fn shutdown_threads(&self) {
            let report = self
                .manager
                .shutdown_all_threads_bounded(std::time::Duration::from_secs(10))
                .await;
            assert!(report.submit_failed.is_empty());
            assert!(report.timed_out.is_empty());
        }

        fn user_input_ops_count(&self) -> usize {
            self.manager
                .captured_ops()
                .into_iter()
                .filter(|(_, op)| matches!(op, Op::UserInput { .. }))
                .count()
        }
    }

    #[test]
    fn completion_watermark_never_regresses_below_claimed_input_watermark() {
        let stage1_output = stage1_output_with_source_updated_at(/*source_updated_at*/ 123);

        let completion = phase2::get_watermark(/*claimed_watermark*/ 1_000, &[stage1_output]);
        pretty_assertions::assert_eq!(completion, 1_000);
    }

    #[test]
    fn completion_watermark_uses_claimed_watermark_when_there_are_no_memories() {
        let completion = phase2::get_watermark(/*claimed_watermark*/ 777, &[]);
        pretty_assertions::assert_eq!(completion, 777);
    }

    #[test]
    fn completion_watermark_uses_latest_memory_timestamp_when_it_is_newer() {
        let older = stage1_output_with_source_updated_at(/*source_updated_at*/ 123);
        let newer = stage1_output_with_source_updated_at(/*source_updated_at*/ 456);

        let completion = phase2::get_watermark(/*claimed_watermark*/ 200, &[older, newer]);
        pretty_assertions::assert_eq!(completion, 456);
    }

    #[tokio::test]
    async fn dispatch_skips_when_global_job_is_not_dirty() {
        let harness = DispatchHarness::new().await;

        phase2::run(&harness.session, Arc::clone(&harness.config)).await;

        pretty_assertions::assert_eq!(harness.user_input_ops_count(), 0);
        let thread_ids = harness.manager.list_thread_ids().await;
        pretty_assertions::assert_eq!(thread_ids.len(), 0);
    }

    #[tokio::test]
    async fn dispatch_skips_when_global_job_is_already_running() {
        let harness = DispatchHarness::new().await;
        harness
            .state_db
            .enqueue_global_consolidation(/*input_watermark*/ 123)
            .await
            .expect("enqueue global consolidation");
        let claimed = harness
            .state_db
            .try_claim_global_phase2_job(ThreadId::new(), /*lease_seconds*/ 3_600)
            .await
            .expect("claim running global lock");
        assert!(
            matches!(claimed, Phase2JobClaimOutcome::Claimed { .. }),
            "precondition should claim the running lock"
        );

        phase2::run(&harness.session, Arc::clone(&harness.config)).await;

        let running_claim = harness
            .state_db
            .try_claim_global_phase2_job(ThreadId::new(), /*lease_seconds*/ 3_600)
            .await
            .expect("claim while lock is still running");
        pretty_assertions::assert_eq!(running_claim, Phase2JobClaimOutcome::SkippedRunning);
        pretty_assertions::assert_eq!(harness.user_input_ops_count(), 0);
        let thread_ids = harness.manager.list_thread_ids().await;
        pretty_assertions::assert_eq!(thread_ids.len(), 0);
    }

    #[tokio::test]
    async fn dispatch_reclaims_stale_global_lock_and_starts_consolidation() {
        let harness = DispatchHarness::new().await;
        harness.seed_stage1_output(Utc::now().timestamp()).await;

        let stale_claim = harness
            .state_db
            .try_claim_global_phase2_job(ThreadId::new(), /*lease_seconds*/ 0)
            .await
            .expect("claim stale global lock");
        assert!(
            matches!(stale_claim, Phase2JobClaimOutcome::Claimed { .. }),
            "stale lock precondition should be claimed"
        );

        phase2::run(&harness.session, Arc::clone(&harness.config)).await;

        let post_dispatch_claim = harness
            .state_db
            .try_claim_global_phase2_job(ThreadId::new(), /*lease_seconds*/ 3_600)
            .await
            .expect("claim after stale lock dispatch");
        assert!(
            matches!(
                post_dispatch_claim,
                Phase2JobClaimOutcome::SkippedRunning | Phase2JobClaimOutcome::SkippedNotDirty
            ),
            "stale-lock dispatch should either keep the reclaimed job running or finish it before re-claim"
        );

        let user_input_ops = harness.user_input_ops_count();
        pretty_assertions::assert_eq!(user_input_ops, 1);
        let thread_ids = harness.manager.list_thread_ids().await;
        pretty_assertions::assert_eq!(thread_ids.len(), 1);
        let thread_id = thread_ids[0];
        let subagent = harness
            .manager
            .get_thread(thread_id)
            .await
            .expect("get consolidation thread");
        let config_snapshot = subagent.config_snapshot().await;
        pretty_assertions::assert_eq!(config_snapshot.approval_policy, AskForApproval::Never);
        assert!(config_snapshot.ephemeral);
        pretty_assertions::assert_eq!(
            config_snapshot.cwd.as_path(),
            memory_root(&harness.config.codex_home).as_path()
        );
        match &config_snapshot.sandbox_policy {
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                network_access,
                ..
            } => {
                assert!(!*network_access);
                pretty_assertions::assert_eq!(
                    writable_roots.as_slice(),
                    [memory_root(&harness.config.codex_home)],
                    "consolidation subagent should only be able to write the memory root"
                );
            }
            other => panic!("unexpected sandbox policy: {other:?}"),
        }
        pretty_assertions::assert_eq!(
            config_snapshot.session_source.get_agent_path(),
            Some(AgentPath::morpheus())
        );
        assert!(
            harness
                .session
                .services
                .agent_control
                .get_agent_metadata(thread_id)
                .is_none(),
            "memory consolidation should not be registered in the root collab agent registry"
        );
        let turn_context = subagent.codex.session.new_default_turn().await;
        pretty_assertions::assert_eq!(
            turn_context.file_system_sandbox_policy,
            FileSystemSandboxPolicy::from_legacy_sandbox_policy(
                &config_snapshot.sandbox_policy,
                config_snapshot.cwd.as_path(),
            ),
            "consolidation subagent split filesystem policy should match the memory-root legacy policy"
        );
        assert!(
            turn_context
                .file_system_sandbox_policy
                .can_write_path_with_cwd(
                    memory_root(&harness.config.codex_home).as_path(),
                    config_snapshot.cwd.as_path(),
                ),
            "consolidation subagent should be able to write the memory root"
        );
        assert!(
            !turn_context
                .file_system_sandbox_policy
                .can_write_path_with_cwd(
                    harness.config.codex_home.join("config.toml").as_path(),
                    config_snapshot.cwd.as_path(),
                ),
            "consolidation subagent should not inherit codex_home write access"
        );
        pretty_assertions::assert_eq!(
            turn_context.network_sandbox_policy,
            NetworkSandboxPolicy::Restricted,
            "consolidation subagent split network policy should preserve no-network sandboxing"
        );
        assert!(
            !turn_context.features.enabled(Feature::MemoryTool),
            "consolidation subagent should have the memories feature disabled"
        );
        assert!(
            !turn_context.config.memories.generate_memories,
            "consolidation subagent should not generate memories"
        );
        assert!(
            !turn_context.config.memories.use_memories,
            "consolidation subagent should not read memories"
        );
        assert!(
            subagent.rollout_path().is_none(),
            "ephemeral consolidation thread should not materialize a rollout"
        );
        let memory_mode = harness
            .state_db
            .get_thread_memory_mode(thread_id)
            .await
            .expect("read consolidation thread memory mode");
        pretty_assertions::assert_eq!(memory_mode, None);

        harness.shutdown_threads().await;
    }

    #[tokio::test]
    async fn dispatch_with_empty_stage1_outputs_rebuilds_local_artifacts() {
        let harness = DispatchHarness::new().await;
        let root = memory_root(&harness.config.codex_home);
        let summaries_dir = rollout_summaries_dir(&root);
        tokio::fs::create_dir_all(&summaries_dir)
            .await
            .expect("create rollout summaries dir");

        let stale_summary_path = summaries_dir.join(format!("{}.md", ThreadId::new()));
        tokio::fs::write(&stale_summary_path, "stale summary\n")
            .await
            .expect("write stale rollout summary");
        let raw_memories_path = raw_memories_file(&root);
        tokio::fs::write(&raw_memories_path, "stale raw memories\n")
            .await
            .expect("write stale raw memories");
        let memory_index_path = root.join("MEMORY.md");
        tokio::fs::write(&memory_index_path, "stale memory index\n")
            .await
            .expect("write stale memory index");
        let memory_summary_path = root.join("memory_summary.md");
        tokio::fs::write(&memory_summary_path, "stale memory summary\n")
            .await
            .expect("write stale memory summary");
        let stale_skill_file = root.join("skills/demo/SKILL.md");
        tokio::fs::create_dir_all(
            stale_skill_file
                .parent()
                .expect("skills subdirectory parent should exist"),
        )
        .await
        .expect("create stale skills dir");
        tokio::fs::write(&stale_skill_file, "stale skill\n")
            .await
            .expect("write stale skill");

        harness
            .state_db
            .enqueue_global_consolidation(/*input_watermark*/ 999)
            .await
            .expect("enqueue global consolidation");

        phase2::run(&harness.session, Arc::clone(&harness.config)).await;

        assert!(
            !tokio::fs::try_exists(&stale_summary_path)
                .await
                .expect("check stale summary existence"),
            "empty consolidation should prune stale rollout summary files"
        );
        let raw_memories = tokio::fs::read_to_string(&raw_memories_path)
            .await
            .expect("read rebuilt raw memories");
        pretty_assertions::assert_eq!(raw_memories, "# Raw Memories\n\nNo raw memories yet.\n");
        assert!(
            !tokio::fs::try_exists(&memory_index_path)
                .await
                .expect("check memory index existence"),
            "empty consolidation should remove stale MEMORY.md"
        );
        assert!(
            !tokio::fs::try_exists(&memory_summary_path)
                .await
                .expect("check memory summary existence"),
            "empty consolidation should remove stale memory_summary.md"
        );
        assert!(
            !tokio::fs::try_exists(&stale_skill_file)
                .await
                .expect("check stale skill existence"),
            "empty consolidation should remove stale skills artifacts"
        );
        assert!(
            !tokio::fs::try_exists(root.join("skills"))
                .await
                .expect("check skills dir existence"),
            "empty consolidation should remove stale skills directory"
        );
        let next_claim = harness
            .state_db
            .try_claim_global_phase2_job(ThreadId::new(), /*lease_seconds*/ 3_600)
            .await
            .expect("claim global job after empty consolidation success");
        pretty_assertions::assert_eq!(next_claim, Phase2JobClaimOutcome::SkippedNotDirty);
        pretty_assertions::assert_eq!(harness.user_input_ops_count(), 0);
        let thread_ids = harness.manager.list_thread_ids().await;
        pretty_assertions::assert_eq!(thread_ids.len(), 0);

        harness.shutdown_threads().await;
    }

    #[tokio::test]
    async fn dispatch_marks_job_for_retry_when_sandbox_policy_cannot_be_overridden() {
        let harness = DispatchHarness::new().await;
        harness
            .state_db
            .enqueue_global_consolidation(/*input_watermark*/ 99)
            .await
            .expect("enqueue global consolidation");
        let mut constrained_config = harness.config.as_ref().clone();
        constrained_config.permissions.sandbox_policy =
            Constrained::allow_only(SandboxPolicy::DangerFullAccess);

        phase2::run(&harness.session, Arc::new(constrained_config)).await;

        let retry_claim = harness
            .state_db
            .try_claim_global_phase2_job(ThreadId::new(), /*lease_seconds*/ 3_600)
            .await
            .expect("claim global job after sandbox policy failure");
        pretty_assertions::assert_eq!(retry_claim, Phase2JobClaimOutcome::SkippedNotDirty);
        pretty_assertions::assert_eq!(harness.user_input_ops_count(), 0);
        let thread_ids = harness.manager.list_thread_ids().await;
        pretty_assertions::assert_eq!(thread_ids.len(), 0);
    }

    #[tokio::test]
    async fn dispatch_marks_job_for_retry_when_syncing_artifacts_fails() {
        let harness = DispatchHarness::new().await;
        harness.seed_stage1_output(/*source_updated_at*/ 100).await;
        let root = memory_root(&harness.config.codex_home);
        tokio::fs::write(&root, "not a directory")
            .await
            .expect("create file at memory root");

        phase2::run(&harness.session, Arc::clone(&harness.config)).await;

        let retry_claim = harness
            .state_db
            .try_claim_global_phase2_job(ThreadId::new(), /*lease_seconds*/ 3_600)
            .await
            .expect("claim global job after sync failure");
        pretty_assertions::assert_eq!(retry_claim, Phase2JobClaimOutcome::SkippedNotDirty);
        pretty_assertions::assert_eq!(harness.user_input_ops_count(), 0);
        let thread_ids = harness.manager.list_thread_ids().await;
        pretty_assertions::assert_eq!(thread_ids.len(), 0);
    }

    #[tokio::test]
    async fn dispatch_marks_job_for_retry_when_rebuilding_raw_memories_fails() {
        let harness = DispatchHarness::new().await;
        harness.seed_stage1_output(/*source_updated_at*/ 100).await;
        let root = memory_root(&harness.config.codex_home);
        tokio::fs::create_dir_all(raw_memories_file(&root))
            .await
            .expect("create raw_memories.md as a directory");

        phase2::run(&harness.session, Arc::clone(&harness.config)).await;

        let retry_claim = harness
            .state_db
            .try_claim_global_phase2_job(ThreadId::new(), /*lease_seconds*/ 3_600)
            .await
            .expect("claim global job after rebuild failure");
        pretty_assertions::assert_eq!(retry_claim, Phase2JobClaimOutcome::SkippedNotDirty);
        pretty_assertions::assert_eq!(harness.user_input_ops_count(), 0);
        let thread_ids = harness.manager.list_thread_ids().await;
        pretty_assertions::assert_eq!(thread_ids.len(), 0);
    }

    #[tokio::test]
    async fn dispatch_marks_job_for_retry_when_spawn_agent_fails() {
        let codex_home = tempfile::tempdir().expect("create temp codex home");
        let mut config = test_config().await;
        config.codex_home =
            codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(codex_home.path())
                .expect("codex home is absolute");
        config.cwd = config.codex_home.clone();
        let config = Arc::new(config);

        let state_db = codex_state::StateRuntime::init(
            config.codex_home.to_path_buf(),
            config.model_provider_id.clone(),
        )
        .await
        .expect("initialize state db");

        let (mut session, _turn_context) = make_session_and_context().await;
        session.services.state_db = Some(Arc::clone(&state_db));
        session.services.agent_control = AgentControl::default();
        let session = Arc::new(session);

        let thread_id = ThreadId::new();
        let mut metadata_builder = ThreadMetadataBuilder::new(
            thread_id,
            config
                .codex_home
                .join(format!("rollout-{thread_id}.jsonl"))
                .to_path_buf(),
            Utc::now(),
            SessionSource::Cli,
        );
        metadata_builder.cwd = config.cwd.to_path_buf();
        metadata_builder.model_provider = Some(config.model_provider_id.clone());
        let metadata = metadata_builder.build(&config.model_provider_id);
        state_db
            .upsert_thread(&metadata)
            .await
            .expect("upsert thread metadata");

        let claim = state_db
            .try_claim_stage1_job(
                thread_id,
                session.conversation_id,
                /*source_updated_at*/ 100,
                /*lease_seconds*/ 3_600,
                /*max_running_jobs*/ 64,
            )
            .await
            .expect("claim stage-1 job");
        let ownership_token = match claim {
            codex_state::Stage1JobClaimOutcome::Claimed { ownership_token } => ownership_token,
            other => panic!("unexpected stage-1 claim outcome: {other:?}"),
        };
        assert!(
            state_db
                .mark_stage1_job_succeeded(
                    thread_id,
                    &ownership_token,
                    /*source_updated_at*/ 100,
                    "raw memory",
                    "rollout summary",
                    /*rollout_slug*/ None,
                )
                .await
                .expect("mark stage-1 success"),
            "stage-1 success should enqueue global consolidation"
        );

        let chronicle_resources = config
            .codex_home
            .join("memories_extensions/chronicle/resources");
        tokio::fs::create_dir_all(&chronicle_resources)
            .await
            .expect("create chronicle resources");
        tokio::fs::write(
            config
                .codex_home
                .join("memories_extensions/chronicle/instructions.md"),
            "instructions",
        )
        .await
        .expect("write chronicle instructions");
        let old_file = chronicle_resources.join(format!(
            "{}-abcd-10min-old.md",
            (Utc::now() - ChronoDuration::days(8)).format("%Y-%m-%dT%H-%M-%S")
        ));
        tokio::fs::write(&old_file, "old resource")
            .await
            .expect("write old extension resource");

        phase2::run(&session, Arc::clone(&config)).await;

        let retry_claim = state_db
            .try_claim_global_phase2_job(ThreadId::new(), /*lease_seconds*/ 3_600)
            .await
            .expect("claim global job after spawn failure");
        pretty_assertions::assert_eq!(
            retry_claim,
            Phase2JobClaimOutcome::SkippedNotDirty,
            "spawn failures should leave the job in retry backoff instead of running"
        );
        assert!(
            tokio::fs::try_exists(&old_file)
                .await
                .expect("check old extension resource"),
            "spawn failures should not prune extension resources before retry"
        );
    }
}
// Integration tests for the consolidated memory subsystem.
// Appended to memories/tests.rs to access pub(crate) symbols.

mod claudemd_notepad_integration {
    use super::super::claudemd;
    use super::super::notepad;
    use super::super::notepad::NotepadSection;
    use super::super::scan_memory_topics;
    use super::super::write_topic;
    use super::super::relevance_score;
    use super::super::MemoryFrontmatter;
    use super::super::memory_root;
    use chrono::Utc;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use std::path::PathBuf;
    use tempfile::TempDir;

    struct TestEnv {
        codex_home: TempDir,
        project_root: TempDir,
    }

    impl TestEnv {
        fn new() -> Self {
            Self {
                codex_home: TempDir::new().unwrap(),
                project_root: TempDir::new().unwrap(),
            }
        }

        fn codex_home_path(&self) -> &std::path::Path {
            self.codex_home.path()
        }

        fn project_root_path(&self) -> &std::path::Path {
            self.project_root.path()
        }

        fn memories_root(&self) -> PathBuf {
            self.codex_home.path().join("memories")
        }

        fn codex_home_abs(&self) -> AbsolutePathBuf {
            AbsolutePathBuf::from_absolute_path(self.codex_home.path()).unwrap()
        }
    }

    // --- Claudemd tests ---

    #[test]
    fn claudemd_loads_all_four_scopes() {
        let env = TestEnv::new();
        let ch = env.codex_home_path();
        let pr = env.project_root_path();

        std::fs::create_dir_all(ch.join("rules")).unwrap();
        std::fs::write(ch.join("rules/01_policy.md"), "Managed: policy A").unwrap();
        std::fs::write(ch.join("AGENTS.md"), "User: global instructions").unwrap();
        std::fs::write(pr.join("AGENTS.md"), "Project: project instructions").unwrap();
        std::fs::create_dir_all(pr.join(".codex")).unwrap();
        std::fs::write(pr.join(".codex/AGENTS.md"), "Local: local override").unwrap();

        let files = claudemd::load_all_memory_files(pr, ch);
        pretty_assertions::assert_eq!(files.len(), 4);

        pretty_assertions::assert_eq!(files[0].scope, claudemd::MemoryScope::Managed);
        assert!(files[0].content.contains("Managed: policy A"));
        pretty_assertions::assert_eq!(files[1].scope, claudemd::MemoryScope::User);
        assert!(files[1].content.contains("User: global instructions"));
        pretty_assertions::assert_eq!(files[2].scope, claudemd::MemoryScope::Project);
        assert!(files[2].content.contains("Project: project instructions"));
        pretty_assertions::assert_eq!(files[3].scope, claudemd::MemoryScope::Local);
        assert!(files[3].content.contains("Local: local override"));

        // build_memory_prompt concatenation
        let prompt = claudemd::build_memory_prompt(&files);
        assert!(prompt.contains("Managed: policy A"));
        assert!(prompt.contains("User: global instructions"));
        assert!(prompt.contains("Project: project instructions"));
        assert!(prompt.contains("Local: local override"));
    }

    #[test]
    fn claudemd_agents_md_preferred_over_claude_md() {
        let env = TestEnv::new();
        let ch = env.codex_home_path();
        let pr = env.project_root_path();

        // Both files exist at project level
        std::fs::write(pr.join("AGENTS.md"), "From AGENTS.md").unwrap();
        std::fs::write(pr.join("CLAUDE.md"), "From CLAUDE.md").unwrap();

        let files = claudemd::load_all_memory_files(pr, ch);
        // Should load both: AGENTS.md first, then CLAUDE.md
        assert!(files.len() >= 2);
        assert!(files[0].content.contains("From AGENTS.md"));
        assert!(files[1].content.contains("From CLAUDE.md"));
    }

    #[test]
    fn claudemd_include_expansion() {
        let env = TestEnv::new();
        let pr = env.project_root_path();

        std::fs::write(pr.join("extra.md"), "Included content here").unwrap();
        std::fs::write(
            pr.join("AGENTS.md"),
            "Main content\n@include extra.md\nAfter include",
        )
        .unwrap();

        let files = claudemd::load_all_memory_files(pr, env.codex_home_path());
        pretty_assertions::assert_eq!(files.len(), 1);
        assert!(files[0].content.contains("Main content"));
        assert!(files[0].content.contains("Included content here"));
        assert!(files[0].content.contains("After include"));
    }

    #[test]
    fn claudemd_circular_include_skipped() {
        let env = TestEnv::new();
        let pr = env.project_root_path();

        std::fs::write(pr.join("a.md"), "@include b.md\nContent A").unwrap();
        std::fs::write(pr.join("b.md"), "@include a.md\nContent B").unwrap();
        std::fs::write(pr.join("AGENTS.md"), "@include a.md").unwrap();

        let files = claudemd::load_all_memory_files(pr, env.codex_home_path());
        let content = &files[0].content;
        assert!(content.contains("Content A"));
        // b.md's @include a.md is circular → skipped
        assert!(content.contains("Content B") || content.contains("circular @include"));
    }

    #[test]
    fn claudemd_frontmatter_parsing() {
        let input = "---\nmemory_type: project\npriority: 10\n---\nBody content";
        let (fm, body) = claudemd::parse_frontmatter(input);
        pretty_assertions::assert_eq!(fm.memory_type.as_deref(), Some("project"));
        pretty_assertions::assert_eq!(fm.priority, Some(10));
        assert!(body.starts_with("Body content"));
    }

    // --- Notepad integration tests ---

    #[tokio::test]
    async fn notepad_full_lifecycle() {
        let env = TestEnv::new();
        let root = env.memories_root();

        // Initially empty
        assert!(notepad::read_notepad(&root, None).await.is_none());

        // Write priority
        notepad::write_priority(&root, "Fix login bug first")
            .await
            .unwrap();
        let priority = notepad::read_notepad(&root, Some(NotepadSection::Priority))
            .await
            .unwrap();
        pretty_assertions::assert_eq!(priority, "Fix login bug first");

        // Append working entries
        notepad::append_working(&root, "Started refactoring auth module")
            .await
            .unwrap();
        notepad::append_working(&root, "Found circular dependency in UserService")
            .await
            .unwrap();
        let working = notepad::read_notepad(&root, Some(NotepadSection::Working))
            .await
            .unwrap();
        assert!(working.contains("Started refactoring auth module"));
        assert!(working.contains("Found circular dependency in UserService"));

        // Append manual
        notepad::append_manual(&root, "Always use bcrypt for passwords")
            .await
            .unwrap();
        let manual = notepad::read_notepad(&root, Some(NotepadSection::Manual))
            .await
            .unwrap();
        assert!(manual.contains("Always use bcrypt for passwords"));

        // Read all sections
        let all = notepad::read_notepad(&root, None).await.unwrap();
        assert!(all.contains("## PRIORITY"));
        assert!(all.contains("## WORKING MEMORY"));
        assert!(all.contains("## MANUAL"));

        // Prune old entries (max_age_days=0 → all working entries pruned)
        let removed = notepad::prune_working(&root, Some(0)).await.unwrap();
        pretty_assertions::assert_eq!(removed, 2);

        // Working should be empty after pruning
        let working_after = notepad::read_notepad(&root, Some(NotepadSection::Working)).await;
        assert!(working_after.is_none() || working_after.unwrap().trim().is_empty());

        // Priority and manual should survive pruning
        assert!(notepad::read_notepad(&root, Some(NotepadSection::Priority))
            .await
            .is_some());
    }

    #[tokio::test]
    async fn notepad_priority_truncation() {
        let env = TestEnv::new();
        let root = env.memories_root();

        let long_content = "x".repeat(600);
        notepad::write_priority(&root, &long_content).await.unwrap();

        let priority = notepad::read_notepad(&root, Some(NotepadSection::Priority))
            .await
            .unwrap();
        assert!(
            priority.len() <= 500,
            "Priority should be truncated to 500 chars, got {}",
            priority.len()
        );
    }

    #[tokio::test]
    async fn notepad_priority_replaces_not_appends() {
        let env = TestEnv::new();
        let root = env.memories_root();

        notepad::write_priority(&root, "First priority").await.unwrap();
        notepad::write_priority(&root, "Second priority").await.unwrap();

        let priority = notepad::read_notepad(&root, Some(NotepadSection::Priority))
            .await
            .unwrap();
        pretty_assertions::assert_eq!(priority, "Second priority");
    }

    // --- Memdir topics + relevance ---

    #[tokio::test]
    async fn memdir_topic_write_and_scan() {
        let env = TestEnv::new();
        let root = env.memories_root();

        let fm1 = MemoryFrontmatter {
            name: "architecture".to_string(),
            description: "System architecture decisions".to_string(),
            r#type: "project".to_string(),
            keywords: vec!["microservice".to_string(), "api".to_string()],
            source: "agent".to_string(),
            updated_at: Some(Utc::now()),
        };
        write_topic(&root, "architecture", &fm1, "Use microservice pattern for API gateway")
            .await
            .unwrap();

        let fm2 = MemoryFrontmatter {
            name: "testing".to_string(),
            description: "Testing conventions and patterns".to_string(),
            r#type: "project".to_string(),
            keywords: vec!["pytest".to_string(), "unit-test".to_string()],
            source: "agent".to_string(),
            updated_at: Some(Utc::now()),
        };
        write_topic(&root, "testing", &fm2, "Use pytest with fixtures for unit tests")
            .await
            .unwrap();

        let topics = scan_memory_topics(&root).await;
        pretty_assertions::assert_eq!(topics.len(), 2);

        let score_arch = relevance_score(
            topics.iter().find(|t| t.frontmatter.name == "architecture").unwrap(),
            "microservice API design",
        );
        let score_test = relevance_score(
            topics.iter().find(|t| t.frontmatter.name == "testing").unwrap(),
            "microservice API design",
        );
        assert!(
            score_arch > score_test,
            "Architecture should score higher for microservice API query ({} vs {})",
            score_arch, score_test
        );
    }

    // --- Full stack integration ---

    #[tokio::test]
    async fn full_memory_stack_claudemd_memdir_notepad() {
        let env = TestEnv::new();
        let ch = env.codex_home_path();
        let pr = env.project_root_path();
        let root = env.memories_root();

        // Claudemd: user + project scopes
        std::fs::write(ch.join("AGENTS.md"), "User-level: prefer Rust over Python").unwrap();
        std::fs::write(pr.join("AGENTS.md"), "Project-level: use tokio for async").unwrap();

        // Memdir: create a topic
        let fm = MemoryFrontmatter {
            name: "conventions".to_string(),
            description: "Coding conventions".to_string(),
            r#type: "project".to_string(),
            keywords: vec!["rust".to_string(), "tokio".to_string()],
            source: "agent".to_string(),
            updated_at: Some(Utc::now()),
        };
        write_topic(&root, "conventions", &fm, "Always use tokio::spawn for async tasks")
            .await
            .unwrap();

        // Notepad: priority + working
        notepad::write_priority(&root, "Ship v2 by Friday")
            .await
            .unwrap();
        notepad::append_working(&root, "Completed auth module refactor")
            .await
            .unwrap();

        // Verify claudemd loads both scopes
        let claudemd_files = claudemd::load_all_memory_files(pr, ch);
        pretty_assertions::assert_eq!(claudemd_files.len(), 2);
        let prompt = claudemd::build_memory_prompt(&claudemd_files);
        assert!(prompt.contains("prefer Rust over Python"));
        assert!(prompt.contains("use tokio for async"));

        // Verify memdir topics
        let topics = scan_memory_topics(&root).await;
        pretty_assertions::assert_eq!(topics.len(), 1);
        pretty_assertions::assert_eq!(topics[0].frontmatter.name, "conventions");

        // Verify notepad
        let priority = notepad::read_notepad(&root, Some(NotepadSection::Priority))
            .await
            .unwrap();
        assert!(priority.contains("Ship v2 by Friday"));
        let working = notepad::read_notepad(&root, Some(NotepadSection::Working))
            .await
            .unwrap();
        assert!(working.contains("Completed auth module refactor"));

        // Verify notepad prune doesn't affect topics
        let removed = notepad::prune_working(&root, Some(0)).await.unwrap();
        assert!(removed > 0);
        let topics_after = scan_memory_topics(&root).await;
        pretty_assertions::assert_eq!(topics_after.len(), 1);
    }

    #[tokio::test]
    async fn memory_prompt_injects_notepad_priority() {
        let env = TestEnv::new();
        let codex_home = env.codex_home_abs();

        // Set up notepad priority
        let root = memory_root(&codex_home);
        tokio::fs::create_dir_all(&root).await.unwrap();
        notepad::write_priority(&root, "Critical: deploy hotfix before EOD")
            .await
            .unwrap();

        // Build memory developer instructions (same path as session/mod.rs)
        let result = crate::memories::prompts::build_memory_tool_developer_instructions(
            &codex_home,
            "",
        )
        .await;

        assert!(result.is_some(), "Should return Some when notepad priority exists");
        let instructions = result.unwrap();
        assert!(
            instructions.contains("Critical: deploy hotfix before EOD"),
            "Developer instructions should contain notepad priority content"
        );
    }
}
