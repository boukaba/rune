mod test262;

fn main() {
    let args = std::env::args().skip(1);
    let mut ic_stats = false;
    let mut trace_stats = false;
    let mut snapshot_path: Option<String> = None;
    let mut cache_path: Option<String> = None;

    let mut source_args = Vec::new();
    for arg in args {
        if arg == "--ic-stats" {
            ic_stats = true;
        } else if arg == "--trace-stats" {
            trace_stats = true;
        } else if arg == "--snapshot" {
            // Next arg is the snapshot path, or use ".rune-cache"
            // Actually — save snapshot AFTER eval to the given path
            snapshot_path = Some(".rune-cache".to_string());
        } else if let Some(rest) = arg.strip_prefix("--snapshot=") {
            snapshot_path = Some(rest.to_string());
        } else if arg == "--cache" {
            cache_path = Some(".rune-cache".to_string());
        } else if let Some(rest) = arg.strip_prefix("--cache=") {
            cache_path = Some(rest.to_string());
        } else {
            source_args.push(arg);
        }
    }

    if source_args.is_empty() {
        let source = "1 + 2;";
        let mut ctx = rune_embed::Context::new_small();
        match ctx.eval(source) {
            Ok(val) => println!("=> {:?}", val),
            Err(e) => eprintln!("Error: {e}"),
        }
        if ic_stats {
            eprintln!("{}", ctx.vm().dump_ic_stats());
        }
        if trace_stats {
            eprintln!("{}", ctx.vm().dump_trace_stats());
        }
        return;
    }

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
            // Source-level snapshot cache (text). If both --snapshot and --cache
            // are provided, --cache takes precedence for execution.
            let source = if snapshot_path.is_some() && cache_path.is_none() {
                if let Some(ref snap_path) = snapshot_path {
                    if let Ok(cached) = std::fs::read_to_string(snap_path) {
                        cached
                    } else {
                        source.to_string()
                    }
                } else {
                    source.to_string()
                }
            } else {
                source.to_string()
            };

            let mut ctx = rune_embed::Context::new_small();
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
            if ic_stats {
                eprintln!("{}", ctx.vm().dump_ic_stats());
            }
            if trace_stats {
                eprintln!("{}", ctx.vm().dump_trace_stats());
            }
        }
    }
}
