mod test262;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        // Default: eval a simple expression
        let source = "1 + 2;";
        let mut ctx = rune_embed::Context::new();
        match ctx.eval(source) {
            Ok(val) => println!("=> {:?}", val),
            Err(e) => eprintln!("Error: {e}"),
        }
        return;
    }

    match args[0].as_str() {
        "test262" => {
            // args[1] = optional subdirectory filter, args[2] = optional suite_dir
            let subdir = args.get(1).map(|s| s.as_str());
            let suite_dir = args
                .get(2)
                .map(std::path::PathBuf::from)
                .or_else(|| std::env::var("TEST262_DIR").ok().map(std::path::PathBuf::from))
                .unwrap_or_else(|| std::path::PathBuf::from("./test262"));
            let passed = test262::run_suite(&suite_dir, subdir);
            if passed == 0 {
                std::process::exit(1);
            }
        }
        source => {
            // Treat argument as JS source
            let mut ctx = rune_embed::Context::new();
            match ctx.eval(source) {
                Ok(val) => println!("=> {:?}", val),
                Err(e) => eprintln!("Error: {e}"),
            }
        }
    }
}
