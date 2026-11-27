use super::*;

fn input(name: &str, steps: Vec<(&str, &str)>) -> RunbookInput {
    RunbookInput {
        name: name.into(),
        description: String::new(),
        tags: vec![" Web ".into(), "web".into(), "".into()],
        inputs: vec![],
        steps: steps.into_iter().map(|(n, r)| Step { name: n.into(), run: r.into(), ..Default::default() }).collect(),
        on_step_failure: FailurePolicy::Stop,
        output_limit: None,
        rollout: Rollout::default(),
        schedules: vec![],
        notifications: vec![],
        secrets: vec![],
        targets: Targets::default(),
        timeout_seconds: None,
    }
}

fn def(name: &str, kind: InputKind) -> InputDef {
    InputDef { name: name.into(), kind, ..Default::default() }
}

#[test]
fn inputs_are_validated() {
    let mut rb = input("x", vec![("a", "echo {{ inputs.service }}")]);
    assert!(rb.clone().validate().is_err(), "reference to a missing input");
    rb.inputs = vec![def("service", InputKind::Text)];
    assert!(rb.clone().validate().is_ok());
    for bad in ["Service", "1x", "a-b", "", &"x".repeat(41)] {
        let mut r = rb.clone();
        r.inputs = vec![def(bad, InputKind::Text)];
        assert!(r.validate().is_err(), "{bad}");
    }
    let mut dup = rb.clone();
    dup.inputs = vec![def("service", InputKind::Text), def("service", InputKind::Number)];
    assert!(dup.validate().is_err());
    let mut choice = rb.clone();
    choice.inputs = vec![def("service", InputKind::Choice)];
    assert!(choice.clone().validate().is_err(), "choice without choices");
    choice.inputs[0].choices = vec!["nginx".into(), " nginx ".into(), "redis".into()];
    assert_eq!(choice.validate().unwrap().inputs[0].choices, vec!["nginx", "redis"]);
    let mut secret_default = rb.clone();
    secret_default.inputs = vec![InputDef { default: Some("hunter22".into()), ..def("service", InputKind::Secret) }];
    assert!(secret_default.validate().is_err());
    let mut bad_default = rb.clone();
    bad_default.inputs = vec![InputDef { default: Some("ten".into()), ..def("service", InputKind::Number) }];
    assert!(bad_default.validate().is_err());
    let mut bad_pattern = rb.clone();
    bad_pattern.inputs = vec![InputDef { pattern: Some("(".into()), ..def("service", InputKind::Text) }];
    assert!(bad_pattern.validate().is_err());
}

#[test]
fn step_options_are_checked() {
    let mut rb = input("x", vec![("a", "true")]);
    rb.steps[0].retries = MAX_RETRIES + 1;
    assert!(rb.clone().validate().is_err());
    rb.steps[0].retries = 2;
    rb.steps[0].retry_delay_seconds = MAX_RETRY_DELAY + 1;
    assert!(rb.clone().validate().is_err());
    rb.steps[0].retry_delay_seconds = 5;
    rb.steps[0].timeout_seconds = Some(0);
    assert!(rb.clone().validate().is_err());
    rb.steps[0].timeout_seconds = Some(30);
    rb.output_limit = Some(OutputLimit { lines: 5, action: LimitAction::Fail });
    assert!(rb.clone().validate().is_err());
    rb.output_limit = Some(OutputLimit { lines: 100, action: LimitAction::Fail });
    assert!(rb.validate().is_ok());
    let parsed: RunbookInput = serde_json::from_value(serde_json::json!({
        "name": "x", "timeout_seconds": "2h",
        "steps": [{"name": "a", "run": "true", "timeout_seconds": "90s", "retry_delay_seconds": "1m"}]
    }))
    .unwrap();
    assert_eq!(parsed.timeout_seconds, Some(7200));
    assert_eq!(parsed.steps[0].timeout_seconds, Some(90));
    assert_eq!(parsed.steps[0].retry_delay_seconds, 60);
}

#[test]
fn rollouts_plan_their_waves() {
    let plan = |r: Rollout, n: usize| r.waves(n).into_iter().map(|w| (w.label, w.hosts, w.concurrency)).collect::<Vec<_>>();
    let seq = Rollout::default();
    assert_eq!(plan(seq, 3), vec![(String::new(), 0..3, 1)]);
    assert_eq!(plan(Rollout { canary: 1, ..seq }, 3), vec![("Canary".into(), 0..1, 1), (String::new(), 1..3, 1)]);
    let par = Rollout { mode: RolloutMode::Parallel, ..seq };
    assert_eq!(plan(par, 4), vec![(String::new(), 0..4, 4)]);
    assert_eq!(plan(Rollout { max_parallel: 2, ..par }, 5), vec![(String::new(), 0..5, 2)]);
    let batches = Rollout { mode: RolloutMode::Batches, batch_size: Some(BatchSize::Percent(40)), canary: 1, ..seq };
    // 40% of 6 hosts is 3 (rounded up), after one canary host.
    assert_eq!(plan(batches, 6), vec![("Canary".into(), 0..1, 1), ("Batch 1 of 2".into(), 1..4, 3), ("Batch 2 of 2".into(), 4..6, 2)]);
    assert_eq!(plan(Rollout { canary: 9, ..batches }, 2), vec![(String::new(), 0..2, 2)], "a canary larger than the run is the whole run");
    assert!(plan(par, 0).is_empty());
    assert!(!seq.stops_on_host_failure(FailurePolicy::FinishThenFail));
    assert!(seq.stops_on_host_failure(FailurePolicy::Stop));
    assert!(!Rollout { on_host_failure: Some(HostFailure::Continue), ..seq }.stops_on_host_failure(FailurePolicy::Stop));
}

