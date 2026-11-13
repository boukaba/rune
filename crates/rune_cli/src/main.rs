mod test262;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut ic_stats = false;
    let mut trace_stats = false;
    let mut jit_stats = false;
    let mut snapshot_path: Option<String> = None;
    let mut cache_path: Option<String> = None;
    let mut inline_source: Option<String> = None;
    let mut enable_inlining = false;
    let mut stencil_jit = false;
    let mut enable_jit = false;
    let mut allow_private_fetch = false;
    let mut build_out: Option<String> = None;

    let mut source_args: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--ic-stats" {
            ic_stats = true;
        } else if arg == "--trace-stats" {
            trace_stats = true;
        } else if arg == "--jit-stats" {
            jit_stats = true;
        } else if arg == "-e" || arg == "--eval" {
            i += 1;
            if i < args.len() {
                inline_source = Some(args[i].to_string());
            }
        } else if arg == "--snapshot" {
            snapshot_path = Some(".rune-cache".to_string());
        } else if let Some(rest) = arg.strip_prefix("--snapshot=") {
            snapshot_path = Some(rest.to_string());
        } else if arg == "--cache" {
            cache_path = Some(".rune-cache".to_string());
        } else if let Some(rest) = arg.strip_prefix("--cache=") {
            cache_path = Some(rest.to_string());
        } else if arg == "--inline" {
            enable_inlining = true;
        } else if arg == "--no-inline" {
            enable_inlining = false;
        } else if arg == "--jit" {
            enable_jit = true;
        } else if arg == "--no-jit" {
            enable_jit = false;
        } else if arg == "--allow-private-fetch" {
            allow_private_fetch = true;
        } else if arg == "--out" {
            i += 1;
            if i < args.len() {
                build_out = Some(args[i].to_string());
            }
        } else if let Some(rest) = arg.strip_prefix("--out=") {
            build_out = Some(rest.to_string());
        } else if arg == "--stencil-jit" {
            stencil_jit = true;
        } else if arg == "--no-stencil-jit" {
            stencil_jit = false;
        } else {
            source_args.push(arg.clone());
        }
        i += 1;
    }

    let mut ctx = rune_embed::Context::new();
    ctx.enable_inlining = enable_inlining;
    ctx.stencil_jit = stencil_jit;
    ctx.enable_jit = enable_jit;
    // Install timer/fetch host globals and the event-loop job queues. The
    // test262 harness keeps a bare context; scripts get the runtime globals.
    let mut runtime_jobs = if source_args.first().is_some_and(|a| a == "test262") {
        None
    } else {
        let mut jobs = rune_runtime::install(&mut ctx);
        jobs.set_fetch_policy(rune_runtime::FetchPolicy {
            allow_private_networks: allow_private_fetch,
            ..Default::default()
        });
        jobs.set_fetcher(http_fetch);
        Some(jobs)
    };

    if let Some(source) = inline_source {
        let result = ctx.eval(&source);
        match result {
            Ok(val) => println!("=> {:?}", val),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    } else if source_args.is_empty() {
        let source = "1 + 2;";
        match ctx.eval(source) {
            Ok(val) => println!("=> {:?}", val),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        match source_args[0].as_str() {
            "build" => {
                let Some(entry) = source_args.get(1) else {
                    eprintln!("usage: rune build <script.js> [--out <app.runeapp>]");
                    std::process::exit(1);
                };
                let source = match std::fs::read_to_string(entry) {
                    Ok(source) => source,
                    Err(error) => {
                        eprintln!("Error: cannot read {entry}: {error}");
                        std::process::exit(1);
                    }
                };
                let out = build_out.clone().unwrap_or_else(|| {
                    std::path::Path::new(entry)
                        .with_extension("runeapp")
                        .to_string_lossy()
                        .into_owned()
                });
                let bytecode = match ctx.compile(&source) {
                    Ok(bytecode) => bytecode,
                    Err(error) => {
                        eprintln!("Error: {error}");
                        std::process::exit(1);
                    }
                };
                let mut cache = rune_embed::afpc::AfpcCache::from_runtime(bytecode, Vec::new());
                cache.source_hash = rune_embed::afpc::source_hash(&source);
                match rune_embed::bundle::save_bundle(&out, &source, &cache) {
                    Ok(size) => println!("built {out} ({size} bytes)"),
                    Err(error) => {
                        eprintln!("Error: cannot write {out}: {error}");
                        std::process::exit(1);
                    }
                }
            }
            "run" => {
                let Some(path) = source_args.get(1) else {
                    eprintln!("usage: rune run <app.runeapp>");
                    std::process::exit(1);
                };
                let Some(bundle) = rune_embed::bundle::load_bundle(path) else {
                    eprintln!("Error: invalid or incompatible bundle {path}");
                    std::process::exit(1);
                };
                let cache = bundle.cache;
                cache.restore_shapes();
                if !cache.compiled_funcs.is_empty() {
                    let native = rune_embed::afpc::InstalledNativeCode::from_cache(&cache);
                    ctx.install_native_code(native);
                }
                ctx.set_ics(cache.ic_table);
                let result = ctx.eval_bytecode_owned(cache.bytecode);
                if result.is_ok() {
                    if let Some(jobs) = runtime_jobs.as_mut() {
                        if let Err(error) = jobs.run_jobs(&mut ctx) {
                            eprintln!("Error: {error}");
                            std::process::exit(1);
                        }
                    }
                }
                match result {
                    Ok(value) => println!("=> {:?}", value),
                    Err(error) => {
                        eprintln!("Error: {error}");
                        std::process::exit(1);
                    }
                }
            }
            "test262" => {
                let subdir = source_args.get(1).map(|s| s.as_str());
                let suite_dir = source_args
                    .get(2)
                    .map(std::path::PathBuf::from)
                    .or_else(|| {
                        std::env::var("TEST262_DIR")
                            .ok()
                            .map(std::path::PathBuf::from)
                    })
                    .unwrap_or_else(|| std::path::PathBuf::from("./test262"));
                let passed = test262::run_suite(&suite_dir, subdir);
                if passed == 0 {
                    std::process::exit(1);
                }
            }
            "test262-exec" => {
                // Isolated single-test executor for the conformance harness.
                // Args: test262-exec <file> <ok|throw|parse>
                // Exit codes: 0 = expected outcome, 1 = wrong outcome,
                //             2 = panic/crash, 3 = usage/IO error.
                let path = source_args.get(1).cloned();
                let mode = source_args.get(2).map(|s| s.as_str()).unwrap_or("");
                if mode.is_empty() || path.is_none() {
                    eprintln!("usage: rune test262-exec <file> <ok|throw|parse>");
                    std::process::exit(3);
                }
                let src = match std::fs::read_to_string(path.unwrap()) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("read error: {e}");
                        std::process::exit(3);
                    }
                };
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match mode {
                        "parse" => {
                            let ctx = rune_embed::Context::new_large();
                            ctx.compile(&src).map(|_| ())
                        }
                        _ => {
                            // 64 MiB heap: spec-correct multi-MB results (B1f-3
                            // 1M-element maps) don't fit the 16 MiB default.
                            let mut ctx = rune_embed::Context::new_large();
                            let mut jobs = rune_runtime::install(&mut ctx);
                            let done = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
                            let done_sink = done.clone();
                            ctx.register_host_fn("$DONE", 1, move |_gc, _this, args, _vm| {
                                if let Some(error) = args.first() {
                                    if !error.is_undefined() {
                                        let text =
                                            rune_interpreter::builtins::value_to_js_string(*error);
                                        if done_sink.borrow().is_none() {
                                            *done_sink.borrow_mut() = Some(text);
                                        }
                                    }
                                }
                                rune_core::value::Value::undefined()
                            });
                            let eval = ctx.eval(&src).map(|_| ());
                            if eval.is_ok() {
                                // Timers plus promise microtasks; `$DONE` may be
                                // called from either channel.
                                jobs.run_jobs(&mut ctx)?;
                                if let Some(error) = done.borrow_mut().take() {
                                    return Err(error);
                                }
                            }
                            eval
                        }
                    }));
                let code = match result {
                    Err(_) => {
                        // Panic inside the engine — treat as crash.
                        2
                    }
                    Ok(Ok(())) => {
                        if mode == "throw" || mode == "parse" {
                            1
                        } else {
                            0
                        }
                    }
                    Ok(Err(_)) => {
                        if mode == "ok" {
                            1
                        } else {
                            0
                        }
                    }
                };
                std::process::exit(code);
            }
            source => {
                // Read source code from file, or use inline if it's a valid expression.
                let entry_path = if std::path::Path::new(source).exists() {
                    Some(source.to_string())
                } else {
                    None
                };
                let source = if let Ok(code) = std::fs::read_to_string(source) {
                    code
                } else {
                    source.to_string()
                };

                // Source-level snapshot cache (text). If both --snapshot and --cache
                // are provided, --cache takes precedence for execution.
                let source = if snapshot_path.is_some() && cache_path.is_none() {
                    if let Some(ref snap_path) = snapshot_path {
                        if let Ok(cached) = std::fs::read_to_string(snap_path) {
                            cached
                        } else {
                            source
                        }
                    } else {
                        source
                    }
                } else {
                    source
                };

                let source_fingerprint = rune_embed::afpc::source_hash(&source);
                let result = if let Some(ref path) = cache_path {
                    // AFPC cache: only reuse a cache compiled from this exact
                    // source. The header also carries an engine/ABI tag, so a
                    // stale or foreign native cache falls back to compilation.
                    if let Some(cache) = rune_embed::afpc::load_afpc_cache(path)
                        .filter(|c| c.source_hash == source_fingerprint)
                    {
                        cache.restore_shapes();
                        if !cache.compiled_funcs.is_empty() {
                            let native = rune_embed::afpc::InstalledNativeCode::from_cache(&cache);
                            ctx.install_native_code(native);
                        }
                        ctx.set_ics(cache.ic_table);
                        ctx.eval_bytecode_owned(cache.bytecode)
                    } else {
                        match ctx.compile(&source) {
                            Ok(bytecode) => {
                                // Compile hot functions to native code on supported platforms.
                                let compiled_funcs =
                                    rune_embed::afpc::aot_compile_functions(&bytecode);
                                // Execute once to warm up ICs, then save the cache.
                                let exec_result = ctx.eval_bytecode_owned(bytecode.clone());
                                let ics = ctx.ics();
                                let mut cache =
                                    rune_embed::afpc::AfpcCache::from_runtime(bytecode, ics);
                                cache.source_hash = source_fingerprint;
                                cache.compiled_funcs = compiled_funcs;
                                let _ = rune_embed::afpc::save_afpc_cache(path, &cache);
                                exec_result
                            }
                            Err(e) => Err(e),
                        }
                    }
                } else {
                    // ESM: `.mjs` files (and any file whose script-mode parse
                    // reports Import/Export tokens) evaluate as modules with a
                    // filesystem resolver. Script mode otherwise.
                    let is_module_file = entry_path
                        .as_ref()
                        .map(|p| p.ends_with(".mjs"))
                        .unwrap_or(false);
                    let script_result = if is_module_file {
                        None
                    } else {
                        Some(ctx.eval(&source))
                    };
                    let module_err = script_result.as_ref().and_then(|r| r.as_ref().err());
                    let looks_like_module = module_err
                        .map(|e| e.contains("Import") || e.contains("Export"))
                        .unwrap_or(false);
                    if is_module_file || looks_like_module {
                        let entry = entry_path.unwrap_or_else(|| "<inline>".to_string());
                        let mut resolver = |spec: &str, referrer: &str| -> Result<String, String> {
                            let referrer = if referrer == "<entry>" {
                                &entry
                            } else {
                                referrer
                            };
                            rune_module::fs_resolve(spec, referrer)
                        };
                        ctx.eval_module(&source, &mut resolver)
                    } else {
                        script_result.unwrap()
                    }
                };

                match result {
                    Ok(val) => {
                        println!("=> {:?}", val);
                        // Source-level snapshot save for next run.
                        if let Some(ref snap_path) = snapshot_path {
                            let _ = std::fs::write(snap_path, &source);
                        }
                    }
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            }
        }
    }

    if let Some(ref mut jobs) = runtime_jobs {
        if let Err(e) = jobs.run_jobs(&mut ctx) {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }

    if ic_stats {
        eprintln!("{}", ctx.vm().dump_ic_stats());
    }
    if trace_stats {
        eprintln!("{}", ctx.vm().dump_trace_stats());
    }
    if jit_stats {
        eprintln!("{}", ctx.vm().dump_jit_stats());
    }
}

