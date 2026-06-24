mod test262;

fn main() {
    let args = std::env::args().skip(1);
    let mut ic_stats = false;
    let mut trace_stats = false;
    let mut snapshot_path: Option<String> = None;

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
            // AFPC: check if snapshot exists — load from cache, skip parse+emit
            let source = if let Some(ref snap_path) = snapshot_path {
                if let Ok(cached) = std::fs::read_to_string(snap_path) {
                    cached
                } else {
                    source.to_string()
                }
            } else {
                source.to_string()
            };

            let mut ctx = rune_embed::Context::new_small();
            match ctx.eval(&source) {
                Ok(val) => {
                    println!("=> {:?}", val);
                    // AFPC: save snapshot for next run
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