#[test]
fn rollouts_are_checked() {
    let parse = |v: serde_json::Value| serde_json::from_value::<Rollout>(v).map_err(|e| e.to_string()).and_then(|r| r.validate().map(|_| r));
    assert_eq!(parse(serde_json::json!({"mode": "batches", "batch_size": "25%", "pause_seconds": "1m"})).unwrap().batch_size, Some(BatchSize::Percent(25)));
    assert_eq!(parse(serde_json::json!({"mode": "batches", "batch_size": 3})).unwrap().batch_size, Some(BatchSize::Hosts(3)));
    assert_eq!(serde_json::to_value(BatchSize::Percent(25)).unwrap(), serde_json::json!("25%"));
    for bad in [
        serde_json::json!({"mode": "batches"}),
        serde_json::json!({"mode": "batches", "batch_size": 0}),
        serde_json::json!({"mode": "batches", "batch_size": "120%"}),
        serde_json::json!({"mode": "batches", "batch_size": "lots"}),
        serde_json::json!({"mode": "parallel", "batch_size": 2}),
        serde_json::json!({"mode": "sequential", "max_parallel": 2}),
        serde_json::json!({"mode": "parallel", "pause_seconds": "2h"}),
        serde_json::json!({"mode": "everywhere"}),
    ] {
        assert!(parse(bad.clone()).is_err(), "{bad}");
    }
}

#[test]
fn input_values_are_resolved() {
    let defs = vec![
        InputDef { required: true, pattern: Some("[a-z]+".into()), ..def("service", InputKind::Text) },
        InputDef { default: Some("3".into()), ..def("count", InputKind::Number) },
        def("dry_run", InputKind::Boolean),
        InputDef { choices: vec!["eu".into(), "us".into()], default: Some("eu".into()), ..def("region", InputKind::Choice) },
        def("token", InputKind::Secret),
    ];
    let given = |v: serde_json::Value| v.as_object().unwrap().clone();
    let ok = resolve_inputs(&defs, &given(serde_json::json!({"service": "nginx", "dry_run": true, "token": "abcd"}))).unwrap();
    let get = |n: &str| ok.iter().find(|i| i.name == n).unwrap().value.clone();
    assert_eq!((get("service"), get("count"), get("dry_run"), get("region"), get("token")), ("nginx".into(), "3".into(), "true".into(), "eu".into(), "abcd".into()));
    assert!(ok.iter().find(|i| i.name == "token").unwrap().secret);
    assert_eq!(ok[0].env_name(), "OPSKILLS_INPUT_SERVICE");
    for bad in [
        serde_json::json!({}),
        serde_json::json!({"service": ""}),
        serde_json::json!({"service": "Nginx"}),
        serde_json::json!({"service": "nginx", "count": "many"}),
        serde_json::json!({"service": "nginx", "dry_run": "maybe"}),
        serde_json::json!({"service": "nginx", "region": "asia"}),
        serde_json::json!({"service": "nginx", "nope": 1}),
        serde_json::json!({"service": ["a"]}),
    ] {
        assert!(resolve_inputs(&defs, &given(bad.clone())).is_err(), "{bad}");
    }
}

#[test]
fn validation_normalizes_and_rejects() {
    let ok = input("  Check disk  ", vec![("", "df -h")]).validate().unwrap();
    assert_eq!(ok.name, "Check disk");
    assert_eq!(ok.tags, vec!["web"]);
    assert_eq!(ok.steps[0].name, "Step 1");
    assert!(input("", vec![("a", "true")]).validate().is_err());
    assert!(input("x", vec![]).validate().is_err());
    assert!(input("x", vec![("a", "   ")]).validate().is_err());
    let mut bad_tag = input("x", vec![("a", "true")]);
    bad_tag.tags = vec!["no spaces".into()];
    assert!(bad_tag.validate().is_err());
    let mut bad_secret = input("x", vec![("a", "true")]);
    bad_secret.secrets = vec!["db_password".into()];
    assert!(bad_secret.validate().is_err());
    for name in ["PATH", "OPSKILLS_X", "1ABC", ""] {
        assert!(validate_secret_name(name).is_err(), "{name}");
    }
    assert!(validate_secret_name("DB_PASSWORD").is_ok());
    assert!(validate_secret_value("abc").is_err());
    let mut bad_timeout = input("x", vec![("a", "true")]);
    bad_timeout.timeout_seconds = Some(0);
    assert!(bad_timeout.validate().is_err());
}
