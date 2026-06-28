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

    if let Some(source) = inline_source {
        let result = ctx.eval(&source);
        match result {
            Ok(val) => println!("=> {:?}", val),
            Err(e) => eprintln!("Error: {e}"),
        }
    } else if source_args.is_empty() {
        let source = "1 + 2;";
        match ctx.eval(source) {
            Ok(val) => println!("=> {:?}", val),
            Err(e) => eprintln!("Error: {e}"),
        }
    } else {
        match source_args[0].as_str() {
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
            source => {
                // Read source code from file, or use inline if it's a valid expression.
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

                let result = if let Some(ref path) = cache_path {
                    // AFPC cache: try full cache (bytecode + shapes + ICs + native code) first.
                    if let Some(cache) = rune_embed::afpc::load_afpc_cache(path) {
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
                                cache.compiled_funcs = compiled_funcs;
                                let _ = rune_embed::afpc::save_afpc_cache(path, &cache);
                                exec_result
                            }
                            Err(e) => Err(e),
                        }
                    }
                } else {
                    ctx.eval(&source)
                };

                match result {
                    Ok(val) => {
                        println!("=> {:?}", val);
                        // Source-level snapshot save for next run.
                        if let Some(ref snap_path) = snapshot_path {
                            let _ = std::fs::write(snap_path, &source);
                        }
                    }
                    Err(e) => eprintln!("Error: {e}"),
                }
            }
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
