    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Barrier;

    struct FakeTool {
        spec: ToolSpec,
        permissioned: bool,
        overlap: Option<(Arc<Barrier>, Arc<AtomicUsize>, Arc<AtomicUsize>)>,
        delay: Duration,
        output: String,
    }
    impl tools::Tool for FakeTool {
        fn spec(&self) -> ToolSpec {
            self.spec.clone()
        }

        fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
            self.permissioned.then(|| PermissionRequest {
                id: PermissionRequestId(format!("permission-{}", invocation.id.0)),
                task_id: invocation.task_id.clone(),
                risk: PermissionRisk::Read,
                action: invocation.tool_name.clone(),
                reason: "test permission".to_string(),
                scope: "test".to_string(),
                metadata: Metadata::new(),
            })
        }

        fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, tools::ToolError> {
            if let Some((barrier, active, peak)) = &self.overlap {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                barrier.wait();
                std::thread::sleep(self.delay);
                active.fetch_sub(1, Ordering::SeqCst);
            }
            Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Succeeded,
                self.output.clone(),
                Metadata::new(),
            ))
        }
    }

    fn independent_read_spec() -> ToolSpec {
        ToolSpec::builtin(
            "test.read",
            "test",
            "test read",
            ToolRisk::ReadOnly,
            r#"{"type":"object","properties":{}}"#,
        )
        .with_execution_concurrency(agent_core::ToolExecutionConcurrency::IndependentRead)
    }

    fn invocation(id: &str, tool_name: &str) -> ToolInvocation {
        ToolInvocation {
            id: agent_core::ToolCallId(id.to_string()),
            task_id: TaskId("task-test".to_string()),
            tool_name: tool_name.to_string(),
            input_json: "{}".to_string(),
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        }
    }

    fn fake_tool(
        name: &str,
        independent_read: bool,
        permissioned: bool,
        overlap: Option<(Arc<Barrier>, Arc<AtomicUsize>, Arc<AtomicUsize>)>,
        delay: Duration,
        output: &str,
    ) -> Box<dyn tools::Tool> {
        let spec = ToolSpec::builtin(
            name,
            "test",
            "fake test tool",
            ToolRisk::ReadOnly,
            r#"{"type":"object","properties":{}}"#,
        );
        let spec = if independent_read {
            spec.with_execution_concurrency(
                agent_core::ToolExecutionConcurrency::IndependentRead,
            )
        } else {
            spec
        };
        Box::new(FakeTool {
            spec,
            permissioned,
            overlap,
            delay,
            output: output.to_string(),
        })
    }

    fn prepared_call(id: &str, tool_name: &str) -> PreparedParallelToolCall {
        PreparedParallelToolCall {
            call: AgentToolRequest {
                call_id: agent_core::ToolCallId(id.to_string()),
                tool_name: tool_name.to_string(),
                input: "{}".to_string(),
            },
            invocation: invocation(id, tool_name),
            risk: ToolRisk::ReadOnly,
            input_fingerprint: format!("fingerprint-{id}"),
        }
    }

    #[test]
    fn parallel_tool_path_requires_explicit_builtin_read_contract() {
        assert!(tool_spec_allows_independent_read(&independent_read_spec()));
        assert!(!tool_spec_allows_independent_read(
            &independent_read_spec()
                .with_effect_semantics(agent_core::ToolEffectSemantics::Idempotent)
        ));

        let mut mcp = independent_read_spec();
        mcp.source = agent_core::ToolSource::Mcp {
            server_id: "test".to_string(),
        };
        assert!(!tool_spec_allows_independent_read(&mcp));
    }

    #[test]
    fn parallel_tool_bodies_overlap_but_return_in_request_order() {
        let _executor_guard = crate::parallel_execution::TOOL_EXECUTOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let barrier = Arc::new(Barrier::new(2));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let overlap = || {
            Some((
                Arc::clone(&barrier),
                Arc::clone(&active),
                Arc::clone(&peak),
            ))
        };
        let mut registry = ToolRegistry::new();
        registry.register(fake_tool(
            "test.first",
            true,
            false,
            overlap(),
            Duration::from_millis(30),
            "first",
        ));
        registry.register(fake_tool(
            "test.second",
            true,
            false,
            overlap(),
            Duration::ZERO,
            "second",
        ));
        let prepared = vec![
            prepared_call("call-first", "test.first"),
            prepared_call("call-second", "test.second"),
        ];
        let cancellation = Arc::new(AgentRunControl::new("fast"));
        let lease = match cancellation.execution_epoch_lease() {
            agent_runtime::RunEpochLeaseOutcome::Acquired(lease) => lease,
            outcome => panic!("expected current execution lease, got {outcome:?}"),
        };

        let results = run_parallel_tool_bodies(&registry, &cancellation, lease, &prepared);

        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(
            results
                .into_iter()
                .map(|result| result.result.output)
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
    }

    #[test]
    fn batch_eligibility_rejects_serial_permissioned_and_unknown_members() {
        let mut registry = ToolRegistry::new();
        registry.register(fake_tool(
            "test.read",
            true,
            false,
            None,
            Duration::ZERO,
            "read",
        ));
        registry.register(fake_tool(
            "test.read-two",
            true,
            false,
            None,
            Duration::ZERO,
            "read two",
        ));
        registry.register(fake_tool(
            "test.serial",
            false,
            false,
            None,
            Duration::ZERO,
            "serial",
        ));
        registry.register(fake_tool(
            "test.permissioned",
            true,
            true,
            None,
            Duration::ZERO,
            "permissioned",
        ));

        let eligible = [
            invocation("call-read", "test.read"),
            invocation("call-read-two", "test.read-two"),
        ];
        assert!(parallel_tool_batch_contracts(&registry, eligible.len(), eligible.iter()).is_some());

        for rejected_tool in ["test.serial", "test.permissioned", "test.unknown"] {
            let mixed = [
                invocation("call-read", "test.read"),
                invocation("call-rejected", rejected_tool),
            ];
            assert!(
                parallel_tool_batch_contracts(&registry, mixed.len(), mixed.iter()).is_none(),
                "{rejected_tool} must force the exact serial path"
            );
        }
    }

    #[test]
    fn proposed_and_started_events_preserve_model_call_order() {
        let mut store = SqliteStore::in_memory().expect("test store should open");
        let prepared = vec![
            prepared_call("call-first", "test.first"),
            prepared_call("call-second", "test.second"),
        ];

        append_parallel_tool_started_events(&mut store, &prepared, &Metadata::new())
            .expect("ordered tool events should append");

        let events = store
            .list_by_task(&TaskId("task-test".to_string()))
            .expect("tool events should load");
        let order = events
            .iter()
            .map(|event| {
                let kind = match event.kind {
                    EventKind::ToolCallProposed => "proposed",
                    EventKind::ToolCallStarted => "started",
                    ref other => panic!("unexpected event kind: {other:?}"),
                };
                (
                    kind,
                    event
                        .metadata
                        .get("tool_call_id")
                        .cloned()
                        .expect("tool event should carry call id"),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            vec![
                ("proposed", "call-first".to_string()),
                ("started", "call-first".to_string()),
                ("proposed", "call-second".to_string()),
                ("started", "call-second".to_string()),
            ]
        );
    }
