/// Test262 runner.
///
/// Usage:
///   rune test262 [path-to-test262]
///
/// Expects the Test262 suite at the given path or the `TEST262_DIR` env var.
/// If neither is provided, looks for `./test262` relative to the working directory.
use std::path::{Path, PathBuf};

/// Outcome of a single test.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Pass,
    Fail { message: String },
    Skipped { reason: String },
}

/// Metadata extracted from a Test262 test file.
#[derive(Debug, Default, Clone)]
pub struct TestMeta {
    pub description: Option<String>,
    pub negative_type: Option<String>,
    pub negative_phase: Option<String>,
    pub includes: Vec<String>,
    pub flags: Vec<String>,
    pub esid: Option<String>,
}

/// A single Test262 test case.
pub struct TestCase {
    pub path: PathBuf,
    pub meta: TestMeta,
    pub source: String,
    pub suite_dir: PathBuf,
}

/// Features we definitely do NOT support yet.
const UNSUPPORTED_FEATURES: &[&str] = &[
    "class",
    "class-fields",
    "class-fields-private",
    "class-methods-private",
    "class-static-fields-private",
    "class-static-fields-public",
    "class-static-methods-private",
    "cross-realm",
    "dynamic-import",
    "export",
    "import",
    "import-assertions",
    "import-attributes",
    "iterator-helpers",
    "json",
    "map",
    "modules",
    "promise",
    "proxy",
    "reflect",
    "regexp",
    "regexp-dotall",
    "regexp-lookbehind",
    "regexp-named-groups",
    "regexp-unicode-property-escape",
    "regexp-v-flag",
    "set",
    "set-methods",
    "shadowrealm",
    "shared-array-buffer",
    "string-trimming",
    "symbol",
    "top-level-await",
    "typedarray",
    "weakmap",
    "weakref",
    "weakset",
];

/// Flags that make a test impossible for us to run.
const UNSKIPPABLE_FLAGS: &[&str] = &["module", "onlyStrict"];

/// Run all Test262 tests in `suite_dir`, optionally limited to a subdirectory.
pub fn run_suite(suite_dir: &Path, subdir: Option<&str>) -> usize {
    let mut count = 0;
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;

    let test_dir = suite_dir.join("test");
    if !test_dir.exists() {
        eprintln!("Test262 test directory not found: {}", test_dir.display());
        eprintln!("Clone it with: git clone https://github.com/tc39/test262.git");
        return 0;
    }

    let walk_root = match subdir {
        Some(s) => suite_dir.join("test").join(s),
        None => test_dir,
    };
    if !walk_root.exists() {
        eprintln!("Test262 subdirectory not found: {}", walk_root.display());
        return 0;
    }

    for entry in walkdir::WalkDir::new(&walk_root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "js"))
    {
        count += 1;
        let path = entry.path().to_path_buf();
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  SKIP {} (read error: {})", path.display(), e);
                skipped += 1;
                continue;
            }
        };

        let meta = parse_metadata(&source);
        let test = TestCase {
            path,
            meta: meta.clone(),
            source,
            suite_dir: suite_dir.to_path_buf(),
        };

        match run_test(&test) {
            Outcome::Pass => {
                passed += 1;
                if count % 100 == 0 {
                    eprint!(".");
                }
            }
            Outcome::Fail { message } => {
                failed += 1;
                let rel = test.path.strip_prefix(suite_dir).unwrap_or(&test.path);
                eprintln!("\nFAIL {}: {}", rel.display(), message);
            }
            Outcome::Skipped { .. } => {
                skipped += 1;
                if count % 500 == 0 {
                    eprint!("s");
                }
            }
        }
    }

    eprintln!();
    eprintln!("Results: {passed} passed, {failed} failed, {skipped} skipped, {count} total");
    passed
}

/// Build the full source for a test, prepending requested harness includes.
fn build_test_source(test: &TestCase) -> Result<String, String> {
    let mut full_source = String::new();

    // Strip frontmatter for execution
    let source = strip_frontmatter(&test.source);

    // If `raw` flag is set, no harness includes
    if test.meta.flags.iter().any(|f| f == "raw") {
        return Ok(source);
    }

    let harness_dir = test.suite_dir.join("harness");

    // Don't inject sta.js — builtins provide Test262Error, $DONOTEVALUATE
    // Don't inject assert.js — uses unsupported features (typeof, JSON, bigint, etc.)
    // Include other requested harness files (skip known ones that use unsupported features)
    const SKIP_INCLUDES: &[&str] = &[
        "sta.js",
        "sta",
        "assert.js",
        "assert",
        "fnGlobalObject.js",
        "doneprintHandle.js",
        "testTypedArray.js",
        "testBigIntTypedArray.js",
        "byteConversionValues.js",
        "nans.js",
        "proxyTrapsHelper.js",
        "dateConstants.js",
        "propertyHelper.js",
    ];

    for include in &test.meta.includes {
        if SKIP_INCLUDES.contains(&include.as_str()) {
            continue;
        }
        let path = harness_dir.join(include);
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read harness {}: {}", include, e))?;
        full_source.push_str(&content);
        full_source.push('\n');
    }

    full_source.push_str(&source);
    Ok(full_source)
}

