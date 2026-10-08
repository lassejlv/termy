use super::*;

fn record(body: &str) -> ProgramStatusRecord {
    match parse(body).expect("valid report") {
        Report::Set(record) => record,
        Report::Clear(_) => panic!("expected record"),
    }
}

#[test]
fn reports_replace_records_and_resolve_apps_dynamically() {
    let mut status = ProgramStatus::default();
    status.report("state=working:app=deploy:msg=SGk=");
    status.report("state=blocked:id=eu/test:kind=permission:progress=40");
    assert_eq!(status.snapshot()[1].app.as_deref(), Some("deploy"));
    status.report("state=idle:id=eu:app=worker");
    assert_eq!(status.snapshot()[1].app.as_deref(), Some("worker"));
    status.report("state=done:id=eu");
    assert_eq!(status.snapshot()[1].app.as_deref(), Some("deploy"));
    status.report("state=idle");
    let root = status.snapshot().pop().unwrap();
    assert_eq!(root.app, None);
    assert_eq!(root.msg, None);
    assert_eq!(status.snapshot()[0].app, None);
}

#[test]
fn parsing_skips_malformed_pairs_and_unknown_keys_and_last_value_wins() {
    let r = record(
        " state = working :state=blocked:kind=question:progress=7:progress=60:app=cargo:app=bad/name:title=SGk:msg=SGVsbG8=:future=ok:broken:=x:oops=not valid",
    );
    assert_eq!(r.state, ProgramState::Blocked);
    assert_eq!(r.kind, Some(ProgramStatusKind::Question));
    assert_eq!(r.progress, Some(60));
    assert_eq!(r.title.as_deref(), Some("Hi"));
    assert_eq!(r.msg.as_deref(), Some("Hello"));
    assert_eq!(r.app, None);
    assert!(parse("state=working:state=future").is_none());
    assert!(parse("app=cargo").is_none());
    for value in ["", "101", "-1", "+1", "1.5", "unknown"] {
        assert_eq!(
            record(&format!("state=working:progress={value}")).progress,
            None
        );
    }
    assert_eq!(record("state=done:progress=50:kind=auth").progress, None);
    assert_eq!(record("state=working:kind=auth").kind, None);
    assert_eq!(record("state=blocked:kind=future").kind, None);
}

#[test]
fn invalid_reports_are_atomic_even_when_bad_fields_are_overwritten() {
    let mut status = ProgramStatus::default();
    status.report("state=done:app=cargo");
    let before = status.snapshot();
    for bad in [
        "state=error:msg=Z:msg=SGk=", // invalid base64, overwritten
        "state=clear:msg=AA==",       // NUL must not clear anything
        "state=error:title=fw==",     // DEL
        "state=error:msg=woU=",       // C1 control
        "state=error:msg=/w==",       // non-UTF-8
        "state=error:id=",
        "state=error:id=/child",
        "state=error:id=child/",
        "state=error:id=a//b",
        "state=error:id=a,b",
        "state=error:id=a=b",
    ] {
        status.report(bad);
        assert_eq!(status.snapshot(), before, "{bad}");
    }
    for field in [
        format!("app={}", "a".repeat(33)),
        format!("id={}", "a".repeat(33)),
        format!("id={}", ["a"; 9].join("/")),
        format!("id={}", vec!["a".repeat(32); 4].join("/")),
        format!("msg={}", "A".repeat(2733)),
        format!("msg={}", STANDARD.encode(vec![b'a'; 2049])),
        format!("title={}", STANDARD.encode(vec![b'a'; 193])),
        format!("{}=ok", "a".repeat(17)),
        format!("future={}", "a".repeat(4096)),
    ] {
        status.report(&format!("state=clear:{field}"));
        assert_eq!(status.snapshot(), before, "{field}");
    }
}

#[test]
fn valid_text_and_limits_are_accepted() {
    let msg = "é".repeat(1024);
    let title = "界".repeat(64);
    let r = record(&format!(
        "state=working:msg={}:title={}:app={}:id={}",
        STANDARD.encode(&msg),
        STANDARD.encode(&title),
        "a".repeat(32),
        ["abc"; 8].join("/")
    ));
    assert_eq!(r.msg.as_deref(), Some(msg.as_str()));
    assert_eq!(r.title.as_deref(), Some(title.as_str()));
    assert_eq!(record("state=working:msg=:title=").msg.as_deref(), Some(""));
}

#[test]
fn clear_removes_only_addressed_subtree_and_root_clear_removes_all() {
    let mut status = ProgramStatus::default();
    for id in ["build", "build/test", "builder", "other"] {
        status.report(&format!("state=done:id={id}"));
    }
    status.report("state=clear:id=build");
    assert_eq!(
        status
            .snapshot()
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        ["builder", "other"]
    );
    status.report("state=clear");
    assert_eq!(status.take_changed(), Some(vec![]));
    assert_eq!(status.take_changed(), None);
}

#[test]
fn prompt_and_exit_drop_active_records_but_preserve_results_and_idle() {
    let mut status = ProgramStatus::default();
    for state in ["idle", "working", "blocked", "done", "error"] {
        status.report(&format!("state={state}:id={state}"));
    }
    status.finish();
    assert_eq!(
        status
            .snapshot()
            .iter()
            .map(|r| r.state)
            .collect::<Vec<_>>(),
        [ProgramState::Idle, ProgramState::Done, ProgramState::Error]
    );
}

#[test]
fn capacity_evicts_least_recently_updated_and_updates_do_not_evict_others() {
    let mut status = ProgramStatus::default();
    for id in 0..256 {
        status.report(&format!("state=working:id={id}"));
    }
    status.report("state=done:id=0");
    assert_eq!(status.snapshot().len(), 256);
    status.report("state=done:id=new");
    let records = status.snapshot();
    assert_eq!(records.len(), 256);
    assert!(records.iter().any(|r| r.id == "0"));
    assert!(!records.iter().any(|r| r.id == "1"));
    assert_eq!(records[0].id, "2");
}