/// Default CLI fetch transport: real HTTP/HTTPS via ureq. Redirects and TLS
/// are the crate defaults; the response body is exposed as text.
fn http_fetch(req: &rune_runtime::FetchRequest) -> Result<rune_runtime::FetchResponse, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .build();
    let mut request = agent.request(&req.method, &req.url);
    for (name, value) in &req.headers {
        request = request.set(name, value);
    }
    let response = if let Some(body) = &req.body {
        request.send_string(body)
    } else {
        request.call()
    };
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(err) => return Err(err.to_string()),
    };
    let status = response.status();
    let status_text = response.status_text().to_string();
    let mut headers = Vec::new();
    for name in response.headers_names() {
        if let Some(value) = response.header(&name) {
            headers.push((name, value.to_string()));
        }
    }
    let body = response
        .into_string()
        .map_err(|e| format!("fetch body: {e}"))?;
    Ok(rune_runtime::FetchResponse {
        status,
        status_text,
        headers,
        body,
    })
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn http_fetch_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                )
                .expect("write");
        });
        let response = http_fetch(&rune_runtime::FetchRequest {
            url: format!("http://{addr}/"),
            method: "GET".to_string(),
            headers: Vec::new(),
            body: None,
        })
        .expect("fetch");
        server.join().expect("server");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "hello");
        assert!(
            response
                .headers
                .iter()
                .any(|(k, v)| k == "content-type" && v == "text/plain")
        );
    }
}
