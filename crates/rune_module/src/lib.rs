// ESM module compilation and loading (§16 Module Grammar, §31 Modules).
// The loader compiles a module-goal source into a BytecodeProgram carrying
// `ModuleInfo` linkage metadata; the rune_embed Context walks the import
// graph (resolve → compile → recurse) and the VM evaluates it.

use rune_bytecode::opcode::BytecodeProgram;
use rune_parser::Parser;
use rune_parser::emitter::Emitter;

/// Parse (Module goal) and compile a module source into a bytecode program
/// with module linkage metadata (`program.module`, `program.is_module`).
pub fn compile_module(source: &str) -> Result<BytecodeProgram, String> {
    let mut parser = Parser::new_module(source);
    let program = parser.parse();
    if !parser.errors.is_empty() {
        return Err(format!(
            "SyntaxError: {}",
            parser.errors.first().unwrap_or(&String::new())
        ));
    }
    let emitter = Emitter::new();
    Ok(emitter.emit_module_program(&program))
}

/// Parse a module source for static import-graph discovery without compiling.
/// Returns the list of dependency specifiers in import order.
pub fn module_dependencies(source: &str) -> Result<Vec<String>, String> {
    let program = compile_module(source)?;
    let mut deps = Vec::new();
    if let Some(info) = &program.module {
        for imp in &info.imports {
            if !deps.contains(&imp.specifier) {
                deps.push(imp.specifier.clone());
            }
        }
        for (_, spec, _) in &info.indirect_exports {
            if !deps.contains(spec) {
                deps.push(spec.clone());
            }
        }
        for spec in &info.star_exports {
            if !deps.contains(spec) {
                deps.push(spec.clone());
            }
        }
        for (_, spec) in &info.namespace_exports {
            if !deps.contains(spec) {
                deps.push(spec.clone());
            }
        }
    }
    Ok(deps)
}

/// Default filesystem resolver: resolves `specifier` relative to the
/// directory of the referrer module. Bare specifiers (no `./`/`../` prefix)
/// resolve under the referrer's directory too (no node_modules lookup).
pub fn fs_resolve(specifier: &str, referrer: &str) -> Result<String, String> {
    let path = std::path::Path::new(referrer)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(specifier);
    let path = if path.extension().is_none() {
        let mut js = path.clone();
        js.set_extension("js");
        js
    } else {
        path
    };
    std::fs::read_to_string(&path)
        .map_err(|e| format!("Cannot load module {specifier} (referrer {referrer}): {e}"))
}
