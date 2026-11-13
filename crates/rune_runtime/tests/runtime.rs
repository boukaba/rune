use rune_interpreter::builtins::value_to_js_string;
use rune_runtime::Runtime;

fn eval_str(rt: &mut Runtime, source: &str) -> String {
    value_to_js_string(rt.context_mut().eval(source).unwrap())
}

#[test]
fn timers_run_after_eval_in_order() {
    let mut rt = Runtime::new();
    rt.eval(
        r#"
        var out = "";
        setTimeout(function () { out += "a"; }, 0);
        setTimeout(function () { out += "b"; }, 0);
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut rt, "out"), "ab");
}

#[test]
fn timer_args_clear_and_nested_timers() {
    let mut rt = Runtime::new();
    rt.eval(
        r#"
        var got = "";
        var id = setTimeout(function (a, b) { got = a + b; }, 0, "x", "y");
        var bad = setTimeout(function () { got = "bad"; }, 0);
        clearTimeout(bad);
        setTimeout(function () {
            setTimeout(function () { got += "!"; }, 0);
        }, 0);
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut rt, "got"), "xy!");
}

#[test]
fn timer_promise_microtasks_drain() {
    let mut rt = Runtime::new();
    rt.eval(
        r#"
        var out = "";
        setTimeout(function () {
            Promise.resolve(1).then(function (v) { out = "p" + v; });
        }, 0);
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut rt, "out"), "p1");
}

#[test]
fn host_functions_compose_with_runtime() {
    let mut rt = Runtime::new();
    rt.context_mut()
        .register_host_fn("hostAdd", 2, |_gc, _this, args, _vm| {
            let a = args[0].as_smi().unwrap_or(0);
            let b = args[1].as_smi().unwrap_or(0);
            rune_core::value::Value::smi(a + b)
        });
    let v = rt.eval("hostAdd(20, 22)").unwrap();
    assert_eq!(v.as_smi(), Some(42));
}

#[test]
fn fetch_resolves_promise_and_builds_response() {
    use rune_runtime::{FetchRequest, FetchResponse};
    let mut rt = Runtime::new();
    rt.set_fetcher(|req: &FetchRequest| {
        assert_eq!(req.url, "https://example.com/api");
        assert_eq!(req.method, "GET");
        Ok(FetchResponse {
            status: 200,
            status_text: "OK".to_string(),
            headers: vec![("content-type".to_string(), "text/plain".to_string())],
            body: "hello".to_string(),
        })
    });
    rt.eval(
        r#"
        var out = "";
        fetch("https://example.com/api").then(function (r) {
            r.text().then(function (t) {
                out = r.status + "|" + r.statusText + "|" + r.ok + "|" +
                      r.headers.get("Content-Type") + "|" + t + "|" + r.url;
            });
        });
        "#,
    )
    .unwrap();
    assert_eq!(
        eval_str(&mut rt, "out"),
        "200|OK|true|text/plain|hello|https://example.com/api"
    );
}

#[test]
fn fetch_rejection_reaches_catch() {
    let mut rt = Runtime::new();
    rt.set_fetcher(|_req| Err("network down".to_string()));
    rt.eval(
        r#"
        var out = "";
        fetch("https://example.com/fail").catch(function (e) { out = e; });
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut rt, "out"), "network down");
}

#[test]
fn fetch_accepts_url_objects_and_headers() {
    use rune_runtime::{FetchRequest, FetchResponse};
    let mut rt = Runtime::new();
    rt.set_fetcher(|req: &FetchRequest| {
        assert_eq!(req.url, "https://example.com/x?q=1");
        assert_eq!(req.method, "POST");
        assert_eq!(req.body.as_deref(), Some("payload"));
        assert!(req.headers.iter().any(|(k, v)| k == "X-Test" && v == "yes"));
        Ok(FetchResponse {
            status: 201,
            status_text: "Created".to_string(),
            headers: vec![],
            body: "{}".to_string(),
        })
    });
    rt.eval(
        r#"
        var u = new URL("https://example.com/x?q=1");
        var out = 0;
        fetch(u, { method: "post", body: "payload", headers: { "X-Test": "yes" } })
            .then(function (r) { out = r.status; });
        "#,
    )
    .unwrap();
    assert_eq!(rt.context_mut().eval("out").unwrap().as_smi(), Some(201));
}

#[test]
fn fetch_policy_blocks_private_and_unsupported_schemes() {
    use rune_runtime::FetchPolicy;
    let policy = FetchPolicy::default();
    assert!(policy.check("https://example.com/a").is_ok());
    assert!(policy.check("http://example.com/a").is_ok());
    for url in [
        "http://localhost/a",
        "http://127.0.0.1/a",
        "http://10.0.0.1/a",
        "http://192.168.1.1/a",
        "http://169.254.1.1/a",
        "http://[::1]/a",
        "file:///etc/passwd",
        "ftp://example.com/a",
    ] {
        assert!(policy.check(url).is_err(), "{url} should be blocked");
    }
}

#[test]
fn fetch_policy_allowlist_and_private_flag() {
    use rune_runtime::FetchPolicy;
    let policy = FetchPolicy {
        allowed_hosts: Some(vec![
            "example.com".to_string(),
            "*.internal.test".to_string(),
        ]),
        ..Default::default()
    };
    assert!(policy.check("https://example.com/").is_ok());
    assert!(policy.check("https://api.internal.test/").is_ok());
    assert!(policy.check("https://evil.example.com/").is_err());
    assert!(policy.check("https://127.0.0.1/").is_err());

    let private_ok = FetchPolicy {
        allow_private_networks: true,
        ..Default::default()
    };
    assert!(private_ok.check("http://127.0.0.1:8765/").is_ok());
}

#[test]
fn runtime_fetch_policy_rejects_before_transport() {
    let mut rt = Runtime::new();
    rt.set_fetcher(|_req| panic!("transport must not run for a blocked host"));
    rt.eval(
        r#"
        var out = "";
        fetch("http://127.0.0.1:1/").catch(function (e) { out = e; });
        "#,
    )
    .unwrap();
    let out = eval_str(&mut rt, "out");
    assert!(
        out.contains("private/loopback"),
        "unexpected rejection: {out}"
    );
}