/// Strip the YAML frontmatter block from test source.
fn strip_frontmatter(source: &str) -> String {
    let start = source.find("/*---");
    let end = source.rfind("---*/");
    match (start, end) {
        (Some(0), Some(e)) => {
            let after = source[e + 5..]
                .find('\n')
                .map(|i| e + 5 + i + 1)
                .unwrap_or(source.len());
            source[after..].trim().to_string()
        }
        _ => source.to_string(),
    }
}

/// Run a single test case, catching Rust panics to keep the runner alive.
fn run_test(test: &TestCase) -> Outcome {
    // Skip tests with unsupported features
    if let Some(features) = test
        .meta
        .flags
        .iter()
        .find(|f| UNSUPPORTED_FEATURES.contains(&f.as_str()))
    {
        return Outcome::Skipped {
            reason: format!("unsupported feature: {features}"),
        };
    }

    // Skip tests with flags we can't handle
    for flag in &test.meta.flags {
        if UNSKIPPABLE_FLAGS.contains(&flag.as_str()) {
            return Outcome::Skipped {
                reason: format!("unsupported flag: {flag}"),
            };
        }
    }

    let source = match build_test_source(test) {
        Ok(s) => s,
        Err(e) => return Outcome::Skipped { reason: e },
    };

    // Skip tests containing $DONOTEVALUATE (must not be executed)
    if source.contains("$DONOTEVALUATE") {
        return Outcome::Skipped {
            reason: "contains $DONOTEVALUATE".to_string(),
        };
    }

    // Check for negative tests (expected to fail)
    if let Some(ref neg_type) = test.meta.negative_type {
        let phase = test.meta.negative_phase.as_deref().unwrap_or("runtime");

        if phase == "parse" {
            let mut ctx = rune_embed::Context::new();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ctx.eval(&source))) {
                Ok(Ok(_)) => Outcome::Fail {
                    message: format!("expected parse error {neg_type}, but parsed successfully"),
                },
                _ => Outcome::Pass,
            }
        } else {
            let mut ctx = rune_embed::Context::new();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ctx.eval(&source))) {
                Ok(Ok(_)) => Outcome::Fail {
                    message: format!("expected runtime error {neg_type}, but ran successfully"),
                },
                _ => Outcome::Pass,
            }
        }
    } else {
        // Normal test
        let mut ctx = rune_embed::Context::new();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ctx.eval(&source))) {
            Ok(Ok(_)) => {
                // Some tests use throw new Test262Error(...) instead of assert.* functions.
                // If the test completed without error, it passed.
                Outcome::Pass
            }
            Ok(Err(e)) => Outcome::Fail {
                message: format!("runtime error: {e}"),
            },
            Err(panic) => {
                let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                Outcome::Fail {
                    message: format!("panic: {msg}"),
                }
            }
        }
    }
}

/// Parse Test262 YAML frontmatter.
fn parse_metadata(source: &str) -> TestMeta {
    let mut meta = TestMeta::default();

    let start = source.find("/*---").map(|i| i + 5);
    let end = source.rfind("---*/");

    let body = match (start, end) {
        (Some(s), Some(e)) if s < e => &source[s..e],
        _ => return meta,
    };

    let mut negative_next = false;
    for line in body.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("description: ") {
            meta.description = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("esid: ") {
            meta.esid = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("includes: ") {
            let list = val.trim().strip_prefix('[').unwrap_or(val.trim());
            let list = list.strip_suffix(']').unwrap_or(list.trim());
            meta.includes = list
                .split(',')
                .map(|s| s.trim().trim_matches('"').to_string())
                .collect();
        } else if let Some(val) = line.strip_prefix("flags: ") {
            let list = val.trim().strip_prefix('[').unwrap_or(val.trim());
            let list = list.strip_suffix(']').unwrap_or(list.trim());
            meta.flags = list
                .split(',')
                .map(|s| s.trim().trim_matches('"').to_string())
                .collect();
        } else if line == "negative:" {
            negative_next = true;
        } else if negative_next {
            if let Some(val) = line.strip_prefix("  type: ") {
                meta.negative_type = Some(val.trim().to_string());
            } else if let Some(val) = line.strip_prefix("  phase: ") {
                meta.negative_phase = Some(val.trim().to_string());
            }
            if line.is_empty() || !line.starts_with("  ") {
                negative_next = false;
            }
        }
    }

    meta
}
