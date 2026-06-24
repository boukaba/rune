mod test262;

fn main() {
    let args = std::env::args().skip(1);
    let mut ic_stats = false;

    // Handle flags
    let mut source_args = Vec::new();
    for arg in args {
        if arg == "--ic-stats" {
            ic_stats = true;
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
            let mut ctx = rune_embed::Context::new_small();
            match ctx.eval(source) {
                Ok(val) => println!("=> {:?}", val),
                Err(e) => eprintln!("Error: {e}"),
            }
            if ic_stats {
                eprintln!("{}", ctx.vm().dump_ic_stats());
            }
        }
    }
}
