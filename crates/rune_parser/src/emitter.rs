use crate::ast::*;
use rune_bytecode::opcode::{BytecodeProgram, Instruction, ModuleImport, ModuleInfo, Opcode};
use std::collections::HashMap;

/// A lexical scope binding with its allocated absolute slot index.
struct LexicalBinding {
    name: String,
    slot: usize,
}

/// Kind of a pending do-while loop jump, resolved when the loop ends.
enum DestructureStore {
    /// Declaration context: `var`/`let`/`const` declare the binding.
    Decl(VarKind),
    /// Assignment context: store into an existing binding.
    Assign,
}

enum LoopJumpKind {
    Break,
    Continue,
}

/// Bytecode emitter. Walks an AST and produces instructions.
pub struct Emitter {
    pub instructions: Vec<Instruction>,
    pub is_generator: bool,
    pub is_async: bool,
    pub named_function: bool,
    pub string_pool: Vec<String>,
    pub float_pool: Vec<f64>,
    pub regex_pool: Vec<(String, String)>,
    pub nested_funcs: Vec<BytecodeProgram>,
    locals: Vec<String>,
    /// Lexical scope stack (let/const per block). Each scope knows its
    /// outermost lexicals + the base slot index in the flat lexical slot array.
    lexical_scopes: Vec<Vec<LexicalBinding>>,
    /// Total lexical slots allocated in the current function.
    lexical_slot_count: usize,
    /// Names of variables in THIS function that are captured by inner closures.
    captured_names: Vec<String>,
    /// How many env slots this function's env object has (0 = no env).
    captured_env_size: usize,
    /// Captured_names of enclosing functions, ordered closest-first.
    /// Used by inner functions to resolve free variables via LoadCaptured(depth, slot).
    env_scope_stack: Vec<Vec<String>>,
    loop_exit_stack: Vec<usize>,
    loop_cont_stack: Vec<usize>,
    /// Pending `break`/`continue` jump positions of the innermost do-while loop,
    /// patched at the loop end (do-while uses `usize::MAX` sentinels in
    /// `loop_exit_stack`/`loop_cont_stack` because its cond position is only
    /// known after the body is emitted).
    pending_loop_jumps: Vec<Vec<(usize, LoopJumpKind)>>,
    switch_exit_stack: Vec<usize>,
    switch_break_jumps: Vec<usize>,
    /// Private field names declared by the enclosing class (for #name → slot index resolution).
    private_field_names: Vec<String>,
    /// Module goal mode: top-level bindings are module bindings (StoreGlobal
    /// routes into the module environment at runtime).
    module_mode: bool,
    /// Imported binding names → index into the module's `ModuleInfo.imports`.
    /// Reads/writes of these emit LoadModuleImport/StoreModuleImport (live bindings).
    module_imports: HashMap<Box<str>, usize>,
    /// Module bindings that are exported under a DIFFERENT name (e.g.
    /// `export {a as b}`): stored name → export names. After each module-level
    /// store of the stored name, an ExportSync for each export name is emitted.
    module_export_renames: HashMap<String, Vec<String>>,
}

impl Default for Emitter {
    fn default() -> Self {
        Self::new()
    }
}

impl Emitter {
    pub fn new() -> Self {
        Emitter {
            instructions: Vec::new(),
            is_generator: false,
            is_async: false,
            named_function: false,
            string_pool: Vec::new(),
            float_pool: Vec::new(),
            regex_pool: Vec::new(),
            nested_funcs: Vec::new(),
            locals: Vec::new(),
            lexical_scopes: Vec::new(),
            lexical_slot_count: 0,
            captured_names: Vec::new(),
            captured_env_size: 0,
            env_scope_stack: Vec::new(),
            loop_exit_stack: Vec::new(),
            loop_cont_stack: Vec::new(),
            pending_loop_jumps: Vec::new(),
            switch_exit_stack: Vec::new(),
            switch_break_jumps: Vec::new(),
            private_field_names: Vec::new(),
            module_mode: false,
            module_imports: HashMap::new(),
            module_export_renames: HashMap::new(),
        }
    }

    fn emit(&mut self, op: Opcode, operands: Vec<i64>) {
        self.instructions.push(Instruction::new(op, operands));
    }

    fn patch(&mut self, index: usize, target: usize) {
        self.instructions[index].operands[0] = target as i64;
    }

    fn patch_operand(&mut self, instr_idx: usize, operand: usize, target: i64) {
        self.instructions[instr_idx].operands[operand] = target;
    }

    fn current(&self) -> usize {
        self.instructions.len()
    }

    fn intern_string(&mut self, s: &str) -> usize {
        if let Some(idx) = self.string_pool.iter().position(|x| x == s) {
            return idx;
        }
        let idx = self.string_pool.len();
        self.string_pool.push(s.to_string());
        idx
    }

    fn intern_float(&mut self, v: f64) -> usize {
        if let Some(idx) = self
            .float_pool
            .iter()
            .position(|x| x.to_bits() == v.to_bits())
        {
            return idx;
        }
        let idx = self.float_pool.len();
        self.float_pool.push(v);
        idx
    }

    pub fn emit_program(&mut self, prog: &Program) {
        if prog.body.is_empty() {
            self.emit(Opcode::LoadUndefined, vec![]);
            self.emit(Opcode::Return, vec![]);
            return;
        }
        // Wrap program body in an implicit lexical scope for let/const/TDZ
        let lexical_count = self.count_lexicals(&prog.body);
        if lexical_count > 0 {
            self.enter_lexical_scope(&prog.body, lexical_count);
            self.emit(Opcode::BlockEnter, vec![lexical_count as i64]);
        }
        let last_idx = prog.body.len() - 1;
        for stmt in &prog.body[..last_idx] {
            self.emit_statement(stmt);
        }
        self.emit_last_statement(&prog.body[last_idx]);
        if lexical_count > 0 {
            self.emit(Opcode::BlockLeave, vec![]);
            self.leave_lexical_scope();
        }
    }

    /// Emit a module-goal program (ESM §16).
    ///
    /// Instruction layout (matches spec instantiation/evaluation ordering):
    ///   section 1: hoisted bindings — function/class/var/let/const declarations
    ///              create module bindings in source order (functions fully
    ///              created; everything else bound to `undefined`)
    ///   section 2: `ImportModule` per import (evaluates dependencies in DFS
    ///              order; cycle-safe via per-module status)
    ///   section 3: remaining statements in source order (var initializers,
    ///              let/const initializers, class evaluation, everything else)
    /// Top-level module bindings route through StoreGlobal, which the VM
    /// redirects into the module environment while it runs. The program ends
    /// with `undefined` (module evaluation has no completion value).
    pub fn emit_module_program(mut self, prog: &Program) -> BytecodeProgram {
        self.module_mode = true;
        let mut module = ModuleInfo {
            imports: Vec::new(),
            local_exports: Vec::new(),
            indirect_exports: Vec::new(),
            star_exports: Vec::new(),
            namespace_exports: Vec::new(),
        };
        // export_name → local/stored name
        let mut export_map: Vec<(String, String)> = Vec::new();
        // stored name → export names needing ExportSync after each store
        let mut renames: HashMap<String, Vec<String>> = HashMap::new();
        let mut hoisted_functions: Vec<FnNode> = Vec::new();
        let mut hoisted_vars: Vec<(String, VarKind)> = Vec::new();
        let mut hoisted_classes: Vec<ClassNode> = Vec::new();
        let mut imports: Vec<&ImportDecl> = Vec::new();

        // ---- classify top-level statements ----
        for stmt in &prog.body {
            match stmt {
                Stmt::Import(imp, _) => {
                    imports.push(imp);
                }
                Stmt::Export(exp, _) => match &exp.kind {
                    ExportKind::Named(names) => {
                        for (exported, local) in names {
                            // The parser tuple is (local_binding, exported_name)
                            // — `export { local as exported }` yields
                            // ("local", "exported").
                            export_map.push((local.to_string(), exported.to_string()));
                            if exported != local {
                                renames
                                    .entry(exported.to_string())
                                    .or_default()
                                    .push(local.to_string());
                            }
                        }
                    }
                    ExportKind::NamedFrom(names, spec) => {
                        for (exported, local) in names {
                            // The parser tuple is (exported, local); for a
                            // re-export the "local" is the exported name and
                            // the "exported" is the imported name.
                            module.indirect_exports.push((
                                local.to_string(),
                                spec.to_string(),
                                exported.to_string(),
                            ));
                        }
                    }
                    ExportKind::Star(spec) => {
                        module.star_exports.push(spec.to_string());
                    }
                    ExportKind::StarAs(ns, spec) => {
                        module
                            .namespace_exports
                            .push((ns.to_string(), spec.to_string()));
                    }
                    ExportKind::Default(inner) => match inner.as_ref() {
                        Stmt::Function(f, _) => {
                            let mut f = (**f).clone();
                            if f.name.is_none() {
                                f.name = Some(Box::from("*default*"));
                            }
                            let name = f.name.clone().unwrap().to_string();
                            hoisted_functions.push(f);
                            export_map.push(("default".to_string(), name));
                        }
                        Stmt::Class(c, _) => {
                            let mut c = (**c).clone();
                            if c.name.is_none() {
                                c.name = Some(Box::from("*default*"));
                            }
                            let name = c.name.clone().unwrap().to_string();
                            hoisted_classes.push(c);
                            export_map.push(("default".to_string(), name));
                        }
                        // `export default <expr>` — evaluated in section 3 and
                        // synced into the env under the internal "*default*" key.
                        _ => {
                            export_map.push(("default".to_string(), "*default*".to_string()));
                        }
                    },
                    ExportKind::Decl(stmt) => match stmt.as_ref() {
                        Stmt::Function(f, _) => {
                            let name = f.name.clone().unwrap().to_string();
                            hoisted_functions.push((**f).clone());
                            export_map.push((name.clone(), name));
                        }
                        Stmt::Class(c, _) => {
                            let name = c.name.clone().unwrap().to_string();
                            hoisted_classes.push((**c).clone());
                            export_map.push((name.clone(), name));
                        }
                        Stmt::Var(kind, decls, _) => {
                            for d in decls {
                                if d.pattern.is_none() {
                                    hoisted_vars.push((d.name.to_string(), kind.clone()));
                                    export_map.push((d.name.to_string(), d.name.to_string()));
                                }
                            }
                        }
                        _ => {}
                    },
                },
                _ => {}
            }
        }
        // De-duplicate export_map (e.g. `export var x` + `export {x as y}`)
        let mut seen: Vec<String> = Vec::new();
        export_map.retain(|(exported, _)| {
            if seen.contains(exported) {
                false
            } else {
                seen.push(exported.clone());
                true
            }
        });
        module.local_exports = export_map;

        // ---- build import metadata + register namespace-import locals ----
        for imp in imports.iter() {
            if !imp.default_local.is_empty() {
                module.imports.push(ModuleImport {
                    specifier: imp.specifier.to_string(),
                    imported: "default".to_string(),
                    local: imp.default_local.to_string(),
                });
            }
            for (exported, local) in &imp.named {
                module.imports.push(ModuleImport {
                    specifier: imp.specifier.to_string(),
                    imported: exported.to_string(),
                    local: local.to_string(),
                });
            }
            if !imp.namespace_local.is_empty() {
                module.imports.push(ModuleImport {
                    specifier: imp.specifier.to_string(),
                    imported: "*ns*".to_string(),
                    local: imp.namespace_local.to_string(),
                });
            }
            if imp.default_local.is_empty()
                && imp.namespace_local.is_empty()
                && imp.named.is_empty()
            {
                module.imports.push(ModuleImport {
                    specifier: imp.specifier.to_string(),
                    imported: String::new(),
                    local: String::new(),
                });
            }
        }
        // Re-exported modules are dependencies too (§16.2.1.5 ModuleRequests
        // includes `export ... from` and `export * from` specifiers) — append
        // dependency-only entries so section 2 evaluates them in DFS order.
        let mut dep_specs: Vec<String> = Vec::new();
        for (_, spec, _) in &module.indirect_exports {
            dep_specs.push(spec.clone());
        }
        for spec in &module.star_exports {
            dep_specs.push(spec.clone());
        }
        for (_, spec) in &module.namespace_exports {
            dep_specs.push(spec.clone());
        }
        for spec in dep_specs {
            if !module.imports.iter().any(|i| i.specifier == spec) {
                module.imports.push(ModuleImport {
                    specifier: spec,
                    imported: String::new(),
                    local: String::new(),
                });
            }
        }
        // local binding name → import entry index
        let mut import_locals: HashMap<Box<str>, usize> = HashMap::new();
        for (idx, entry) in module.imports.iter().enumerate() {
            if entry.imported == "*ns*" {
                if !self.locals.contains(&entry.local) {
                    self.locals.push(entry.local.clone());
                }
            } else {
                import_locals.insert(entry.local.clone().into_boxed_str(), idx);
            }
        }
        self.module_imports = import_locals;
        // renames for ExportSync hooks
        for (stored, export_names) in &renames {
            if !stored.is_empty() {
                self.module_export_renames
                    .entry(stored.clone())
                    .or_default()
                    .extend(export_names.iter().cloned());
            }
        }

        // ---- section 1: hoisted bindings in source order ----
        for f in &hoisted_functions {
            let idx = self.compile_function(f);
            self.emit(Opcode::MakeFunction, vec![idx as i64]);
            if let Some(name) = &f.name {
                let name_idx = self.intern_string(name) as i64;
                self.emit(Opcode::StoreGlobal, vec![name_idx]);
                self.emit(Opcode::Pop, vec![]);
                self.emit_module_rename_sync(name);
            }
        }
        for c in &hoisted_classes {
            if let Some(name) = &c.name {
                let name_idx = self.intern_string(name) as i64;
                self.emit(Opcode::LoadUndefined, vec![]);
                self.emit(Opcode::StoreGlobal, vec![name_idx]);
                self.emit(Opcode::Pop, vec![]);
            }
        }
        for (name, kind) in &hoisted_vars {
            let name_idx = self.intern_string(name) as i64;
            // `var` bindings initialize to undefined at instantiation; `let`/
            // `const` start in the TDZ (ModuleTdz marks the env binding with a
            // sentinel — reads throw ReferenceError until section 3 runs the
            // initializer).
            if *kind == VarKind::Var {
                self.emit(Opcode::LoadUndefined, vec![]);
                self.emit(Opcode::StoreGlobal, vec![name_idx]);
                self.emit(Opcode::Pop, vec![]);
            } else {
                self.emit(Opcode::ModuleTdz, vec![name_idx]);
            }
        }

        // ---- section 2: import evaluation (DFS, cycle-safe) ----
        for (idx, _) in module.imports.iter().enumerate() {
            self.emit(Opcode::ImportModule, vec![idx as i64]);
            self.emit(Opcode::Pop, vec![]);
        }

        // ---- section 3: remaining statements ----
        for stmt in &prog.body {
            self.emit_module_statement(stmt);
        }

        self.emit(Opcode::LoadUndefined, vec![]);
        self.emit(Opcode::Return, vec![]);

        let mut program = self.into_bytecode();
        program.is_module = true;
        program.module = Some(module);
        program
    }

    /// Emit ExportSync hooks after a module-level store of a renamed export.
    fn emit_module_rename_sync(&mut self, stored: &str) {
        if !self.module_mode {
            return;
        }
        let export_names: Vec<String> = self
            .module_export_renames
            .get(stored)
            .cloned()
            .unwrap_or_default();
        for name in &export_names {
            let idx = self.intern_string(name) as i64;
            self.emit(Opcode::ExportSync, vec![idx]);
        }
    }

    /// Emit a top-level module statement (section 3). Declarations that were
    /// already handled by section 1 (functions, var/let/const declaration
    /// parts) emit only their initializer/assignment parts here.
    fn emit_module_statement(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Import(_, _) | Stmt::Function(_, _) => {}
            Stmt::Export(exp, _) => match &exp.kind {
                // `export default <expr>` — evaluate and sync into the env.
                ExportKind::Default(inner) => {
                    if let Stmt::Expr(e, _) = inner.as_ref() {
                        self.emit_expression(e);
                        let idx = self.intern_string("*default*") as i64;
                        self.emit(Opcode::ExportSync, vec![idx]);
                        self.emit(Opcode::Pop, vec![]);
                    }
                    // Default function/class decls were fully created in
                    // section 1 — nothing to do here.
                }
                // `export <var|let|const|class decl>` — emit the declaration
                // body (initializers/class evaluation); section 1 declared
                // the bindings. StoreGlobal keeps the env in sync.
                ExportKind::Decl(inner) => self.emit_module_statement(inner),
                _ => {}
            },
            Stmt::Class(class, _) => {
                self.emit_class(class, false);
                if let Some(name) = &class.name {
                    if let Some(idx) = self.local_index(name) {
                        self.emit(Opcode::LoadLocal, vec![idx as i64]);
                        let name_idx = self.intern_string(name) as i64;
                        self.emit(Opcode::ExportSync, vec![name_idx]);
                        self.emit(Opcode::Pop, vec![]);
                    }
                }
            }
            Stmt::Var(kind, decls, _) => {
                for decl in decls {
                    if let Some(pattern) = &decl.pattern {
                        if let Some(init) = &decl.init {
                            self.emit_expression(init);
                            self.emit_destructuring(pattern, &DestructureStore::Decl(VarKind::Var));
                        }
                    } else if let Some(init) = &decl.init {
                        self.emit_expression(init);
                        let name_idx = self.intern_string(&decl.name) as i64;
                        self.emit(Opcode::StoreGlobal, vec![name_idx]);
                        self.emit(Opcode::Pop, vec![]);
                        self.emit_module_rename_sync(&decl.name);
                    } else if *kind != VarKind::Var {
                        // Bare `let x;` — the declaration statement itself
                        // initializes the binding to undefined (module-level
                        // var already initialized in section 1).
                        let name_idx = self.intern_string(&decl.name) as i64;
                        self.emit(Opcode::LoadUndefined, vec![]);
                        self.emit(Opcode::StoreGlobal, vec![name_idx]);
                        self.emit(Opcode::Pop, vec![]);
                        self.emit_module_rename_sync(&decl.name);
                    }
                }
            }
            _ => self.emit_statement(stmt),
        }
    }

    /// Compile a function node into a nested BytecodeProgram and return its index.
    fn compile_function(&mut self, func: &FnNode) -> usize {
        let mut sub = Emitter::new();
        sub.env_scope_stack = self.env_scope_stack.clone();
        sub.private_field_names = self.private_field_names.clone();
        sub.module_mode = self.module_mode;
        sub.module_imports = self.module_imports.clone();
        sub.module_export_renames = self.module_export_renames.clone();
        sub.is_generator = func.is_generator;
        sub.is_async = func.is_async;
        let named_offset = if let Some(name) = &func.name {
            sub.named_function = true;
            sub.locals.push(name.to_string());
            1
        } else {
            0
        };
        // First pass: register all param locals (placeholders for patterns)
        for param in &func.params {
            match param {
                Pattern::Identifier(name, _, _) => sub.locals.push(name.to_string()),
                _ => sub.locals.push("_destructure".to_string()),
            }
        }
        // Register rest param and emit MakeRestArray BEFORE destructuring
        // so MakeRestArray reads the original overflow args (not yet overwritten)
        if let Some(rest_name) = &func.rest_param {
            sub.locals.push(rest_name.to_string());
            sub.emit(Opcode::MakeRestArray, vec![func.params.len() as i64]);
            if let Some(idx) = sub.local_index(rest_name) {
                sub.emit(Opcode::StoreLocal, vec![idx as i64]);
                sub.emit(Opcode::Pop, vec![]);
            }
        }
        // Second pass: emit destructuring/default code for regular params
        for (i, param) in func.params.iter().enumerate() {
            let param_idx = named_offset + i;
            match param {
                Pattern::Identifier(name, _, default) => {
                    if default.is_some() {
                        sub.emit(Opcode::LoadLocal, vec![param_idx as i64]);
                        sub.emit_store_with_default(
                            name.as_ref(),
                            &DestructureStore::Decl(VarKind::Var),
                            default,
                        );
                    }
                }
                _ => {
                    sub.emit(Opcode::LoadLocal, vec![param_idx as i64]);
                    sub.emit_destructuring_binding(param, &DestructureStore::Decl(VarKind::Var));
                }
            }
        }
        // For non-arrow functions: materialize `arguments` object only when referenced
        if !func.is_arrow && uses_arguments_stmt(&func.body) {
            sub.locals.push("arguments".to_string());
            sub.emit(Opcode::MakeArgumentsArray, vec![]);
            if let Some(idx) = sub.local_index("arguments") {
                sub.emit(Opcode::StoreLocal, vec![idx as i64]);
                sub.emit(Opcode::Pop, vec![]);
            }
        }
        // --- Escape analysis: does this function contain any inner function? ---
        // Pre-scan: collect all var declaration names so locals is complete before capture
        let mut all_var_names: Vec<String> = Vec::new();
        collect_var_names_stmt(&func.body, &mut all_var_names);
        for name in &all_var_names {
            if !sub.locals.contains(name) {
                sub.locals.push(name.clone());
            }
        }
        let has_inner = contains_inner_function_stmt(&func.body);
        if has_inner && !sub.locals.is_empty() {
            // Conservative approach: capture ALL local variables into the env
            sub.captured_names = sub.locals.clone();
            sub.captured_env_size = sub.locals.len();
            sub.emit(Opcode::MakeEnv, vec![sub.captured_env_size as i64]);
            // Copy each local's initial value from Frame.locals into the env slot.
            // StoreCaptured pops the value, so NO Pop after it.
            for i in 0..sub.captured_env_size {
                sub.emit(Opcode::LoadLocal, vec![i as i64]);
                sub.emit(Opcode::StoreCaptured, vec![0, i as i64]);
            }
            // Push captured_names onto env_scope_stack so inner functions can resolve
            sub.env_scope_stack.push(sub.captured_names.clone());
        }
        // Emit body: for arrow expression body (Stmt::Expr), use it as return value
        let is_arrow_expr = func.name.is_none() && matches!(&func.body, Stmt::Expr(..));
        match &func.body {
            Stmt::Expr(expr, _) if is_arrow_expr => {
                // Arrow expression body: emit expression then Return
                sub.emit_expression(expr);
                sub.emit(Opcode::Return, vec![]);
            }
            _ => {
                // Emit the body statement — for `Stmt::Block`, this goes through
                // the lexical scope setup in emit_statement (BlockEnter/BlockLeave).
                // For other body types, just emit them normally.
                sub.emit_statement(&func.body);
                // Add implicit undefined return if body doesn't end with Return
                let needs_return = match sub.instructions.last() {
                    Some(last) => last.opcode != Opcode::Return,
                    None => true,
                };
                if needs_return {
                    sub.emit(Opcode::LoadUndefined, vec![]);
                    sub.emit(Opcode::Return, vec![]);
                }
            }
        }
        let program = sub.into_bytecode();
        let idx = self.nested_funcs.len();
        self.nested_funcs.push(program);
        idx
    }

    /// Compile a function node into a nested BytecodeProgram (appended by the
    /// caller to a specific program's `functions` list — used when a MakeFunction
    /// will execute inside that program, e.g. private methods defined in a class
    /// constructor).
    fn compile_function_into(&mut self, func: &FnNode) -> BytecodeProgram {
        let mut sub = Emitter::new();
        sub.env_scope_stack = self.env_scope_stack.clone();
        sub.private_field_names = self.private_field_names.clone();
        sub.module_mode = self.module_mode;
        sub.module_imports = self.module_imports.clone();
        sub.module_export_renames = self.module_export_renames.clone();
        sub.is_generator = func.is_generator;
        sub.is_async = func.is_async;
        if let Some(name) = &func.name {
            sub.named_function = true;
            sub.locals.push(name.to_string());
        }
        for param in &func.params {
            match param {
                Pattern::Identifier(name, _, _) => sub.locals.push(name.to_string()),
                _ => sub.locals.push("_destructure".to_string()),
            }
        }
        if let Some(rest_name) = &func.rest_param {
            sub.locals.push(rest_name.to_string());
            sub.emit(Opcode::MakeRestArray, vec![func.params.len() as i64]);
            if let Some(idx) = sub.local_index(rest_name) {
                sub.emit(Opcode::StoreLocal, vec![idx as i64]);
                sub.emit(Opcode::Pop, vec![]);
            }
        }
        for (i, param) in func.params.iter().enumerate() {
            let param_idx = (if func.name.is_some() { 1 } else { 0 }) + i;
            match param {
                Pattern::Identifier(name, _, default) => {
                    if default.is_some() {
                        sub.emit(Opcode::LoadLocal, vec![param_idx as i64]);
                        sub.emit_store_with_default(
                            name.as_ref(),
                            &DestructureStore::Decl(VarKind::Var),
                            default,
                        );
                    }
                }
                _ => {
                    sub.emit(Opcode::LoadLocal, vec![param_idx as i64]);
                    sub.emit_destructuring_binding(param, &DestructureStore::Decl(VarKind::Var));
                }
            }
        }
        if !func.is_arrow && uses_arguments_stmt(&func.body) {
            sub.locals.push("arguments".to_string());
            sub.emit(Opcode::MakeArgumentsArray, vec![]);
            if let Some(idx) = sub.local_index("arguments") {
                sub.emit(Opcode::StoreLocal, vec![idx as i64]);
                sub.emit(Opcode::Pop, vec![]);
            }
        }
        let mut all_var_names: Vec<String> = Vec::new();
        collect_var_names_stmt(&func.body, &mut all_var_names);
        for name in &all_var_names {
            if !sub.locals.contains(name) {
                sub.locals.push(name.clone());
            }
        }
        let has_inner = contains_inner_function_stmt(&func.body);
        if has_inner && !sub.locals.is_empty() {
            sub.captured_names = sub.locals.clone();
            sub.captured_env_size = sub.locals.len();
            sub.emit(Opcode::MakeEnv, vec![sub.captured_env_size as i64]);
            for i in 0..sub.captured_env_size {
                sub.emit(Opcode::LoadLocal, vec![i as i64]);
                sub.emit(Opcode::StoreCaptured, vec![0, i as i64]);
            }
            sub.env_scope_stack.push(sub.captured_names.clone());
        }
        let is_arrow_expr = func.name.is_none() && matches!(&func.body, Stmt::Expr(..));
        match &func.body {
            Stmt::Expr(expr, _) if is_arrow_expr => {
                sub.emit_expression(expr);
                sub.emit(Opcode::Return, vec![]);
            }
            _ => {
                sub.emit_statement(&func.body);
                let needs_return = match sub.instructions.last() {
                    Some(last) => last.opcode != Opcode::Return,
                    None => true,
                };
                if needs_return {
                    sub.emit(Opcode::LoadUndefined, vec![]);
                    sub.emit(Opcode::Return, vec![]);
                }
            }
        }
        // The caller appends into the target program's functions list.
        sub.into_bytecode()
    }

    /// Emit bytecode for a class node.
    /// `for_expr`: true for class expressions (leaves constructor on stack), false for declarations (statement, no stack value).
    fn emit_class(&mut self, class: &ClassNode, for_expr: bool) {
        // 0. Save and set private element names (fields + private methods,
        //    deduplicated — getter/setter pairs share one slot) for method sub-emitters
        let _saved_private = self.private_field_names.clone();
        let mut names: Vec<String> = Vec::new();
        for pf in &class.private_fields {
            if !names.iter().any(|n| n == pf.name.as_ref()) {
                names.push(pf.name.to_string());
            }
        }
        self.private_field_names = names;

        // 0.5 Named classes: expose the class binding to ALL methods (static
        // and instance) through the lexical-env capture channel. The env scope
        // is pushed at COMPILE time so method bodies resolve the name via
        // LoadCaptured; the matching MakeEnv/StoreCaptured below fills it at
        // runtime before any method can be called.
        let mut class_env_pushed = false;
        if let Some(ref cname) = class.name {
            let already = self
                .env_scope_stack
                .iter()
                .any(|sc| sc.iter().any(|n| n.as_str() == cname.as_ref()));
            if !already {
                self.env_scope_stack.push(vec![cname.to_string()]);
                class_env_pushed = true;
                self.emit(Opcode::MakeEnv, vec![1]);
            }
        }

        // 1. Compile all methods, identify constructor
        let mut constructor_idx = None;
        let mut method_funcs: Vec<(PropKey, usize)> = Vec::new();
        let mut static_method_funcs: Vec<(PropKey, usize)> = Vec::new();
        let mut getter_funcs: Vec<(PropKey, usize)> = Vec::new();
        let mut setter_funcs: Vec<(PropKey, usize)> = Vec::new();
        let mut static_getter_funcs: Vec<(PropKey, usize)> = Vec::new();
        let mut static_setter_funcs: Vec<(PropKey, usize)> = Vec::new();

        for method in &class.methods {
            let is_constructor =
                matches!(&method.key, PropKey::Identifier(n) if n.as_ref() == "constructor");
            let func_name = if is_constructor {
                class.name.clone()
            } else {
                match &method.key {
                    PropKey::Identifier(n) => Some(n.clone()),
                    PropKey::String(n) => Some(n.clone()),
                    PropKey::Number(n) => Some(Box::from(n.to_string())),
                    PropKey::Computed(_) => None,
                }
            };

            let mut func = method.func.clone();
            func.name = func_name;
            let idx = self.compile_function(&func);
            if is_constructor {
                constructor_idx = Some(idx);
            } else if method.is_getter {
                if method.is_static {
                    static_getter_funcs.push((method.key.clone(), idx));
                } else {
                    getter_funcs.push((method.key.clone(), idx));
                }
            } else if method.is_setter {
                if method.is_static {
                    static_setter_funcs.push((method.key.clone(), idx));
                } else {
                    setter_funcs.push((method.key.clone(), idx));
                }
            } else if method.is_static {
                static_method_funcs.push((method.key.clone(), idx));
            } else {
                method_funcs.push((method.key.clone(), idx));
            }
        }

        // 1b. Compile static private method/accessor functions (parallel to
        //     private_fields). Instance private methods are compiled into the
        //     constructor's program at step 4.5 (their MakeFunction executes
        //     inside the ctor, which resolves indices against the ctor program).
        let mut private_funcs: Vec<Option<(usize, Option<usize>)>> =
            Vec::with_capacity(class.private_fields.len());
        for pf in &class.private_fields {
            if pf.is_static {
                if let Some(func) = &pf.func {
                    let mut f = (**func).clone();
                    f.name = Some(pf.name.clone());
                    let idx = self.compile_function(&f);
                    let second = pf.second_func.as_ref().map(|sf| {
                        let mut s = (**sf).clone();
                        s.name = Some(pf.name.clone());
                        self.compile_function(&s)
                    });
                    private_funcs.push(Some((idx, second)));
                } else {
                    private_funcs.push(None);
                }
            } else {
                private_funcs.push(None);
            }
        }

        let proto_key_idx = self.intern_string("prototype") as i64;
        let proto_proto_idx = self.intern_string("__proto__") as i64;

        // 2. Create empty prototype object
        self.emit(Opcode::NewObject, vec![0]);

        // 2.1 Emit PrivateNameScope if the class has private fields.
        //     This must happen BEFORE any MakeFunction calls so that sub-methods
        //     inherit the private name IDs from the class-evaluation function.
        let private_count = class.private_fields.len();
        if private_count > 0 {
            self.emit(Opcode::PrivateNameScope, vec![private_count as i64]);
        }

        // 2.5 Handle heritage (extends)
        let mut heritage_super_slot = None;
        if let Some(heritage) = &class.heritage {
            let pslot = self.locals.len();
            self.locals.push(format!("__ext_proto_{}", pslot));
            let sslot = self.locals.len();
            self.locals.push(format!("__ext_super_{}", sslot));
            heritage_super_slot = Some(sslot);

            // Save child proto to local (StoreLocal keeps value on stack too)
            self.emit(Opcode::StoreLocal, vec![pslot as i64]);

            // Evaluate heritage expression → pushes superclass constructor
            self.emit_expression(heritage);

            // Save superclass constructor (value stays on stack AND saved in local)
            self.emit(Opcode::StoreLocal, vec![sslot as i64]);

            // Load super.prototype (consumes the superclass copy on stack, but saved in sslot)
            self.emit(Opcode::LoadStringConst, vec![proto_key_idx]);
            self.emit(Opcode::LoadProperty, vec![]);

            // Set child_proto.__proto__ = super.prototype
            // Stack: [child_proto, super_prototype]
            self.emit(Opcode::LoadStringConst, vec![proto_proto_idx]);
            // Stack: [child_proto, super_prototype, "__proto__"]
            self.emit(Opcode::Swap, vec![]);
            // Stack: [child_proto, "__proto__", super_prototype]
            self.emit(Opcode::StoreProperty, vec![]);
            self.emit(Opcode::Pop, vec![]);

            // Restore child proto onto stack for method definitions
            self.emit(Opcode::LoadLocal, vec![pslot as i64]);
        }

        // 3. Add non-constructor, non-static methods to prototype
        for (key, func_idx) in &method_funcs {
            if let PropKey::Computed(expr) = key {
                // Computed key: [proto, key] then MakeFunction → [proto, key, fn]
                self.emit_expression(expr);
                self.emit(Opcode::MakeFunction, vec![*func_idx as i64]);
                self.emit(Opcode::DefineProperty, vec![usize::MAX as i64]);
                continue;
            }
            self.emit(Opcode::MakeFunction, vec![*func_idx as i64]);
            let key_str = match key {
                PropKey::String(s) => s.to_string(),
                PropKey::Identifier(s) => s.to_string(),
                PropKey::Number(n) => n.to_string(),
                PropKey::Computed(_) => unreachable!(),
            };
            let key_idx = self.intern_string(&key_str) as i64;
            self.emit(Opcode::DefineProperty, vec![key_idx]);
        }

        // 3a. Add getter/setter accessors to prototype
        {
            let mut all_acc_names: Vec<PropKey> = Vec::new();
            for (key, _) in &getter_funcs {
                if !all_acc_names.contains(key) {
                    all_acc_names.push(key.clone());
                }
            }
            for (key, _) in &setter_funcs {
                if !all_acc_names.contains(key) {
                    all_acc_names.push(key.clone());
                }
            }
            for key in &all_acc_names {
                let has_getter = getter_funcs.iter().any(|(k, _)| k == key);
                let has_setter = setter_funcs.iter().any(|(k, _)| k == key);
                if let PropKey::Computed(expr) = key {
                    // Computed accessor: [proto, key] then getter+setter funcs
                    self.emit_expression(expr);
                }
                if has_getter {
                    let gi = getter_funcs.iter().find(|(k, _)| k == key).unwrap().1;
                    self.emit(Opcode::MakeFunction, vec![gi as i64]);
                } else {
                    self.emit(Opcode::LoadUndefined, vec![]);
                }
                if has_setter {
                    let si = setter_funcs.iter().find(|(k, _)| k == key).unwrap().1;
                    self.emit(Opcode::MakeFunction, vec![si as i64]);
                } else {
                    self.emit(Opcode::LoadUndefined, vec![]);
                }
                let key_str = match key {
                    PropKey::String(s) => s.to_string(),
                    PropKey::Identifier(s) => s.to_string(),
                    PropKey::Number(n) => n.to_string(),
                    PropKey::Computed(_) => {
                        self.emit(Opcode::DefineAccessor, vec![usize::MAX as i64]);
                        continue;
                    }
                };
                let key_idx = self.intern_string(&key_str) as i64;
                self.emit(Opcode::DefineAccessor, vec![key_idx]);
            }
        }

        // 4. Create constructor function
        let has_heritage = class.heritage.is_some();
        let ctor_idx = constructor_idx.unwrap_or_else(|| {
            let body = if has_heritage {
                // Derived class default constructor: constructor(...args) { super(...args); }
                Stmt::Block(
                    vec![Stmt::Expr(
                        Expr::Call(
                            Box::new(Expr::Super(Span { start: 0, end: 0 })),
                            vec![ArrayElement {
                                expr: Expr::Identifier(
                                    Box::from("args"),
                                    Span { start: 0, end: 0 },
                                ),
                                is_spread: true,
                                span: Span { start: 0, end: 0 },
                            }],
                            Span { start: 0, end: 0 },
                        ),
                        Span { start: 0, end: 0 },
                    )],
                    Span { start: 0, end: 0 },
                )
            } else {
                // Base class default constructor: empty body
                Stmt::Block(vec![], Span { start: 0, end: 0 })
            };
            let synth = FnNode {
                name: class.name.clone(),
                params: vec![],
                rest_param: if has_heritage {
                    Some(Box::from("args"))
                } else {
                    None
                },
                body,
                is_generator: false,
                is_async: false,
                is_arrow: false,
                span: Span { start: 0, end: 0 },
            };
            self.compile_function(&synth)
        });
        self.emit(Opcode::MakeFunction, vec![ctor_idx as i64]);

        // 4.5 Inject private element initialization into the constructor body.
        //     Instance private fields/methods are defined on `this` before the
        //     constructor body runs (InitializeInstanceElements, §7.3.33):
        //       field:   LoadThis; <init_expr>|undefined; DefinePrivateField slot
        //       method:  LoadThis; MakeFunction idx; DefinePrivateField slot
        //       accessor: LoadThis; <getter>|undefined; <setter>|undefined;
        //                MakeAccessorPair; DefinePrivateField slot
        if !class.private_fields.is_empty() {
            // Compile instance private method/accessor functions INTO the ctor
            // program (the MakeFunction emitted below executes inside the ctor,
            // where func indices resolve against the ctor program).
            let mut ctor_private_funcs: Vec<Option<(usize, Option<usize>)>> =
                Vec::with_capacity(class.private_fields.len());
            for pf in &class.private_fields {
                if !pf.is_static {
                    if let Some(func) = &pf.func {
                        let mut f = (**func).clone();
                        f.name = Some(pf.name.clone());
                        let program = self.compile_function_into(&f);
                        let ctor_prog = &mut self.nested_funcs[ctor_idx];
                        let idx = ctor_prog.functions.len();
                        ctor_prog.functions.push(program);
                        let second = pf.second_func.as_ref().map(|sf| {
                            let mut s = (**sf).clone();
                            s.name = Some(pf.name.clone());
                            let program = self.compile_function_into(&s);
                            let ctor_prog = &mut self.nested_funcs[ctor_idx];
                            let idx = ctor_prog.functions.len();
                            ctor_prog.functions.push(program);
                            idx
                        });
                        ctor_private_funcs.push(Some((idx, second)));
                    } else {
                        ctor_private_funcs.push(None);
                    }
                } else {
                    ctor_private_funcs.push(None);
                }
            }
            // Find the last Return instruction
            let ctor_prog = &mut self.nested_funcs[ctor_idx];
            let return_pos = ctor_prog
                .instructions
                .iter()
                .rposition(|i| i.opcode == Opcode::Return);
            let mut inject = Vec::new();
            for (slot, field) in class.private_fields.iter().enumerate() {
                if field.is_static {
                    continue;
                }
                inject.push(Instruction::new(Opcode::LoadThis, vec![]));
                if let Some((func_idx, second_idx)) = ctor_private_funcs[slot] {
                    if field.is_getter || field.is_setter {
                        // Private accessor: func = getter, second_func = setter.
                        if field.is_getter {
                            inject.push(Instruction::new(
                                Opcode::MakeFunction,
                                vec![func_idx as i64],
                            ));
                        } else {
                            inject.push(Instruction::new(Opcode::LoadUndefined, vec![]));
                        }
                        if field.is_setter {
                            inject.push(Instruction::new(
                                Opcode::MakeFunction,
                                vec![second_idx.unwrap() as i64],
                            ));
                        } else {
                            inject.push(Instruction::new(Opcode::LoadUndefined, vec![]));
                        }
                        inject.push(Instruction::new(Opcode::MakeAccessorPair, vec![]));
                    } else {
                        inject.push(Instruction::new(
                            Opcode::MakeFunction,
                            vec![func_idx as i64],
                        ));
                    }
                } else if let Some(init) = &field.init {
                    // Emit the initializer expression
                    let mut sub = Emitter::new();
                    sub.private_field_names = self.private_field_names.clone();
                    sub.emit_expression(init);
                    inject.extend(sub.instructions);
                } else {
                    inject.push(Instruction::new(Opcode::LoadUndefined, vec![]));
                }
                inject.push(Instruction::new(
                    Opcode::DefinePrivateField,
                    vec![slot as i64],
                ));
            }
            if let Some(pos) = return_pos {
                // Insert injected instructions before Return
                for (i, instr) in inject.into_iter().enumerate() {
                    ctor_prog.instructions.insert(pos + i, instr);
                }
            } else {
                // No Return found; append at end
                ctor_prog.instructions.extend(inject);
                ctor_prog
                    .instructions
                    .push(Instruction::new(Opcode::LoadUndefined, vec![]));
                ctor_prog
                    .instructions
                    .push(Instruction::new(Opcode::Return, vec![]));
            }
        }

        // 5. Save constructor to a local slot so it can be restored after
        //    StoreProperty (which consumes it as the obj argument and pushes
        //    the value back instead). Named classes use the class-name local.
        //    Anonymous class expressions use a temp local slot.
        let save_slot: Option<usize> = if let Some(ref name) = class.name {
            if !self.locals.contains(&name.to_string()) {
                self.locals.push(name.to_string());
            }
            self.local_index(name)
        } else if for_expr {
            let temp = format!("__cc_{}", self.locals.len());
            self.locals.push(temp);
            self.local_index(&self.locals[self.locals.len() - 1])
        } else {
            // anonymous declaration (spec-invalid but handled)
            None
        };
        if let Some(idx) = save_slot {
            self.emit(Opcode::StoreLocal, vec![idx as i64]);
        }
        // 5-pre. Bind the class name inside the capture env (slot filled any
        // time before the first method call — closures hold the env object).
        if class_env_pushed {
            if let Some(ctor_slot) = save_slot {
                self.emit(Opcode::LoadLocal, vec![ctor_slot as i64]);
                self.emit(Opcode::StoreCaptured, vec![0i64, 0i64]);
            }
        }

        // 5a. Set superclass on the constructor for super() calls in derived classes
        if let (Some(sslot), Some(ctor_slot)) = (heritage_super_slot, save_slot) {
            self.emit(Opcode::LoadLocal, vec![sslot as i64]);
            self.emit(Opcode::SetSuperclass, vec![]);
            // ctor was consumed by SetSuperclass, restore from saved slot
            self.emit(Opcode::LoadLocal, vec![ctor_slot as i64]);
        }

        // 6. Link: Constructor.prototype = Proto
        //    Stack: [..., proto, ctor]
        //    StoreProperty pops: obj=ctor, key="prototype", value=proto
        //    and pushes value (proto) back.
        self.emit(Opcode::Swap, vec![]);
        self.emit(Opcode::LoadStringConst, vec![proto_key_idx]);
        self.emit(Opcode::Swap, vec![]);
        self.emit(Opcode::StoreProperty, vec![]);
        self.emit(Opcode::Pop, vec![]);

        // 7. Link: Constructor.__proto__ = SuperClass (for extends, static inheritance)
        if let Some(sslot) = heritage_super_slot {
            if let Some(ctor_slot) = save_slot {
                self.emit(Opcode::LoadLocal, vec![ctor_slot as i64]);
                self.emit(Opcode::LoadStringConst, vec![proto_proto_idx]);
                self.emit(Opcode::LoadLocal, vec![sslot as i64]);
                self.emit(Opcode::StoreProperty, vec![]);
                self.emit(Opcode::Pop, vec![]);
            }
        }

        // 7.5 Add static methods to constructor
        if let Some(ctor_slot) = save_slot {
            for (key, func_idx) in &static_method_funcs {
                self.emit(Opcode::LoadLocal, vec![ctor_slot as i64]);
                if let PropKey::Computed(expr) = key {
                    self.emit_expression(expr);
                    self.emit(Opcode::MakeFunction, vec![*func_idx as i64]);
                    self.emit(Opcode::DefineProperty, vec![usize::MAX as i64]);
                    self.emit(Opcode::Pop, vec![]);
                    continue;
                }
                self.emit(Opcode::MakeFunction, vec![*func_idx as i64]);
                let key_str = match key {
                    PropKey::String(s) => s.to_string(),
                    PropKey::Identifier(s) => s.to_string(),
                    PropKey::Number(n) => n.to_string(),
                    PropKey::Computed(_) => unreachable!(),
                };
                let key_idx = self.intern_string(&key_str) as i64;
                self.emit(Opcode::DefineProperty, vec![key_idx]);
                self.emit(Opcode::Pop, vec![]);
            }
            // 7.6 Add static getter/setter accessors to constructor
            {
                let mut all_acc_names: Vec<PropKey> = Vec::new();
                for (key, _) in &static_getter_funcs {
                    if !all_acc_names.contains(key) {
                        all_acc_names.push(key.clone());
                    }
                }
                for (key, _) in &static_setter_funcs {
                    if !all_acc_names.contains(key) {
                        all_acc_names.push(key.clone());
                    }
                }
                for key in &all_acc_names {
                    self.emit(Opcode::LoadLocal, vec![ctor_slot as i64]);
                    let has_getter = static_getter_funcs.iter().any(|(k, _)| k == key);
                    let has_setter = static_setter_funcs.iter().any(|(k, _)| k == key);
                    if let PropKey::Computed(expr) = key {
                        self.emit_expression(expr);
                    }
                    if has_getter {
                        let gi = static_getter_funcs
                            .iter()
                            .find(|(k, _)| k == key)
                            .unwrap()
                            .1;
                        self.emit(Opcode::MakeFunction, vec![gi as i64]);
                    } else {
                        self.emit(Opcode::LoadUndefined, vec![]);
                    }
                    if has_setter {
                        let si = static_setter_funcs
                            .iter()
                            .find(|(k, _)| k == key)
                            .unwrap()
                            .1;
                        self.emit(Opcode::MakeFunction, vec![si as i64]);
                    } else {
                        self.emit(Opcode::LoadUndefined, vec![]);
                    }
                    let key_str = match key {
                        PropKey::String(s) => s.to_string(),
                        PropKey::Identifier(s) => s.to_string(),
                        PropKey::Number(n) => n.to_string(),
                        PropKey::Computed(_) => {
                            self.emit(Opcode::DefineAccessor, vec![usize::MAX as i64]);
                            self.emit(Opcode::Pop, vec![]);
                            continue;
                        }
                    };
                    let key_idx = self.intern_string(&key_str) as i64;
                    self.emit(Opcode::DefineAccessor, vec![key_idx]);
                    self.emit(Opcode::Pop, vec![]);
                }
            }
        }

        // 7.7 Add static private methods/accessors to constructor (§15.7.14 step 31:
        //     PrivateMethodOrAccessorAdd on the constructor, BEFORE static fields)
        if let Some(ctor_slot) = save_slot {
            for (slot, field) in class.private_fields.iter().enumerate() {
                if !field.is_static {
                    continue;
                }
                if let Some((func_idx, second_idx)) = private_funcs[slot] {
                    self.emit(Opcode::LoadLocal, vec![ctor_slot as i64]);
                    if field.is_getter || field.is_setter {
                        // func = getter, second_func = setter (see parser merge).
                        if field.is_getter {
                            self.emit(Opcode::MakeFunction, vec![func_idx as i64]);
                        } else {
                            self.emit(Opcode::LoadUndefined, vec![]);
                        }
                        if field.is_setter {
                            self.emit(Opcode::MakeFunction, vec![second_idx.unwrap() as i64]);
                        } else {
                            self.emit(Opcode::LoadUndefined, vec![]);
                        }
                        self.emit(Opcode::MakeAccessorPair, vec![]);
                    } else {
                        self.emit(Opcode::MakeFunction, vec![func_idx as i64]);
                    }
                    self.emit(Opcode::DefinePrivateField, vec![slot as i64]);
                    self.emit(Opcode::Pop, vec![]);
                }
            }
            // 7.8 Static private fields: DefineField(ctor, record) — the initializer
            //     runs as a zero-arg function with `this` = the constructor (§15.7.14
            //     step 32, §15.7.10 initializer wrapping, DefineField §7.3.32).
            for (slot, field) in class.private_fields.iter().enumerate() {
                if !field.is_static || field.func.is_some() {
                    continue;
                }
                if let Some(init) = &field.init {
                    let synth = FnNode {
                        name: None,
                        params: vec![],
                        rest_param: None,
                        body: Stmt::Expr((**init).clone(), Span { start: 0, end: 0 }),
                        is_generator: false,
                        is_async: false,
                        is_arrow: false,
                        span: Span { start: 0, end: 0 },
                    };
                    let wrapper_idx = self.compile_function(&synth);
                    // [ctor] → [ctor, ctor] → [ctor, ctor, wrapper] → Call pops
                    // [wrapper, ctor] leaving [ctor, result] for DefinePrivateField.
                    self.emit(Opcode::LoadLocal, vec![ctor_slot as i64]);
                    self.emit(Opcode::Dup, vec![]);
                    self.emit(Opcode::MakeFunction, vec![wrapper_idx as i64]);
                    self.emit(Opcode::Call, vec![0]);
                    self.emit(Opcode::DefinePrivateField, vec![slot as i64]);
                    self.emit(Opcode::Pop, vec![]);
                } else {
                    self.emit(Opcode::LoadLocal, vec![ctor_slot as i64]);
                    self.emit(Opcode::LoadUndefined, vec![]);
                    self.emit(Opcode::DefinePrivateField, vec![slot as i64]);
                    self.emit(Opcode::Pop, vec![]);
                }
            }
        }

        // 8. For expressions: restore constructor onto stack from the saved slot.
        if for_expr {
            if let Some(idx) = save_slot {
                self.emit(Opcode::LoadLocal, vec![idx as i64]);
            }
        }

        // 9. Restore parent's private field names (may be empty for top-level)
        self.private_field_names = _saved_private;

        // 10. Pop the class-name capture env (pushed at 0.5). Every method
        // closure already holds the env object, so restoring the frame's env
        // here cannot detach them.
        if class_env_pushed {
            self.emit(Opcode::RestoreEnv, vec![]);
            self.env_scope_stack.pop();
        }
    }

    /// Emit bytecode for destructuring a value according to the pattern.
    fn emit_destructuring(&mut self, pattern: &Pattern, kind: &DestructureStore) {
        // §14.5.1 step 4: throw TypeError if value is null or undefined
        self.emit(Opcode::ThrowIfNullish, vec![]);
        match pattern {
            Pattern::Object(props, rest, _) => {
                for prop in props {
                    self.emit(Opcode::Dup, vec![]);
                    match &prop.key {
                        PropKey::Identifier(id) => {
                            let idx = self.intern_string(id);
                            self.emit(Opcode::LoadStringConst, vec![idx as i64]);
                        }
                        PropKey::String(s) => {
                            let idx = self.intern_string(s);
                            self.emit(Opcode::LoadStringConst, vec![idx as i64]);
                        }
                        PropKey::Number(n) => {
                            let s = n.to_string();
                            let idx = self.intern_string(&s);
                            self.emit(Opcode::LoadStringConst, vec![idx as i64]);
                        }
                        PropKey::Computed(expr) => {
                            self.emit_expression(expr);
                        }
                    }
                    self.emit(Opcode::LoadProperty, vec![]);
                    self.emit_destructuring_binding(&prop.pattern, kind);
                }
                if let Some(rest_pattern) = rest {
                    // rest = copy of source minus already-destructured keys
                    // Dup so SpreadIntoObject can consume the copy while
                    // the original stays on stack for the final Pop
                    self.emit(Opcode::Dup, vec![]);
                    self.emit(Opcode::NewObject, vec![0]);
                    self.emit(Opcode::Swap, vec![]);
                    self.emit(Opcode::SpreadIntoObject, vec![]);
                    for prop in props {
                        self.emit(Opcode::Dup, vec![]);
                        let key_str = match &prop.key {
                            PropKey::Identifier(s) => s.to_string(),
                            PropKey::String(s) => s.to_string(),
                            PropKey::Number(n) => n.to_string(),
                            PropKey::Computed(_) => continue,
                        };
                        let idx = self.intern_string(&key_str);
                        self.emit(Opcode::LoadStringConst, vec![idx as i64]);
                        self.emit(Opcode::DeleteProperty, vec![]);
                        self.emit(Opcode::Pop, vec![]);
                    }
                    self.emit_destructuring_binding(rest_pattern, kind);
                }
                self.emit(Opcode::Pop, vec![]);
            }
            Pattern::Array(items, _) => {
                for (i, item) in items.iter().enumerate() {
                    self.emit(Opcode::Dup, vec![]);
                    if let Some(pattern) = item {
                        if matches!(pattern, Pattern::Rest(..)) {
                            self.emit(Opcode::LoadSmi, vec![i as i64]);
                            self.emit(Opcode::ArraySlice, vec![]);
                        } else {
                            self.emit(Opcode::LoadSmi, vec![i as i64]);
                            self.emit(Opcode::LoadProperty, vec![]);
                        }
                        self.emit_destructuring_binding(pattern, kind);
                    }
                }
                self.emit(Opcode::Pop, vec![]);
            }
            Pattern::Identifier(name, _, default) => {
                self.emit_store_with_default(name, kind, default);
            }
            Pattern::Default(_, _) => {
                unreachable!("Pattern::Default should be handled by emit_destructuring_binding");
            }
            Pattern::Rest(inner, _) => {
                self.emit_destructuring_binding(inner, kind);
            }
        }
    }

    /// Emit a store operation for a single binding in a destructuring pattern.
    /// Recurses into nested patterns.
    fn emit_destructuring_binding(&mut self, pattern: &Pattern, kind: &DestructureStore) {
        match pattern {
            Pattern::Identifier(name, _, default) => {
                self.emit_store_with_default(name, kind, default);
            }
            Pattern::Default(inner, expr) => {
                // Check if the value is undefined; if so, replace with default expr
                self.emit(Opcode::Dup, vec![]);
                self.emit(Opcode::LoadUndefined, vec![]);
                self.emit(Opcode::StrictEq, vec![]);
                self.emit(Opcode::JumpIfFalse, vec![0]);
                let jump_pos = self.current() - 1;
                self.emit(Opcode::Pop, vec![]);
                self.emit_expression(expr);
                self.instructions[jump_pos].operands[0] = self.current() as i64;
                // Recurse with the (possibly defaulted) value
                self.emit_destructuring_binding(inner, kind);
            }
            Pattern::Object(_, _, _) | Pattern::Array(_, _) => {
                self.emit_destructuring(pattern, kind);
            }
            Pattern::Rest(inner, _) => {
                self.emit_destructuring_binding(inner, kind);
            }
        }
    }

    /// Store a value to a binding (var → StoreLocal+Pop, let/const → DeclareLet/DeclareConst).
    /// With an optional default: if the value is undefined, evaluate the default instead.
    fn emit_store_with_default(
        &mut self,
        name: &str,
        kind: &DestructureStore,
        default: &Option<Box<Expr>>,
    ) {
        if let Some(expr) = default {
            self.emit(Opcode::Dup, vec![]);
            self.emit(Opcode::LoadUndefined, vec![]);
            self.emit(Opcode::StrictEq, vec![]);
            self.emit(Opcode::JumpIfFalse, vec![0]);
            let jump_pos = self.current() - 1;
            self.emit(Opcode::Pop, vec![]);
            self.emit_expression(expr);
            self.instructions[jump_pos].operands[0] = self.current() as i64;
        }
        self.emit_store_binding(name, kind);
    }

    /// Store a value to a binding (var → StoreLocal/StoreCaptured+Pop, let/const → DeclareLet/DeclareConst).
    fn emit_store_binding(&mut self, name: &str, kind: &DestructureStore) {
        match kind {
            DestructureStore::Assign => {
                self.emit_assign_store(name);
            }
            DestructureStore::Decl(VarKind::Var) => {
                let is_top_level =
                    self.env_scope_stack.is_empty() && self.captured_names.is_empty();
                if !is_top_level && !self.locals.contains(&name.to_string()) {
                    self.locals.push(name.to_string());
                }
                if let Some((depth, env_slot)) = self.env_captured_slot(name) {
                    // StoreCaptured pops the value — no Pop needed
                    self.emit(Opcode::StoreCaptured, vec![depth as i64, env_slot as i64]);
                } else if let Some(idx) = self.local_index(name) {
                    self.emit(Opcode::StoreLocal, vec![idx as i64]);
                    self.emit(Opcode::Pop, vec![]);
                } else if is_top_level {
                    let name_idx = self.intern_string(name) as i64;
                    self.emit(Opcode::StoreGlobal, vec![name_idx]);
                    self.emit(Opcode::Pop, vec![]);
                    self.emit_module_rename_sync(name);
                }
            }
            DestructureStore::Decl(VarKind::Let | VarKind::Const) => {
                if let Some(slot) = self.lexical_slot(name) {
                    let op = if matches!(kind, DestructureStore::Decl(VarKind::Const)) {
                        Opcode::DeclareConst
                    } else {
                        Opcode::DeclareLet
                    };
                    self.emit(op, vec![slot as i64]);
                }
            }
        }
    }

    fn emit_statement(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Expr(expr, _) => {
                self.emit_expression(expr);
                self.emit(Opcode::Pop, vec![]);
            }
            Stmt::Import(_, _) | Stmt::Export(_, _) => {
                self.emit(Opcode::LoadUndefined, vec![]);
                self.emit(Opcode::Pop, vec![]);
            }
            Stmt::Block(stmts, _) => {
                let lexical_count = self.count_lexicals(stmts);
                if lexical_count > 0 {
                    self.enter_lexical_scope(stmts, lexical_count);
                    self.emit(Opcode::BlockEnter, vec![lexical_count as i64]);
                }
                // Block-scope env for closure capture
                let saved_env_depth = self.env_scope_stack.len();
                if lexical_count > 0 && stmts.iter().any(contains_inner_function_stmt) {
                    // Create a block env so inner functions can capture lexical bindings
                    let block_names: Vec<String> = stmts
                        .iter()
                        .filter_map(|s| match s {
                            Stmt::Var(VarKind::Let | VarKind::Const, decls, _) => Some(decls),
                            _ => None,
                        })
                        .flatten()
                        .filter_map(|d| {
                            if d.pattern.is_none() {
                                Some(d.name.to_string())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !block_names.is_empty() {
                        let name_count = block_names.len();
                        self.env_scope_stack.push(block_names);
                        self.emit(Opcode::MakeEnv, vec![name_count as i64]);
                    }
                }
                for s in stmts {
                    self.emit_statement(s);
                }
                // Restore block-scope env
                if self.env_scope_stack.len() > saved_env_depth {
                    self.emit(Opcode::RestoreEnv, vec![]);
                    self.env_scope_stack.truncate(saved_env_depth);
                }
                if lexical_count > 0 {
                    self.emit(Opcode::BlockLeave, vec![]);
                    self.leave_lexical_scope();
                }
            }
            Stmt::If(cond, then, else_, _) => {
                self.emit_expression(cond);
                let else_jump = self.current();
                self.emit(Opcode::JumpIfFalse, vec![0]);
                self.emit_statement(then);
                if let Some(el) = else_ {
                    let exit_jump = self.current();
                    self.emit(Opcode::Jump, vec![0]);
                    self.patch(else_jump, self.current());
                    self.emit_statement(el);
                    self.patch(exit_jump, self.current());
                } else {
                    self.patch(else_jump, self.current());
                }
            }
            Stmt::While(cond, body, _) => {
                let loop_start = self.current();
                self.emit_expression(cond);
                let exit_jump = self.current();
                self.emit(Opcode::JumpIfFalse, vec![0]);
                // Sentinels: `break` jumps to the exit target, not to the
                // JumpIfFalse instruction (which would resume mid-condition
                // with a corrupted stack).
                self.loop_exit_stack.push(usize::MAX);
                self.loop_cont_stack.push(loop_start);
                self.pending_loop_jumps.push(Vec::new());
                self.emit_statement(body);
                self.loop_cont_stack.pop();
                self.loop_exit_stack.pop();
                self.emit(Opcode::Jump, vec![loop_start as i64]);
                let exit = self.current();
                self.patch(exit_jump, exit);
                for (pos, kind) in self.pending_loop_jumps.pop().unwrap_or_default() {
                    let target = match kind {
                        LoopJumpKind::Break => exit,
                        LoopJumpKind::Continue => loop_start,
                    };
                    self.patch(pos, target);
                }
            }
            Stmt::DoWhile(cond, body, _) => {
                let loop_start = self.current();
                // Sentinels: `break`/`continue` in a do-while emit patchable
                // Jumps instead of jumping to a placeholder (a placeholder at
                // the loop top would execute on first entry and skip the body;
                // the cond position is not known until after the body).
                self.loop_exit_stack.push(usize::MAX);
                self.loop_cont_stack.push(usize::MAX);
                self.pending_loop_jumps.push(Vec::new());
                self.emit_statement(body);
                self.loop_exit_stack.pop();
                self.loop_cont_stack.pop();
                // `continue` re-checks the condition.
                let cond_pos = self.current();
                self.emit_expression(cond);
                self.emit(Opcode::JumpIfTrue, vec![loop_start as i64]);
                let exit = self.current();
                for (pos, kind) in self.pending_loop_jumps.pop().unwrap_or_default() {
                    let target = match kind {
                        LoopJumpKind::Break => exit,
                        LoopJumpKind::Continue => cond_pos,
                    };
                    self.patch(pos, target);
                }
            }
            Stmt::For(init, cond, update, body, _) => {
                // Enter lexical scope for for-init's let/const declarations
                let init_has_scope = if let Some(init_stmt) = init {
                    let init_ref: &Stmt = init_stmt.as_ref();
                    if matches!(init_ref, Stmt::Var(VarKind::Let | VarKind::Const, _, _)) {
                        let count = self.count_lexicals(std::slice::from_ref(init_ref));
                        if count > 0 {
                            self.enter_lexical_scope(std::slice::from_ref(init_ref), count);
                            self.emit(Opcode::BlockEnter, vec![count as i64]);
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };
                // Emit init
                if let Some(init_stmt) = init {
                    self.emit_statement(init_stmt);
                }
                // Collect per-iteration let variable info
                let mut per_iteration_vars: Vec<(String, usize)> = Vec::new();
                if init_has_scope {
                    if let Some(scope) = self.lexical_scopes.last() {
                        for b in scope {
                            per_iteration_vars.push((b.name.clone(), b.slot));
                        }
                    }
                }
                let per_iteration_count = per_iteration_vars.len();
                let shadow_start_slot = self.lexical_slot_count;
                let loop_start = self.current();
                // Create per-iteration shadow scope (copy outer → inner)
                if per_iteration_count > 0 {
                    self.emit(Opcode::BlockEnter, vec![per_iteration_count as i64]);
                    let mut shadow_bindings = Vec::new();
                    for (i, (name, outer_slot)) in per_iteration_vars.iter().enumerate() {
                        let inner_slot = shadow_start_slot + i;
                        self.emit(
                            Opcode::CopyLexical,
                            vec![*outer_slot as i64, inner_slot as i64],
                        );
                        shadow_bindings.push(LexicalBinding {
                            name: name.clone(),
                            slot: inner_slot,
                        });
                    }
                    self.lexical_slot_count += per_iteration_count;
                    self.lexical_scopes.push(shadow_bindings);
                }
                // ── Per-iteration env for closure capture ──
                // Create a child env per iteration so closures capture the
                // per-iteration binding value (e.g., each iteration's `i`).
                let saved_env_depth = self.env_scope_stack.len();
                if per_iteration_count > 0 {
                    let per_iter_names: Vec<String> =
                        per_iteration_vars.iter().map(|(n, _)| n.clone()).collect();
                    self.env_scope_stack.push(per_iter_names);
                    self.emit(Opcode::MakeEnv, vec![per_iteration_count as i64]);
                    for (i, (_, inner_slot)) in per_iteration_vars.iter().enumerate() {
                        self.emit(Opcode::LoadLexical, vec![*inner_slot as i64]);
                        self.emit(Opcode::StoreCaptured, vec![0, i as i64]);
                    }
                }
                let exit_jump = if let Some(c) = cond {
                    self.emit_expression(c);
                    let j = self.current();
                    self.emit(Opcode::JumpIfFalse, vec![0]);
                    j
                } else {
                    self.current()
                };
                // Empty-cond loops (`for(;;)`) have NO JumpIfFalse to break
                // to — a concrete position here would point at the body start
                // (infinite loop). Use the sentinel so breaks are patched to
                // the true exit after the body is emitted.
                self.loop_exit_stack.push(if cond.is_some() {
                    exit_jump
                } else {
                    usize::MAX
                });
                // `continue` must jump to the update expression, not the
                // condition (which would skip the update and loop forever).
                // The update is emitted after the body, so use a sentinel and
                // patch the continues to the update tail.
                self.loop_cont_stack.push(usize::MAX);
                self.pending_loop_jumps.push(Vec::new());
                self.emit_statement(body);
                self.loop_cont_stack.pop();
                self.loop_exit_stack.pop();
                let continue_target = self.current();
                // ── Restore env after body ──
                if per_iteration_count > 0 {
                    self.emit(Opcode::RestoreEnv, vec![]);
                    self.env_scope_stack.truncate(saved_env_depth);
                }
                if let Some(upd) = update {
                    self.emit_expression(upd);
                    self.emit(Opcode::Pop, vec![]);
                }
                // Pop per-iteration scope and copy back (inner → outer)
                if per_iteration_count > 0 {
                    for (i, (_, outer_slot)) in per_iteration_vars.iter().enumerate() {
                        let inner_slot = shadow_start_slot + i;
                        self.emit(
                            Opcode::CopyLexical,
                            vec![inner_slot as i64, *outer_slot as i64],
                        );
                    }
                    self.emit(Opcode::BlockLeave, vec![]);
                    self.lexical_scopes.pop();
                    self.lexical_slot_count -= per_iteration_count;
                }
                self.emit(Opcode::Jump, vec![loop_start as i64]);
                // Exit path (JumpIfFalse lands here): restore env before leaving.
                // With NO condition there is no JumpIfFalse — exit_jump points
                // at the body's first instruction, and patching it would
                // clobber that instruction's operands (seen as `++n` never
                // incrementing inside `for(;;)`). Breaks use the sentinel.
                if cond.is_some() {
                    if per_iteration_count > 0 {
                        self.patch(exit_jump, self.current());
                        self.emit(Opcode::RestoreEnv, vec![]);
                    } else {
                        self.patch(exit_jump, self.current());
                    }
                } else if per_iteration_count > 0 {
                    self.emit(Opcode::RestoreEnv, vec![]);
                }
                // Patch `continue` jumps to the update tail; breaks for
                // empty-cond loops (sentinel) go to the TRUE exit — after the
                // init-scope BlockLeave below, matching natural fall-through.
                let jumps = self.pending_loop_jumps.pop().unwrap_or_default();
                let mut break_jumps: Vec<usize> = Vec::new();
                for (pos, kind) in &jumps {
                    match kind {
                        LoopJumpKind::Continue => self.patch(*pos, continue_target),
                        LoopJumpKind::Break => break_jumps.push(*pos),
                    }
                }
                // Leave for-init lexical scope
                if init_has_scope {
                    self.emit(Opcode::BlockLeave, vec![]);
                    self.leave_lexical_scope();
                }
                let for_true_exit = self.current();
                for pos in break_jumps {
                    self.patch(pos, for_true_exit);
                }
            }
            Stmt::Return(value, _) => {
                if let Some(val) = value {
                    self.emit_expression(val);
                } else {
                    self.emit(Opcode::LoadUndefined, vec![]);
                }
                self.emit(Opcode::Return, vec![]);
            }
            Stmt::Throw(value, _) => {
                self.emit_expression(value);
                self.emit(Opcode::Throw, vec![]);
            }
            Stmt::Var(kind, decls, _) => match kind {
                VarKind::Var => {
                    for decl in decls {
                        if let Some(pattern) = &decl.pattern {
                            if let Some(init) = &decl.init {
                                self.emit_expression(init);
                                self.emit_destructuring(
                                    pattern,
                                    &DestructureStore::Decl(kind.clone()),
                                );
                            }
                        } else {
                            let is_top_level =
                                self.env_scope_stack.is_empty() && self.captured_names.is_empty();
                            if !is_top_level && !self.locals.contains(&decl.name.to_string()) {
                                self.locals.push(decl.name.to_string());
                            }
                            if let Some(init) = &decl.init {
                                self.emit_expression(init);
                                if let Some((depth, env_slot)) = self.env_captured_slot(&decl.name)
                                {
                                    // StoreCaptured pops the value — no Pop needed
                                    self.emit(
                                        Opcode::StoreCaptured,
                                        vec![depth as i64, env_slot as i64],
                                    );
                                } else if let Some(idx) = self.local_index(&decl.name) {
                                    self.emit(Opcode::StoreLocal, vec![idx as i64]);
                                    self.emit(Opcode::Pop, vec![]);
                                } else if is_top_level {
                                    let name_idx = self.intern_string(&decl.name) as i64;
                                    self.emit(Opcode::StoreGlobal, vec![name_idx]);
                                    self.emit(Opcode::Pop, vec![]);
                                    self.emit_module_rename_sync(&decl.name);
                                }
                            }
                        }
                    }
                }
                VarKind::Let | VarKind::Const => {
                    for decl in decls {
                        if let Some(pattern) = &decl.pattern {
                            if let Some(init) = &decl.init {
                                self.emit_expression(init);
                                self.emit_destructuring(
                                    pattern,
                                    &DestructureStore::Decl(kind.clone()),
                                );
                            }
                        } else if let Some(slot) = self.lexical_slot(&decl.name) {
                            if let Some(init) = &decl.init {
                                self.emit_expression(init);
                            } else {
                                self.emit(Opcode::LoadUndefined, vec![]);
                            }
                            let op = if *kind == VarKind::Const {
                                Opcode::DeclareConst
                            } else {
                                Opcode::DeclareLet
                            };
                            self.emit(op, vec![slot as i64]);
                            // If this lexical binding is captured in a block env,
                            // copy from lexical slot to env slot so closures see it
                            if let Some((depth, env_slot)) = self.env_captured_slot(&decl.name) {
                                if depth == 0 {
                                    self.emit(Opcode::LoadLexical, vec![slot as i64]);
                                    self.emit(Opcode::StoreCaptured, vec![0, env_slot as i64]);
                                }
                            }
                        }
                    }
                }
            },
            Stmt::Break(_label, _) => {
                if self.switch_exit_stack.last().is_some() {
                    // Inside a switch — emit Jump with placeholder, track for patching
                    let pos = self.current();
                    self.emit(Opcode::Jump, vec![0]);
                    self.switch_break_jumps.push(pos);
                } else if let Some(exit) = self.loop_exit_stack.last() {
                    if *exit == usize::MAX {
                        let pos = self.current();
                        self.emit(Opcode::Jump, vec![0]);
                        if let Some(p) = self.pending_loop_jumps.last_mut() {
                            p.push((pos, LoopJumpKind::Break));
                        }
                    } else {
                        self.emit(Opcode::Jump, vec![*exit as i64]);
                    }
                }
            }
            Stmt::Continue(_label, _) => {
                if let Some(cont) = self.loop_cont_stack.last() {
                    if *cont == usize::MAX {
                        let pos = self.current();
                        self.emit(Opcode::Jump, vec![0]);
                        if let Some(p) = self.pending_loop_jumps.last_mut() {
                            p.push((pos, LoopJumpKind::Continue));
                        }
                    } else {
                        self.emit(Opcode::Jump, vec![*cont as i64]);
                    }
                }
            }
            Stmt::Function(fnode, _) => {
                let func_idx = self.compile_function(fnode) as i64;
                if let Some(ref name) = fnode.name {
                    if !self.locals.contains(&name.to_string()) {
                        self.locals.push(name.to_string());
                    }
                    self.emit(Opcode::MakeFunction, vec![func_idx]);
                    if let Some(idx) = self.local_index(name) {
                        self.emit(Opcode::StoreLocal, vec![idx as i64]);
                    }
                    self.emit(Opcode::Pop, vec![]);
                }
            }
            Stmt::Class(class, _) => {
                self.emit_class(class, false);
            }
            Stmt::Try(body, catch_opt, finalizer_opt, _) => {
                let try_idx = self.current();
                self.emit(Opcode::TryBegin, vec![0, 0]);

                // --- try body ---
                for stmt in body.iter() {
                    self.emit_statement(stmt);
                }

                match (catch_opt, finalizer_opt) {
                    (Some(c), None) => {
                        // try-catch (no finally) — TryEnd pops TryFrame
                        self.emit(Opcode::TryEnd, vec![]);
                        let past_catch = self.current();
                        self.emit(Opcode::Jump, vec![0]);
                        let catch_entry = self.current();
                        self.patch(try_idx, catch_entry);
                        if !c.param.is_empty() {
                            if !self.locals.contains(&c.param.to_string()) {
                                self.locals.push(c.param.to_string());
                            }
                            self.emit(
                                Opcode::StoreLocal,
                                vec![self.local_index(&c.param).unwrap() as i64],
                            );
                        }
                        self.emit(Opcode::Pop, vec![]);
                        for stmt in c.body.iter() {
                            self.emit_statement(stmt);
                        }
                        self.patch(past_catch, self.current());
                    }
                    (None, Some(fin)) => {
                        // try-finally (no catch) — no TryEnd, fall through to finally
                        let fin_entry = self.current();
                        self.patch_operand(try_idx, 1, fin_entry as i64);
                        for stmt in fin.iter() {
                            self.emit_statement(stmt);
                        }
                        let fd_pc = self.current();
                        let rethrow_pc = fd_pc + 2;
                        self.emit(Opcode::FinallyDone, vec![rethrow_pc as i64]);
                        let past_try = fd_pc + 3;
                        self.emit(Opcode::Jump, vec![past_try as i64]);
                        self.emit(Opcode::Throw, vec![]);
                    }
                    (Some(c), Some(fin)) => {
                        // try-catch-finally — no TryEnd, jump past catch, fall through to finally
                        let past_catch = self.current();
                        self.emit(Opcode::Jump, vec![0]);
                        let catch_entry = self.current();
                        self.patch(try_idx, catch_entry);
                        if !c.param.is_empty() {
                            if !self.locals.contains(&c.param.to_string()) {
                                self.locals.push(c.param.to_string());
                            }
                            self.emit(
                                Opcode::StoreLocal,
                                vec![self.local_index(&c.param).unwrap() as i64],
                            );
                        }
                        self.emit(Opcode::Pop, vec![]);
                        for stmt in c.body.iter() {
                            self.emit_statement(stmt);
                        }
                        let fin_entry = self.current();
                        self.patch(past_catch, fin_entry);
                        self.patch_operand(try_idx, 1, fin_entry as i64);
                        for stmt in fin.iter() {
                            self.emit_statement(stmt);
                        }
                        let fd_pc = self.current();
                        let rethrow_pc = fd_pc + 2;
                        self.emit(Opcode::FinallyDone, vec![rethrow_pc as i64]);
                        let past_try = fd_pc + 3;
                        self.emit(Opcode::Jump, vec![past_try as i64]);
                        self.emit(Opcode::Throw, vec![]);
                    }
                    (None, None) => {
                        // try with neither catch nor finally — just emit the body
                        self.emit(Opcode::TryEnd, vec![]);
                    }
                }
            }
            Stmt::Empty(_) => {}
            Stmt::ForIn(lhs, obj, body, _) => {
                // for (var key in obj) { body }
                // Register the loop variable as a local
                if let Expr::Identifier(name, _) = lhs.as_ref() {
                    if !self.locals.contains(&name.to_string()) {
                        self.locals.push(name.to_string());
                    }
                }
                self.emit_expression(obj);
                self.emit(Opcode::ForInInit, vec![]);
                let loop_start = self.current();
                let exit_jump = self.current();
                self.emit(Opcode::ForInNext, vec![0]); // patched below
                // Store the key into the loop variable
                if let Expr::Identifier(name, _) = lhs.as_ref() {
                    if let Some(idx) = self.local_index(name) {
                        self.emit(Opcode::StoreLocal, vec![idx as i64]);
                    } else {
                        let name_idx = self.intern_string(name) as i64;
                        self.emit(Opcode::StoreGlobal, vec![name_idx]);
                    }
                    // Pop the value that StoreLocal pushes back (it stays on stack for
                    // assignment-expression semantics, but here we only need it stored)
                    self.emit(Opcode::Pop, vec![]);
                }
                self.emit_statement(body);
                self.emit(Opcode::Jump, vec![loop_start as i64]);
                self.patch(exit_jump, self.current());
            }
            Stmt::ForOf(lhs, iterable, body, _) => {
                // for (x of iterable) { body }
                // Register the loop variable as a local
                if let Expr::Identifier(name, _) = lhs.as_ref() {
                    if !self.locals.contains(&name.to_string()) {
                        self.locals.push(name.to_string());
                    }
                }
                self.emit_expression(iterable);
                self.emit(Opcode::ForOfInit, vec![]);
                let loop_start = self.current();
                // break/continue use patchable jumps (loop end position is only
                // known after the body, like do-while).
                self.loop_exit_stack.push(usize::MAX);
                self.loop_cont_stack.push(usize::MAX);
                self.pending_loop_jumps.push(Vec::new());
                // Member LHS: push obj + key BELOW the loop state so the value
                // lands on top for StoreProperty ([.., obj, key, value]).
                let lhs_prefix = match lhs.as_ref() {
                    Expr::Member(obj, prop, computed, _) => {
                        self.emit_expression(obj);
                        if *computed {
                            self.emit_expression(prop);
                        } else {
                            let name = prop_name_as_string(prop);
                            let idx = self.intern_string(&name) as i64;
                            self.emit(Opcode::LoadStringConst, vec![idx]);
                        }
                        2
                    }
                    _ => 0,
                };
                let next_jump = self.current();
                self.emit(Opcode::ForOfNext, vec![0, lhs_prefix]); // patched below
                // Store the value into the loop variable
                match lhs.as_ref() {
                    Expr::Identifier(name, _) => {
                        if let Some(idx) = self.local_index(name) {
                            self.emit(Opcode::StoreLocal, vec![idx as i64]);
                        } else {
                            let name_idx = self.intern_string(name) as i64;
                            self.emit(Opcode::StoreGlobal, vec![name_idx]);
                        }
                        // StoreLocal pushes the value back — discard it
                        self.emit(Opcode::Pop, vec![]);
                    }
                    Expr::Member(_, _, _, _) => {
                        self.emit(Opcode::StoreProperty, vec![]);
                        self.emit(Opcode::Pop, vec![]);
                    }
                    _ => {
                        // Destructuring LHS not yet supported — discard the value
                        self.emit(Opcode::Pop, vec![]);
                    }
                }
                self.emit_statement(body);
                self.loop_cont_stack.pop();
                self.loop_exit_stack.pop();
                self.emit(Opcode::Jump, vec![loop_start as i64]);
                // Done case: drop the leftover obj+key prefix, then fall into
                // the shared exit which discards [iterator, nextMethod].
                let done_cleanup = self.current();
                for _ in 0..lhs_prefix {
                    self.emit(Opcode::Pop, vec![]);
                }
                let exit = self.current();
                self.emit(Opcode::Pop, vec![]);
                self.emit(Opcode::Pop, vec![]);
                self.patch(next_jump, done_cleanup);
                for (pos, kind) in self.pending_loop_jumps.pop().unwrap_or_default() {
                    let target = match kind {
                        LoopJumpKind::Break => exit,
                        LoopJumpKind::Continue => loop_start,
                    };
                    self.patch(pos, target);
                }
            }
            Stmt::Switch(discriminant, cases, default_body, _) => {
                self.emit_expression(discriminant);

                // Mark switch context for break statements
                let switch_marker = self.switch_break_jumps.len();
                self.switch_exit_stack.push(switch_marker);

                // === COMPARISON CHAIN ===
                // Each case: Dup, load test, StrictEq, JumpIfFalse → skip
                // If matched: Pop (remove dup), Jump → body entry in body section
                let mut body_targets = Vec::new();
                for case in cases {
                    self.emit(Opcode::Dup, vec![]);
                    self.emit_expression(&case.test);
                    self.emit(Opcode::StrictEq, vec![]);
                    let skip = self.current();
                    self.emit(Opcode::JumpIfFalse, vec![0]);
                    // Matched — remove dup and jump to body
                    self.emit(Opcode::Pop, vec![]);
                    let body_target = self.current();
                    self.emit(Opcode::Jump, vec![0]);
                    body_targets.push(body_target);
                    self.patch(skip, self.current());
                }

                // No match — pop discriminant
                self.emit(Opcode::Pop, vec![]);
                let no_match_target = self.current();
                self.emit(Opcode::Jump, vec![0]);

                // === BODY SECTION ===
                // Patch each case's body jump to its body location
                for (i, &body_target) in body_targets.iter().enumerate() {
                    self.patch(body_target, self.current());
                    for stmt in &cases[i].body {
                        self.emit_statement(stmt);
                    }
                }

                // Default case body (also reachable via fall-through from last case)
                let default_target = self.current();
                if let Some(body) = default_body {
                    for stmt in body.iter() {
                        self.emit_statement(stmt);
                    }
                }

                let after_pc = self.current();
                // Patch break jumps made inside case bodies
                for i in switch_marker..self.switch_break_jumps.len() {
                    self.patch(self.switch_break_jumps[i], after_pc);
                }
                self.switch_break_jumps.truncate(switch_marker);
                // Patch no-match jump to default or past switch
                self.patch(no_match_target, default_target);
                self.switch_exit_stack.pop();
            }
        }
    }

    fn emit_last_statement(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Expr(expr, _) => {
                self.emit_expression(expr);
            }
            Stmt::Return(value, _) => {
                if let Some(val) = value {
                    self.emit_expression(val);
                } else {
                    self.emit(Opcode::LoadUndefined, vec![]);
                }
                self.emit(Opcode::Return, vec![]);
            }
            Stmt::Throw(value, _) => {
                self.emit_expression(value);
                self.emit(Opcode::Throw, vec![]);
            }
            _ => {
                self.emit_statement(stmt);
                self.emit(Opcode::LoadUndefined, vec![]);
            }
        }
        let needs_return = match self.instructions.last() {
            Some(last) => last.opcode != Opcode::Return,
            None => true,
        };
        if needs_return {
            self.emit(Opcode::Return, vec![]);
        }
    }

    fn emit_expression(&mut self, expr: &Expr) {
        match expr {
            Expr::Number(val, _) => {
                let is_int = val.fract() == 0.0 && val.is_finite();
                if is_int {
                    let ival = *val as i64;
                    if ival >= -(1 << 30) as i64 && ival < (1 << 30) as i64 {
                        self.emit(Opcode::LoadSmi, vec![ival]);
                        return;
                    }
                }
                let idx = self.intern_float(*val) as i64;
                self.emit(Opcode::LoadFloat64, vec![idx]);
            }
            Expr::String(val, _) => {
                let idx = self.intern_string(val) as i64;
                self.emit(Opcode::LoadStringConst, vec![idx]);
            }
            Expr::Boolean(val, _) => {
                self.emit(Opcode::LoadBoolean, vec![if *val { 1 } else { 0 }]);
            }
            Expr::Null(_) => {
                self.emit(Opcode::LoadNull, vec![]);
            }
            Expr::Undefined(_) => {
                self.emit(Opcode::LoadUndefined, vec![]);
            }
            Expr::Template { parts, exprs, .. } => {
                if exprs.is_empty() {
                    let idx = self.intern_string(&parts[0]) as i64;
                    self.emit(Opcode::LoadStringConst, vec![idx]);
                } else {
                    let idx = self.intern_string(&parts[0]) as i64;
                    self.emit(Opcode::LoadStringConst, vec![idx]);
                    for (i, expr) in exprs.iter().enumerate() {
                        self.emit_expression(expr);
                        self.emit(Opcode::ToString, vec![]);
                        self.emit(Opcode::StringConcat, vec![]);
                        if let Some(next) = parts.get(i + 1) {
                            let idx = self.intern_string(next) as i64;
                            self.emit(Opcode::LoadStringConst, vec![idx]);
                            self.emit(Opcode::StringConcat, vec![]);
                        }
                    }
                }
            }
            Expr::Identifier(name, _) => {
                if let Some((depth, slot)) = self.env_captured_slot(name) {
                    self.emit(Opcode::LoadCaptured, vec![depth as i64, slot as i64]);
                } else if let Some(slot) = self.lexical_slot(name) {
                    self.emit(Opcode::LoadLexical, vec![slot as i64]);
                } else if let Some(idx) = self.local_index(name) {
                    self.emit(Opcode::LoadLocal, vec![idx as i64]);
                } else if self.module_mode {
                    if let Some(import_idx) = self.module_imports.get(name) {
                        self.emit(Opcode::LoadModuleImport, vec![*import_idx as i64]);
                    } else {
                        let name_idx = self.intern_string(name) as i64;
                        self.emit(Opcode::LoadGlobal, vec![name_idx]);
                    }
                } else {
                    let name_idx = self.intern_string(name) as i64;
                    self.emit(Opcode::LoadGlobal, vec![name_idx]);
                }
            }
            Expr::This(_) => {
                self.emit(Opcode::LoadThis, vec![]);
            }
            Expr::Unary(op, arg, _) => {
                // delete needs special handling: don't emit_expression(arg) which would
                // evaluate the member expression (including LoadProperty)
                if *op == UnaryOp::Delete {
                    match arg.as_ref() {
                        Expr::Member(obj, prop, computed, _) => {
                            self.emit_expression(obj);
                            if *computed {
                                self.emit_expression(prop);
                            } else {
                                let name = prop_name_as_string(prop);
                                let idx = self.intern_string(&name) as i64;
                                self.emit(Opcode::LoadStringConst, vec![idx]);
                            }
                            self.emit(Opcode::DeleteProperty, vec![]);
                        }
                        _ => {
                            self.emit_expression(arg);
                            self.emit(Opcode::Pop, vec![]);
                            self.emit(Opcode::LoadBoolean, vec![1]);
                        }
                    }
                } else {
                    self.emit_expression(arg);
                    match op {
                        UnaryOp::Minus => self.emit(Opcode::Neg, vec![]),
                        UnaryOp::Plus => self.emit(Opcode::UnaryPlus, vec![]),
                        UnaryOp::Not => self.emit(Opcode::Not, vec![]),
                        UnaryOp::BitNot => self.emit(Opcode::BitNot, vec![]),
                        UnaryOp::Typeof => self.emit(Opcode::TypeOf, vec![]),
                        UnaryOp::Void => {
                            self.emit(Opcode::Pop, vec![]);
                            self.emit(Opcode::LoadUndefined, vec![]);
                        }
                        UnaryOp::Delete => unreachable!(),
                    }
                }
            }
            Expr::Update(op, arg, prefix, _) => match arg.as_ref() {
                Expr::Identifier(name, _) => {
                    let is_pre = *prefix;
                    if let Some((depth, slot)) = self.env_captured_slot(name) {
                        self.emit(Opcode::LoadCaptured, vec![depth as i64, slot as i64]);
                        if !is_pre {
                            self.emit(Opcode::Dup, vec![]);
                        }
                        self.emit(Opcode::LoadSmi, vec![1]);
                        let opcode = match op {
                            UpdateOp::PlusPlus => Opcode::Add,
                            UpdateOp::MinusMinus => Opcode::Sub,
                        };
                        self.emit(opcode, vec![]);
                        self.emit(Opcode::StoreCaptured, vec![depth as i64, slot as i64]);
                        if !is_pre {
                            self.emit(Opcode::Pop, vec![]);
                        }
                    } else if self.lexical_slot(name).is_some() {
                        let slot = self.lexical_slot(name).unwrap();
                        self.emit(Opcode::LoadLexical, vec![slot as i64]);
                        if !is_pre {
                            self.emit(Opcode::Dup, vec![]);
                        }
                        self.emit(Opcode::LoadSmi, vec![1]);
                        let opcode = match op {
                            UpdateOp::PlusPlus => Opcode::Add,
                            UpdateOp::MinusMinus => Opcode::Sub,
                        };
                        self.emit(opcode, vec![]);
                        self.emit(Opcode::StoreLexical, vec![slot as i64]);
                        if !is_pre {
                            self.emit(Opcode::Pop, vec![]);
                        }
                    } else if let Some(idx) = self.local_index(name) {
                        let opcode = match op {
                            UpdateOp::PlusPlus => Opcode::IncLocal,
                            UpdateOp::MinusMinus => Opcode::DecLocal,
                        };
                        self.emit(opcode, vec![idx as i64, is_pre as i64]);
                    } else {
                        let opcode = match op {
                            UpdateOp::PlusPlus => Opcode::IncGlobal,
                            UpdateOp::MinusMinus => Opcode::DecGlobal,
                        };
                        let name_idx = self.intern_string(name) as i64;
                        self.emit(opcode, vec![name_idx, is_pre as i64]);
                    }
                }
                Expr::Member(obj, prop, computed, _) => {
                    let is_pre = *prefix;
                    let temp_slot = if is_pre {
                        None
                    } else {
                        let name = format!("__upd_{}", self.locals.len());
                        self.locals.push(name);
                        Some(self.locals.len() - 1)
                    };
                    match obj.as_ref() {
                        Expr::Super(_) => {
                            // super.x++ → write to this[key], read this.__proto__.__proto__.x
                            self.emit(Opcode::LoadThis, vec![]);
                            self.emit_property_key(prop, *computed);
                            self.emit(Opcode::LoadThis, vec![]);
                            let proto_key = self.intern_string("__proto__") as i64;
                            self.emit(Opcode::LoadStringConst, vec![proto_key]);
                            self.emit(Opcode::LoadProperty, vec![]);
                            self.emit(Opcode::LoadStringConst, vec![proto_key]);
                            self.emit(Opcode::LoadProperty, vec![]);
                            self.emit_property_key(prop, *computed);
                            self.emit(Opcode::LoadProperty, vec![]);
                        }
                        _ => {
                            self.emit_expression(obj);
                            self.emit_property_key(prop, *computed);
                            self.emit(Opcode::Dup2, vec![]);
                            self.emit(Opcode::LoadProperty, vec![]);
                        }
                    }
                    if !is_pre {
                        self.emit(Opcode::Dup, vec![]);
                        self.emit(Opcode::StoreLocal, vec![temp_slot.unwrap() as i64]);
                        self.emit(Opcode::Pop, vec![]);
                    }
                    self.emit(Opcode::LoadSmi, vec![1]);
                    let opcode = match op {
                        UpdateOp::PlusPlus => Opcode::Add,
                        UpdateOp::MinusMinus => Opcode::Sub,
                    };
                    self.emit(opcode, vec![]);
                    self.emit(Opcode::StoreProperty, vec![]);
                    if !is_pre {
                        self.emit(Opcode::LoadLocal, vec![temp_slot.unwrap() as i64]);
                        self.emit(Opcode::Swap, vec![]);
                        self.emit(Opcode::Pop, vec![]);
                    }
                }
                Expr::PrivateMember(obj, name, _) => {
                    // StorePrivateProperty pops obj+value and pushes nothing,
                    // so the object is Dup'd and results parked in temps.
                    let is_pre = *prefix;
                    let old_name = format!("__upd_{}", self.locals.len());
                    self.locals.push(old_name);
                    let old_slot = self.locals.len() - 1;
                    let new_name = format!("__upd_{}", self.locals.len());
                    self.locals.push(new_name);
                    let new_slot = self.locals.len() - 1;
                    let slot_idx = self
                        .private_field_names
                        .iter()
                        .position(|n| n.as_str() == name.as_ref())
                        .unwrap_or(0);
                    self.emit_expression(obj);
                    self.emit(Opcode::Dup, vec![]);
                    self.emit(Opcode::LoadPrivateProperty, vec![slot_idx as i64]);
                    if !is_pre {
                        // Park the old value for the postfix result.
                        self.emit(Opcode::Dup, vec![]);
                        self.emit(Opcode::StoreLocal, vec![old_slot as i64]);
                        self.emit(Opcode::Pop, vec![]);
                    }
                    self.emit(Opcode::LoadSmi, vec![1]);
                    let opcode = match op {
                        UpdateOp::PlusPlus => Opcode::Add,
                        UpdateOp::MinusMinus => Opcode::Sub,
                    };
                    self.emit(opcode, vec![]);
                    self.emit(Opcode::StoreLocal, vec![new_slot as i64]);
                    self.emit(Opcode::StorePrivateProperty, vec![slot_idx as i64]);
                    self.emit(Opcode::Pop, vec![]);
                    self.emit(
                        Opcode::LoadLocal,
                        vec![if is_pre { new_slot } else { old_slot } as i64],
                    );
                }
                _ => {
                    self.emit_expression(arg);
                    self.emit(Opcode::Pop, vec![]);
                    self.emit(Opcode::LoadUndefined, vec![]);
                }
            },
            Expr::DestructureAssign(pattern, rhs, _) => {
                // Evaluate RHS, destructure into the pattern, and leave
                // the RHS value as the expression result.
                self.emit_expression(rhs);
                self.emit_destructuring(pattern, &DestructureStore::Assign);
            }
            Expr::Binary(op, lhs, rhs, _) => {
                if *op == BinaryOp::Assign {
                    match lhs.as_ref() {
                        Expr::Identifier(name, _) => {
                            self.emit_expression(rhs);
                            if let Some(env_slot) = self.captured_slot(name) {
                                self.emit(Opcode::StoreCaptured, vec![0, env_slot as i64]);
                            } else if let Some((depth, slot)) = self.env_captured_slot(name) {
                                self.emit(Opcode::StoreCaptured, vec![depth as i64, slot as i64]);
                            } else if let Some(idx) = self.local_index(name) {
                                self.emit(Opcode::StoreLocal, vec![idx as i64]);
                            } else {
                                let name_idx = self.intern_string(name) as i64;
                                self.emit(Opcode::StoreGlobal, vec![name_idx]);
                            }
                        }
                        Expr::Member(obj, prop, computed, _) => {
                            self.emit_expression(obj);
                            if *computed {
                                self.emit_expression(prop);
                            } else {
                                let name = prop_name_as_string(prop);
                                let idx = self.intern_string(&name) as i64;
                                self.emit(Opcode::LoadStringConst, vec![idx]);
                            }
                            self.emit_expression(rhs);
                            self.emit(Opcode::StoreProperty, vec![]);
                        }
                        _ => {
                            self.emit_expression(rhs);
                        }
                    }
                    return;
                }
                // Comma operator: evaluate lhs, discard, then evaluate rhs
                if *op == BinaryOp::Comma {
                    self.emit_expression(lhs);
                    self.emit(Opcode::Pop, vec![]);
                    self.emit_expression(rhs);
                    return;
                }
                // Short-circuit logical operators
                // JumpIfFalse/JumpIfTrue POP the value, so we Dup first to preserve the result.
                if *op == BinaryOp::LogicalAnd {
                    // a && b: lhs, Dup, JumpIfFalse→end, Pop, rhs, end:
                    self.emit_expression(lhs);
                    self.emit(Opcode::Dup, vec![]);
                    let end = self.current();
                    self.emit(Opcode::JumpIfFalse, vec![0]);
                    self.emit(Opcode::Pop, vec![]);
                    self.emit_expression(rhs);
                    self.patch(end, self.current());
                    return;
                }
                if *op == BinaryOp::LogicalOr {
                    // a || b: lhs, Dup, JumpIfTrue→end, Pop, rhs, end:
                    self.emit_expression(lhs);
                    self.emit(Opcode::Dup, vec![]);
                    let end = self.current();
                    self.emit(Opcode::JumpIfTrue, vec![0]);
                    self.emit(Opcode::Pop, vec![]);
                    self.emit_expression(rhs);
                    self.patch(end, self.current());
                    return;
                }
                if *op == BinaryOp::NullishCoalescing {
                    // a ?? b: lhs, Dup, JumpIfNullOrUndefined→drop,
                    // Jump→end (skip rhs; lhs stays), drop: Pop, rhs, end:
                    self.emit_expression(lhs);
                    self.emit(Opcode::Dup, vec![]);
                    let drop = self.current();
                    self.emit(Opcode::JumpIfNullOrUndefined, vec![0]);
                    let end = self.current();
                    self.emit(Opcode::Jump, vec![0]);
                    self.patch(drop, self.current());
                    self.emit(Opcode::Pop, vec![]);
                    self.emit_expression(rhs);
                    self.patch(end, self.current());
                    return;
                }
                self.emit_expression(lhs);
                self.emit_expression(rhs);
                let opcode = match op {
                    BinaryOp::Add => Opcode::Add,
                    BinaryOp::Sub => Opcode::Sub,
                    BinaryOp::Mul => Opcode::Mul,
                    BinaryOp::Div => Opcode::Div,
                    BinaryOp::Mod => Opcode::Mod,
                    BinaryOp::Exp => Opcode::Exp,
                    BinaryOp::Shl => Opcode::Shl,
                    BinaryOp::Shr => Opcode::Shr,
                    BinaryOp::ShrU => Opcode::ShrU,
                    BinaryOp::BitOr => Opcode::BitOr,
                    BinaryOp::BitXor => Opcode::BitXor,
                    BinaryOp::BitAnd => Opcode::BitAnd,
                    BinaryOp::Eq => Opcode::Eq,
                    BinaryOp::Ne => Opcode::Ne,
                    BinaryOp::StrictEq => Opcode::StrictEq,
                    BinaryOp::StrictNe => Opcode::StrictNe,
                    BinaryOp::Lt => Opcode::Lt,
                    BinaryOp::Gt => Opcode::Gt,
                    BinaryOp::Le => Opcode::Le,
                    BinaryOp::Ge => Opcode::Ge,
                    BinaryOp::In => Opcode::In,
                    BinaryOp::Instanceof => Opcode::Instanceof,
                    BinaryOp::LogicalAnd
                    | BinaryOp::LogicalOr
                    | BinaryOp::NullishCoalescing
                    | BinaryOp::Comma
                    | BinaryOp::Assign => unreachable!(),
                };
                self.emit(opcode, vec![]);
            }
            Expr::Conditional(cond, then, else_, _) => {
                self.emit_expression(cond);
                let else_jump = self.current();
                self.emit(Opcode::JumpIfFalse, vec![0]);
                self.emit_expression(then);
                let exit_jump = self.current();
                self.emit(Opcode::Jump, vec![0]);
                self.patch(else_jump, self.current());
                self.emit_expression(else_);
                self.patch(exit_jump, self.current());
            }
            Expr::Call(callee, args, _) => {
                let has_spread = args.iter().any(|a| a.is_spread);
                if has_spread {
                    // Build args array for spread calls
                    match callee.as_ref() {
                        Expr::Member(obj, prop, computed, _) => {
                            match obj.as_ref() {
                                Expr::Super(_) => {
                                    // super.method(...args): receiver=this, lookup via this.__proto__.__proto__
                                    self.emit(Opcode::LoadThis, vec![]);
                                    self.emit(Opcode::Dup, vec![]);
                                    let proto_key = self.intern_string("__proto__") as i64;
                                    self.emit(Opcode::LoadStringConst, vec![proto_key]);
                                    self.emit(Opcode::LoadProperty, vec![]);
                                    self.emit(Opcode::LoadStringConst, vec![proto_key]);
                                    self.emit(Opcode::LoadProperty, vec![]);
                                    if *computed {
                                        self.emit_expression(prop);
                                    } else {
                                        let name = prop_name_as_string(prop);
                                        let idx = self.intern_string(&name) as i64;
                                        self.emit(Opcode::LoadStringConst, vec![idx]);
                                    }
                                    self.emit(Opcode::LoadProperty, vec![]);
                                }
                                _ => {
                                    self.emit_expression(obj);
                                    self.emit(Opcode::Dup, vec![]);
                                    if *computed {
                                        self.emit_expression(prop);
                                    } else {
                                        let name = prop_name_as_string(prop);
                                        let idx = self.intern_string(&name) as i64;
                                        self.emit(Opcode::LoadStringConst, vec![idx]);
                                    }
                                    self.emit(Opcode::LoadProperty, vec![]);
                                }
                            }
                            // stack: [receiver, method]
                            self.emit(Opcode::NewArray, vec![0]);
                            for arg in args {
                                self.emit_expression(&arg.expr);
                                if arg.is_spread {
                                    self.emit(Opcode::ToArrayFromIterable, vec![]);
                                    self.emit(Opcode::ArrayExtend, vec![]);
                                } else {
                                    self.emit(Opcode::ArrayPush, vec![]);
                                }
                            }
                            // stack: [receiver, method, args_array] — correct for CallFromArray
                        }
                        Expr::Super(_) => {
                            self.emit(Opcode::LoadThis, vec![]);
                            self.emit_expression(callee);
                            // stack: [this, callee]
                            self.emit(Opcode::NewArray, vec![0]);
                            for arg in args {
                                self.emit_expression(&arg.expr);
                                if arg.is_spread {
                                    self.emit(Opcode::ToArrayFromIterable, vec![]);
                                    self.emit(Opcode::ArrayExtend, vec![]);
                                } else {
                                    self.emit(Opcode::ArrayPush, vec![]);
                                }
                            }
                            // stack: [this, callee, args_array] — correct for CallFromArray
                        }
                        _ => {
                            self.emit(Opcode::LoadUndefined, vec![]);
                            self.emit_expression(callee);
                            // stack: [this, callee]
                            self.emit(Opcode::NewArray, vec![0]);
                            for arg in args {
                                self.emit_expression(&arg.expr);
                                if arg.is_spread {
                                    self.emit(Opcode::ToArrayFromIterable, vec![]);
                                    self.emit(Opcode::ArrayExtend, vec![]);
                                } else {
                                    self.emit(Opcode::ArrayPush, vec![]);
                                }
                            }
                            // stack: [this, callee, args_array] — correct for CallFromArray
                        }
                    }
                    // stack: [args_array, callee, this] (or [args_array, method, receiver] after Swap)
                    self.emit(Opcode::CallFromArray, vec![]);
                } else {
                    match callee.as_ref() {
                        Expr::Member(obj, prop, computed, _) => {
                            match obj.as_ref() {
                                Expr::Super(_) => {
                                    // super.method(): receiver=this, lookup via this.__proto__.__proto__
                                    self.emit(Opcode::LoadThis, vec![]);
                                    self.emit(Opcode::Dup, vec![]);
                                    let proto_key = self.intern_string("__proto__") as i64;
                                    self.emit(Opcode::LoadStringConst, vec![proto_key]);
                                    self.emit(Opcode::LoadProperty, vec![]);
                                    self.emit(Opcode::LoadStringConst, vec![proto_key]);
                                    self.emit(Opcode::LoadProperty, vec![]);
                                    if *computed {
                                        self.emit_expression(prop);
                                    } else {
                                        let name = prop_name_as_string(prop);
                                        let idx = self.intern_string(&name) as i64;
                                        self.emit(Opcode::LoadStringConst, vec![idx]);
                                    }
                                    self.emit(Opcode::LoadProperty, vec![]);
                                }
                                _ => {
                                    // Method call: preserve receiver (this) below the method
                                    self.emit_expression(obj);
                                    self.emit(Opcode::Dup, vec![]);
                                    if *computed {
                                        self.emit_expression(prop);
                                    } else {
                                        let name = prop_name_as_string(prop);
                                        let idx = self.intern_string(&name) as i64;
                                        self.emit(Opcode::LoadStringConst, vec![idx]);
                                    }
                                    self.emit(Opcode::LoadProperty, vec![]);
                                }
                            }
                        }
                        Expr::Super(_) => {
                            // super() call: this = current this (for constructor)
                            self.emit(Opcode::LoadThis, vec![]);
                            self.emit_expression(callee);
                        }
                        _ => {
                            // Regular call: this = undefined
                            self.emit(Opcode::LoadUndefined, vec![]);
                            self.emit_expression(callee);
                        }
                    }
                    // stack: [this, callee] or [receiver, method]
                    for arg in args {
                        self.emit_expression(&arg.expr);
                    }
                    self.emit(Opcode::Call, vec![args.len() as i64]);
                }
            }
            Expr::New(callee, args, _) => {
                self.emit_expression(callee);
                for arg in args {
                    self.emit_expression(&arg.expr);
                }
                self.emit(Opcode::New, vec![args.len() as i64]);
            }
            Expr::PrivateMember(obj, name, _) => {
                self.emit_expression(obj);
                // Resolve #name to slot index from private_field_names
                let slot_idx = self
                    .private_field_names
                    .iter()
                    .position(|n| n.as_str() == name.as_ref())
                    .unwrap_or(0);
                self.emit(Opcode::LoadPrivateProperty, vec![slot_idx as i64]);
            }
            Expr::OptionalChain(base, links, _) => {
                // a?.b.c?.[d]?.(x)
                // Every `?.` link gets its own nullish guard; when any guard
                // trips, the WHOLE chain yields undefined (later links never
                // evaluate). Guards jump to per-label tails that pop the
                // running value (+ receiver in the `?.()` method case) and
                // push undefined.
                self.emit_expression(base);
                let mut pending_nullish: Vec<(usize, usize)> = Vec::new();
                for (i, link) in links.iter().enumerate() {
                    let next_is_call = links
                        .get(i + 1)
                        .is_some_and(|l| matches!(l.kind, OptionalLinkKind::Call(..)));
                    let prev_is_load = i > 0
                        && matches!(
                            links[i - 1].kind,
                            OptionalLinkKind::Prop(..)
                                | OptionalLinkKind::Computed(..)
                                | OptionalLinkKind::Private(..)
                        );
                    if link.optional {
                        // [v] -> Dup -> [v,v] -> JumpIfNullOrUndefined (POPS) -> [v]
                        self.emit(Opcode::Dup, vec![]);
                        let guard = self.current();
                        self.emit(Opcode::JumpIfNullOrUndefined, vec![0]);
                        let pops =
                            if matches!(link.kind, OptionalLinkKind::Call(..)) && prev_is_load {
                                2 // [receiver, method] both dropped
                            } else {
                                1
                            };
                        pending_nullish.push((guard, pops));
                    }
                    match &link.kind {
                        OptionalLinkKind::Prop(name) => {
                            if next_is_call {
                                // keep the receiver below the loaded method
                                self.emit(Opcode::Dup, vec![]);
                            }
                            let idx = self.intern_string(name) as i64;
                            self.emit(Opcode::LoadStringConst, vec![idx]);
                            self.emit(Opcode::LoadProperty, vec![]);
                        }
                        OptionalLinkKind::Computed(expr) => {
                            if next_is_call {
                                self.emit(Opcode::Dup, vec![]);
                            }
                            self.emit_expression(expr);
                            self.emit(Opcode::LoadProperty, vec![]);
                        }
                        OptionalLinkKind::Private(name) => {
                            if next_is_call {
                                self.emit(Opcode::Dup, vec![]);
                            }
                            let slot_idx = self
                                .private_field_names
                                .iter()
                                .position(|n| n.as_str() == name.as_ref())
                                .unwrap_or(0);
                            self.emit(Opcode::LoadPrivateProperty, vec![slot_idx as i64]);
                        }
                        OptionalLinkKind::Call(args) => {
                            if !prev_is_load {
                                // plain function value: call with this = undefined
                                // [fn] -> LoadUndefined -> [fn, undefined]
                                //      -> Swap -> [undefined, fn]
                                self.emit(Opcode::LoadUndefined, vec![]);
                                self.emit(Opcode::Swap, vec![]);
                            }
                            let has_spread = args.iter().any(|a| a.is_spread);
                            if has_spread {
                                self.emit(Opcode::NewArray, vec![0]);
                                for arg in args {
                                    self.emit_expression(&arg.expr);
                                    if arg.is_spread {
                                        self.emit(Opcode::ToArrayFromIterable, vec![]);
                                        self.emit(Opcode::ArrayExtend, vec![]);
                                    } else {
                                        self.emit(Opcode::ArrayPush, vec![]);
                                    }
                                }
                                // [this, callee, args_array] for CallFromArray
                                self.emit(Opcode::CallFromArray, vec![]);
                            } else {
                                for arg in args {
                                    self.emit_expression(&arg.expr);
                                }
                                self.emit(Opcode::Call, vec![args.len() as i64]);
                            }
                        }
                    }
                }
                if !pending_nullish.is_empty() {
                    let end = self.current();
                    self.emit(Opcode::Jump, vec![0]);
                    let mut tails = Vec::new();
                    for (guard, pops) in pending_nullish {
                        self.patch(guard, self.current());
                        for _ in 0..pops {
                            self.emit(Opcode::Pop, vec![]);
                        }
                        self.emit(Opcode::LoadUndefined, vec![]);
                        let tail = self.current();
                        self.emit(Opcode::Jump, vec![0]);
                        tails.push(tail);
                    }
                    self.patch(end, self.current());
                    for t in tails {
                        self.patch(t, self.current());
                    }
                }
            }
            Expr::Member(obj, prop, computed, _) => {
                match obj.as_ref() {
                    Expr::Super(_) => {
                        // super.prop: lookup via this.__proto__.__proto__
                        self.emit(Opcode::LoadThis, vec![]);
                        let proto_key = self.intern_string("__proto__") as i64;
                        self.emit(Opcode::LoadStringConst, vec![proto_key]);
                        self.emit(Opcode::LoadProperty, vec![]);
                        self.emit(Opcode::LoadStringConst, vec![proto_key]);
                        self.emit(Opcode::LoadProperty, vec![]);
                    }
                    _ => {
                        self.emit_expression(obj);
                    }
                }
                if *computed {
                    self.emit_expression(prop);
                } else {
                    let name = prop_name_as_string(prop);
                    let idx = self.intern_string(&name) as i64;
                    self.emit(Opcode::LoadStringConst, vec![idx]);
                }
                self.emit(Opcode::LoadProperty, vec![]);
            }
            Expr::Assign(target, value, _) => match target.as_ref() {
                Expr::Identifier(name, _) => {
                    self.emit_expression(value);
                    if let Some((depth, slot)) = self.env_captured_slot(name) {
                        // StoreCaptured pops the value; Dup keeps it for the
                        // statement's cleanup Pop (net 0).
                        self.emit(Opcode::Dup, vec![]);
                        self.emit(Opcode::StoreCaptured, vec![depth as i64, slot as i64]);
                    } else if let Some(slot) = self.lexical_slot(name) {
                        self.emit(Opcode::StoreLexical, vec![slot as i64]);
                    } else if let Some(idx) = self.local_index(name) {
                        self.emit(Opcode::StoreLocal, vec![idx as i64]);
                    } else if self.module_mode {
                        if let Some(import_idx) = self.module_imports.get(name) {
                            // Assigning to an imported binding is a TypeError
                            // at runtime (§9.2.2.3).
                            self.emit(Opcode::StoreModuleImport, vec![*import_idx as i64]);
                        } else {
                            let name_idx = self.intern_string(name) as i64;
                            self.emit(Opcode::StoreGlobal, vec![name_idx]);
                        }
                    } else {
                        let name_idx = self.intern_string(name) as i64;
                        self.emit(Opcode::StoreGlobal, vec![name_idx]);
                    }
                }
                Expr::Member(obj, prop, computed, _) => {
                    match obj.as_ref() {
                        Expr::Super(_) => {
                            // super.prop = val → this.prop = val
                            // ([[Set]] receiver is this, assignment lands on child instance)
                            self.emit(Opcode::LoadThis, vec![]);
                        }
                        _ => {
                            self.emit_expression(obj);
                        }
                    }
                    if *computed {
                        self.emit_expression(prop);
                    } else {
                        let name = prop_name_as_string(prop);
                        let idx = self.intern_string(&name) as i64;
                        self.emit(Opcode::LoadStringConst, vec![idx]);
                    }
                    self.emit_expression(value);
                    self.emit(Opcode::StoreProperty, vec![]);
                }
                Expr::PrivateMember(obj, name, _) => {
                    self.emit_expression(obj);
                    self.emit_expression(value);
                    let slot_idx = self
                        .private_field_names
                        .iter()
                        .position(|n| n.as_str() == name.as_ref())
                        .unwrap_or(0);
                    self.emit(Opcode::StorePrivateProperty, vec![slot_idx as i64]);
                }
                _ => {
                    self.emit_expression(value);
                }
            },
            Expr::CompoundAssign(op, target, rhs, _) => {
                if matches!(
                    op,
                    BinaryOp::LogicalAnd | BinaryOp::LogicalOr | BinaryOp::NullishCoalescing
                ) {
                    self.emit_short_circuit_assign(op, target, rhs);
                    return;
                }
                let bin_opcode = compound_binary_opcode(*op);
                match target.as_ref() {
                    Expr::Identifier(name, _) => {
                        if let Some((depth, slot)) = self.env_captured_slot(name) {
                            self.emit(Opcode::LoadCaptured, vec![depth as i64, slot as i64]);
                            self.emit_expression(rhs);
                            self.emit(bin_opcode, vec![]);
                            // StoreCaptured pops the result; Dup keeps it for the
                            // statement's cleanup Pop (net 0).
                            self.emit(Opcode::Dup, vec![]);
                            self.emit(Opcode::StoreCaptured, vec![depth as i64, slot as i64]);
                        } else if let Some(slot) = self.lexical_slot(name) {
                            self.emit(Opcode::LoadLexical, vec![slot as i64]);
                            self.emit_expression(rhs);
                            self.emit(bin_opcode, vec![]);
                            self.emit(Opcode::StoreLexical, vec![slot as i64]);
                        } else if let Some(idx) = self.local_index(name) {
                            self.emit(Opcode::LoadLocal, vec![idx as i64]);
                            self.emit_expression(rhs);
                            self.emit(bin_opcode, vec![]);
                            self.emit(Opcode::StoreLocal, vec![idx as i64]);
                        } else {
                            let name_idx = self.intern_string(name) as i64;
                            self.emit(Opcode::LoadGlobal, vec![name_idx]);
                            self.emit_expression(rhs);
                            self.emit(bin_opcode, vec![]);
                            self.emit(Opcode::StoreGlobal, vec![name_idx]);
                        }
                    }
                    Expr::Member(obj, prop, computed, _) => {
                        match obj.as_ref() {
                            Expr::Super(_) => {
                                // super.prop += rhs
                                // Stack after write-setup: [this, key]
                                self.emit(Opcode::LoadThis, vec![]);
                                let key_idx = if *computed {
                                    self.emit_expression(prop);
                                    // can't statically determine key
                                    0
                                } else {
                                    let name = prop_name_as_string(prop);
                                    let idx = self.intern_string(&name) as i64;
                                    self.emit(Opcode::LoadStringConst, vec![idx]);
                                    idx
                                };
                                // Read old value: this.__proto__.__proto__.prop
                                self.emit(Opcode::LoadThis, vec![]);
                                let proto_key = self.intern_string("__proto__") as i64;
                                self.emit(Opcode::LoadStringConst, vec![proto_key]);
                                self.emit(Opcode::LoadProperty, vec![]);
                                self.emit(Opcode::LoadStringConst, vec![proto_key]);
                                self.emit(Opcode::LoadProperty, vec![]);
                                if *computed {
                                    self.emit_expression(prop);
                                } else {
                                    self.emit(Opcode::LoadStringConst, vec![key_idx]);
                                }
                                self.emit(Opcode::LoadProperty, vec![]);
                                self.emit_expression(rhs);
                                self.emit(bin_opcode, vec![]);
                                self.emit(Opcode::StoreProperty, vec![]);
                            }
                            _ => {
                                // Desugar: o.a += rhs → o.a = o.a + rhs
                                // Emit obj+key first for StoreProperty (bottom of stack)
                                self.emit_expression(obj);
                                if *computed {
                                    self.emit_expression(prop);
                                } else {
                                    let name = prop_name_as_string(prop);
                                    let idx = self.intern_string(&name) as i64;
                                    self.emit(Opcode::LoadStringConst, vec![idx]);
                                }
                                // Emit obj+key again for LoadProperty
                                self.emit_expression(obj);
                                if *computed {
                                    self.emit_expression(prop);
                                } else {
                                    let name = prop_name_as_string(prop);
                                    let idx = self.intern_string(&name) as i64;
                                    self.emit(Opcode::LoadStringConst, vec![idx]);
                                }
                                self.emit(Opcode::LoadProperty, vec![]);
                                self.emit_expression(rhs);
                                self.emit(bin_opcode, vec![]);
                                self.emit(Opcode::StoreProperty, vec![]);
                            }
                        }
                    }
                    Expr::PrivateMember(obj, name, _) => {
                        let slot_idx = self
                            .private_field_names
                            .iter()
                            .position(|n| n.as_str() == name.as_ref())
                            .unwrap_or(0);
                        // Desugar: obj.#name += rhs
                        // [obj, obj] → LoadPrivateProperty → [obj, value] → binop
                        // → park result → StorePrivateProperty → restore result
                        let temp_name = format!("__cmp_{}", self.locals.len());
                        self.locals.push(temp_name);
                        let temp_slot = self.locals.len() - 1;
                        self.emit_expression(obj);
                        self.emit_expression(obj);
                        self.emit(Opcode::LoadPrivateProperty, vec![slot_idx as i64]);
                        self.emit_expression(rhs);
                        self.emit(bin_opcode, vec![]);
                        self.emit(Opcode::StoreLocal, vec![temp_slot as i64]);
                        self.emit(Opcode::StorePrivateProperty, vec![slot_idx as i64]);
                        self.emit(Opcode::Pop, vec![]);
                        self.emit(Opcode::LoadLocal, vec![temp_slot as i64]);
                    }
                    _ => {}
                }
            }
            Expr::Array(elems, _) => {
                self.emit(Opcode::NewArray, vec![0]);
                for elem in elems {
                    if elem.is_spread {
                        self.emit_expression(&elem.expr);
                        self.emit(Opcode::ToArrayFromIterable, vec![]);
                        self.emit(Opcode::ArrayExtend, vec![]);
                    } else {
                        self.emit_expression(&elem.expr);
                        self.emit(Opcode::ArrayPush, vec![]);
                    }
                }
            }
            Expr::Object(props, _) => {
                let mut has_spread_or_computed = false;
                for prop in props {
                    if prop.is_spread
                        || prop.is_getter
                        || prop.is_setter
                        || matches!(prop.key, PropKey::Computed(_))
                    {
                        has_spread_or_computed = true;
                        break;
                    }
                }
                if has_spread_or_computed {
                    self.emit(Opcode::NewObject, vec![0]);
                    for prop in props {
                        if prop.is_spread {
                            self.emit_expression(&prop.value);
                            self.emit(Opcode::SpreadIntoObject, vec![]);
                        } else if prop.is_getter || prop.is_setter {
                            // Accessor: { get k() {}, set k(v) {} } → DefineAccessor.
                            // Stack: [obj, getter, setter] (+key for computed).
                            // DefineAccessor pops setter, getter, key?, obj.
                            self.emit(Opcode::Dup, vec![]);
                            if let PropKey::Computed(key_expr) = &prop.key {
                                self.emit_expression(key_expr);
                            }
                            if prop.is_getter {
                                self.emit_expression(&prop.value);
                                self.emit(Opcode::LoadUndefined, vec![]);
                            } else {
                                self.emit(Opcode::LoadUndefined, vec![]);
                                self.emit_expression(&prop.value);
                            }
                            if matches!(prop.key, PropKey::Computed(_)) {
                                self.emit(Opcode::DefineAccessor, vec![usize::MAX as i64]);
                            } else {
                                let key_str = match &prop.key {
                                    PropKey::String(s) => s.to_string(),
                                    PropKey::Identifier(s) => s.to_string(),
                                    PropKey::Number(n) => n.to_string(),
                                    PropKey::Computed(_) => unreachable!(),
                                };
                                let idx = self.intern_string(&key_str) as i64;
                                self.emit(Opcode::DefineAccessor, vec![idx]);
                            }
                            self.emit(Opcode::Pop, vec![]);
                        } else if matches!(prop.key, PropKey::Computed(_)) {
                            self.emit(Opcode::Dup, vec![]);
                            if let PropKey::Computed(key_expr) = &prop.key {
                                self.emit_expression(key_expr);
                            }
                            self.emit_expression(&prop.value);
                            self.emit(Opcode::StoreProperty, vec![]);
                            self.emit(Opcode::Pop, vec![]);
                        } else {
                            self.emit_expression(&prop.value);
                            let key_str = match &prop.key {
                                PropKey::String(s) => s.to_string(),
                                PropKey::Identifier(s) => s.to_string(),
                                PropKey::Number(n) => n.to_string(),
                                PropKey::Computed(_) => unreachable!(),
                            };
                            let idx = self.intern_string(&key_str) as i64;
                            self.emit(Opcode::DefineProperty, vec![idx]);
                        }
                    }
                } else {
                    let count = props.len() as i64;
                    for prop in props {
                        self.emit_expression(&prop.value);
                    }
                    let mut operands = vec![count];
                    for prop in props {
                        let key_str = match &prop.key {
                            PropKey::String(s) => s.to_string(),
                            PropKey::Identifier(s) => s.to_string(),
                            PropKey::Number(n) => n.to_string(),
                            PropKey::Computed(_) => unreachable!(),
                        };
                        let idx = self.intern_string(&key_str) as i64;
                        operands.push(idx);
                    }
                    self.emit(Opcode::NewObject, operands);
                }
            }
            Expr::Function(func, _) => {
                let func_idx = self.compile_function(func) as i64;
                let flags = if func.is_arrow { 1 } else { 0 };
                self.emit(Opcode::MakeFunction, vec![func_idx, flags]);
            }
            Expr::Yield(arg, _) => {
                if let Some(val) = arg {
                    self.emit_expression(val);
                } else {
                    self.emit(Opcode::LoadUndefined, vec![]);
                }
                self.emit(Opcode::Yield, vec![]);
            }
            Expr::Await(arg, _) => {
                self.emit_expression(arg);
                self.emit(Opcode::Await, vec![]);
            }
            Expr::RegExp(pattern, flags, _) => {
                let idx = self.regex_pool.len();
                self.regex_pool
                    .push((pattern.to_string(), flags.to_string()));
                self.emit(Opcode::LoadRegExp, vec![idx as i64]);
            }
            Expr::Super(_) => {
                self.emit(Opcode::LoadSuperclass, vec![]);
            }
            Expr::Class(class, _) => {
                self.emit_class(class, true);
            }
        }
    }

    /// Emit a short-circuit compound assignment: `a &&= b`, `a ||= b`, `a ??= b`.
    /// Desugar: `a &&= b` ≡ `a ? (a = b) : a`, `a ||= b` ≡ `a ? a : (a = b)`,
    /// `a ??= b` ≡ `a ?? nullish ? (a = b) : a`.
    /// The target read keeps the store-setup off the stack until the assign
    /// path, so the short-circuit path leaves exactly the current value.
    fn emit_short_circuit_assign(&mut self, op: &BinaryOp, target: &Expr, rhs: &Expr) {
        let check_opcode = match op {
            BinaryOp::LogicalAnd => Opcode::JumpIfFalse,
            BinaryOp::LogicalOr => Opcode::JumpIfTrue,
            BinaryOp::NullishCoalescing => Opcode::JumpIfNullOrUndefined,
            _ => return,
        };
        let is_nullish = *op == BinaryOp::NullishCoalescing;

        match target {
            Expr::Identifier(name, _) => {
                self.emit_target_read(name);
                self.emit(Opcode::Dup, vec![]);
                if is_nullish {
                    let assign = self.current();
                    self.emit(check_opcode, vec![0]);
                    let end = self.current();
                    self.emit(Opcode::Jump, vec![0]);
                    self.patch(assign, self.current());
                    self.emit(Opcode::Pop, vec![]);
                    self.emit_expression(rhs);
                    self.emit_target_store(name);
                    self.patch(end, self.current());
                } else {
                    let end = self.current();
                    self.emit(check_opcode, vec![0]);
                    self.emit(Opcode::Pop, vec![]);
                    self.emit_expression(rhs);
                    self.emit_target_store(name);
                    self.patch(end, self.current());
                }
            }
            Expr::Member(obj, prop, computed, _) => {
                self.emit_expression(obj);
                self.emit_property_key(prop, *computed);
                self.emit(Opcode::LoadProperty, vec![]);
                self.emit(Opcode::Dup, vec![]);
                if is_nullish {
                    let assign = self.current();
                    self.emit(check_opcode, vec![0]);
                    let end = self.current();
                    self.emit(Opcode::Jump, vec![0]);
                    self.patch(assign, self.current());
                    self.emit(Opcode::Pop, vec![]);
                    self.emit_expression(obj);
                    self.emit_property_key(prop, *computed);
                    self.emit_expression(rhs);
                    self.emit(Opcode::StoreProperty, vec![]);
                    self.patch(end, self.current());
                } else {
                    let end = self.current();
                    self.emit(check_opcode, vec![0]);
                    self.emit(Opcode::Pop, vec![]);
                    self.emit_expression(obj);
                    self.emit_property_key(prop, *computed);
                    self.emit_expression(rhs);
                    self.emit(Opcode::StoreProperty, vec![]);
                    self.patch(end, self.current());
                }
            }
            Expr::PrivateMember(obj, name, _) => {
                // Park the object and the read value; StorePrivateProperty
                // pops obj+value and pushes nothing.
                let slot_idx = self
                    .private_field_names
                    .iter()
                    .position(|n| n.as_str() == name.as_ref())
                    .unwrap_or(0);
                let obj_name = format!("__sc_{}", self.locals.len());
                self.locals.push(obj_name);
                let obj_slot = self.locals.len() - 1;
                let res_name = format!("__sc_{}", self.locals.len());
                self.locals.push(res_name);
                let res_slot = self.locals.len() - 1;
                self.emit_expression(obj);
                self.emit(Opcode::Dup, vec![]);
                self.emit(Opcode::StoreLocal, vec![obj_slot as i64]);
                self.emit(Opcode::LoadPrivateProperty, vec![slot_idx as i64]);
                self.emit(Opcode::Dup, vec![]);
                self.emit(Opcode::StoreLocal, vec![res_slot as i64]);
                let assign = self.current();
                self.emit(check_opcode, vec![0]);
                let end = self.current();
                self.emit(Opcode::Jump, vec![0]);
                // Assign path: [o, o, v]
                self.patch(assign, self.current());
                self.emit(Opcode::Pop, vec![]);
                self.emit(Opcode::Pop, vec![]);
                self.emit(Opcode::Pop, vec![]);
                self.emit(Opcode::LoadLocal, vec![obj_slot as i64]);
                self.emit_expression(rhs);
                self.emit(Opcode::StoreLocal, vec![res_slot as i64]);
                self.emit(Opcode::StorePrivateProperty, vec![slot_idx as i64]);
                // Shared epilogue: both paths leave one result.
                self.patch(end, self.current());
                self.emit(Opcode::LoadLocal, vec![res_slot as i64]);
            }
            _ => {
                self.emit_expression(rhs);
            }
        }
    }

    /// Read the current value of an identifier target (assignment LHS).
    fn emit_target_read(&mut self, name: &str) {
        if let Some(env_slot) = self.captured_slot(name) {
            self.emit(Opcode::LoadCaptured, vec![0, env_slot as i64]);
        } else if let Some((depth, slot)) = self.env_captured_slot(name) {
            self.emit(Opcode::LoadCaptured, vec![depth as i64, slot as i64]);
        } else if let Some(slot) = self.lexical_slot(name) {
            self.emit(Opcode::LoadLexical, vec![slot as i64]);
        } else if let Some(idx) = self.local_index(name) {
            self.emit(Opcode::LoadLocal, vec![idx as i64]);
        } else if self.module_mode {
            if let Some(import_idx) = self.module_imports.get(name) {
                self.emit(Opcode::LoadModuleImport, vec![*import_idx as i64]);
            } else {
                let name_idx = self.intern_string(name) as i64;
                self.emit(Opcode::LoadGlobal, vec![name_idx]);
            }
        } else {
            let name_idx = self.intern_string(name) as i64;
            self.emit(Opcode::LoadGlobal, vec![name_idx]);
        }
    }

    /// Store a value to an identifier target (assignment LHS).
    /// Store a destructuring-assignment binding, consuming the value.
    /// (StoreLocal/StoreGlobal push the value back; StoreCaptured/StoreLexical
    /// pop it, so only the net-0 stores need a trailing Pop.)
    fn emit_assign_store(&mut self, name: &str) {
        if let Some(env_slot) = self.captured_slot(name) {
            self.emit(Opcode::StoreCaptured, vec![0, env_slot as i64]);
        } else if let Some((depth, slot)) = self.env_captured_slot(name) {
            self.emit(Opcode::StoreCaptured, vec![depth as i64, slot as i64]);
        } else if let Some(slot) = self.lexical_slot(name) {
            self.emit(Opcode::StoreLexical, vec![slot as i64]);
        } else if let Some(idx) = self.local_index(name) {
            self.emit(Opcode::StoreLocal, vec![idx as i64]);
            self.emit(Opcode::Pop, vec![]);
        } else if self.module_mode {
            if let Some(import_idx) = self.module_imports.get(name) {
                // Assignment to an imported binding is a TypeError at runtime.
                self.emit(Opcode::StoreModuleImport, vec![*import_idx as i64]);
                self.emit(Opcode::Pop, vec![]);
            } else {
                let name_idx = self.intern_string(name) as i64;
                self.emit(Opcode::StoreGlobal, vec![name_idx]);
                self.emit(Opcode::Pop, vec![]);
                self.emit_module_rename_sync(name);
            }
        } else {
            let name_idx = self.intern_string(name) as i64;
            self.emit(Opcode::StoreGlobal, vec![name_idx]);
            self.emit(Opcode::Pop, vec![]);
        }
    }

    fn emit_target_store(&mut self, name: &str) {
        if let Some(env_slot) = self.captured_slot(name) {
            self.emit(Opcode::StoreCaptured, vec![0, env_slot as i64]);
        } else if let Some((depth, slot)) = self.env_captured_slot(name) {
            self.emit(Opcode::StoreCaptured, vec![depth as i64, slot as i64]);
        } else if let Some(slot) = self.lexical_slot(name) {
            self.emit(Opcode::StoreLexical, vec![slot as i64]);
        } else if let Some(idx) = self.local_index(name) {
            self.emit(Opcode::StoreLocal, vec![idx as i64]);
        } else if self.module_mode {
            if let Some(import_idx) = self.module_imports.get(name) {
                self.emit(Opcode::StoreModuleImport, vec![*import_idx as i64]);
            } else {
                let name_idx = self.intern_string(name) as i64;
                self.emit(Opcode::StoreGlobal, vec![name_idx]);
                self.emit_module_rename_sync(name);
            }
        } else {
            let name_idx = self.intern_string(name) as i64;
            self.emit(Opcode::StoreGlobal, vec![name_idx]);
        }
    }

    /// Push the property key for a member access target.
    fn emit_property_key(&mut self, prop: &Expr, computed: bool) {
        if computed {
            self.emit_expression(prop);
        } else {
            let name = prop_name_as_string(prop);
            let idx = self.intern_string(&name) as i64;
            self.emit(Opcode::LoadStringConst, vec![idx]);
        }
    }

    // ---- Lexical scope helpers ----

    /// Count the number of direct `let`/`const` declarations in a list of statements
    /// (does not recurse into nested blocks). Handles destructuring patterns.
    fn count_lexicals(&mut self, stmts: &[Stmt]) -> usize {
        stmts
            .iter()
            .filter(|s| matches!(s, Stmt::Var(VarKind::Let | VarKind::Const, _, _)))
            .map(|s| match s {
                Stmt::Var(_, decls, _) => decls
                    .iter()
                    .map(|d| self.count_pattern_bindings(&d.pattern))
                    .sum(),
                _ => 0,
            })
            .sum()
    }

    /// Count the number of binding identifiers in a pattern (1 if None = simple identifier).
    #[allow(clippy::only_used_in_recursion)]
    fn count_pattern_bindings(&self, pattern: &Option<Pattern>) -> usize {
        match pattern {
            None => 1,
            Some(Pattern::Object(props, rest, _)) => {
                let mut count: usize = props
                    .iter()
                    .map(|p| self.count_pattern_bindings(&Some(p.pattern.clone())))
                    .sum();
                if let Some(rest) = rest {
                    count += self.count_pattern_bindings(&Some((**rest).clone()));
                }
                count
            }
            Some(Pattern::Array(items, _)) => items
                .iter()
                .map(|item| match item {
                    Some(p) => self.count_pattern_bindings(&Some(p.clone())),
                    None => 0,
                })
                .sum(),
            Some(Pattern::Identifier(_, _, _)) => 1,
            Some(Pattern::Default(p, _)) => self.count_pattern_bindings(&Some((**p).clone())),
            Some(Pattern::Rest(inner, _)) => self.count_pattern_bindings(&Some((**inner).clone())),
        }
    }

    /// Enter a lexical scope: register all direct `let`/`const` bindings
    /// (including destructured bindings) and assign them absolute slot indices.
    fn enter_lexical_scope(&mut self, stmts: &[Stmt], _count: usize) {
        let mut bindings = Vec::new();
        for stmt in stmts {
            if let Stmt::Var(kind, decls, _) = stmt {
                if matches!(kind, VarKind::Let | VarKind::Const) {
                    for decl in decls {
                        self.collect_lexical_bindings(&decl.pattern, &decl.name, &mut bindings);
                    }
                }
            }
        }
        self.lexical_slot_count += bindings.len();
        self.lexical_scopes.push(bindings);
    }

    /// Collect all binding names from a pattern into the bindings vector.
    /// For simple declarations (pattern is None), uses the decl name directly.
    fn collect_lexical_bindings(
        &self,
        pattern: &Option<Pattern>,
        name: &str,
        bindings: &mut Vec<LexicalBinding>,
    ) {
        match pattern {
            None => {
                bindings.push(LexicalBinding {
                    name: name.to_string(),
                    slot: self.lexical_slot_count + bindings.len(),
                });
            }
            Some(Pattern::Object(props, rest, _)) => {
                for prop in props {
                    self.collect_lexical_bindings(&Some(prop.pattern.clone()), name, bindings);
                }
                if let Some(rest) = rest {
                    self.collect_lexical_bindings(&Some((**rest).clone()), name, bindings);
                }
            }
            Some(Pattern::Array(items, _)) => {
                for pattern in items.iter().flatten() {
                    self.collect_lexical_bindings(&Some(pattern.clone()), name, bindings);
                }
            }
            Some(Pattern::Identifier(n, _, _)) => {
                bindings.push(LexicalBinding {
                    name: n.to_string(),
                    slot: self.lexical_slot_count + bindings.len(),
                });
            }
            Some(Pattern::Default(p, _)) => {
                self.collect_lexical_bindings(&Some((**p).clone()), name, bindings);
            }
            Some(Pattern::Rest(inner, _)) => {
                self.collect_lexical_bindings(&Some((**inner).clone()), name, bindings);
            }
        }
    }

    /// Leave the current lexical scope.
    fn leave_lexical_scope(&mut self) {
        if let Some(scope) = self.lexical_scopes.pop() {
            self.lexical_slot_count -= scope.len();
        }
    }

    /// Look up a name in the lexical scope stack.
    /// Returns Some(absolute_slot) if found, None if not lexical.
    fn lexical_slot(&self, name: &str) -> Option<usize> {
        for scope in self.lexical_scopes.iter().rev() {
            for binding in scope.iter() {
                if binding.name == name {
                    return Some(binding.slot);
                }
            }
        }
        None
    }

    fn local_index(&self, name: &str) -> Option<usize> {
        self.locals.iter().position(|l| l == name)
    }

    /// Return the env slot index if `name` is captured by THIS function.
    fn captured_slot(&self, name: &str) -> Option<usize> {
        self.captured_names.iter().position(|n| n == name)
    }

    /// Return (depth, slot) if `name` is captured by an ANCESTOR function's env.
    /// depth 0 = parent, 1 = grandparent, etc.
    fn env_captured_slot(&self, name: &str) -> Option<(usize, usize)> {
        // Walk from closest ancestor (last in vec) to farthest (first in vec)
        let len = self.env_scope_stack.len();
        for (i, names) in self.env_scope_stack.iter().enumerate().rev() {
            if let Some(slot) = names.iter().position(|n| n == name) {
                return Some((len - 1 - i, slot));
            }
        }
        None
    }

    pub fn into_bytecode(self) -> BytecodeProgram {
        let mut instructions = Vec::new();
        if self.is_generator {
            instructions.push(Instruction::new(Opcode::InitGenerator, vec![]));
        }
        instructions.extend(self.instructions);
        let mut program = BytecodeProgram::new(instructions, self.string_pool, self.nested_funcs);
        program.named_function = self.named_function;
        program.is_generator = self.is_generator;
        program.is_async = self.is_async;
        program.local_names = self.locals;
        program.captured_env_size = self.captured_env_size;
        program.float_pool = self.float_pool;
        program.regex_pool = self.regex_pool;
        program.assign_ic_indices();
        program
    }
}

/// Recursively check if a statement contains any inner function or arrow.
fn contains_inner_function_stmt(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Expr(expr, _) => contains_inner_function_expr(expr),
        Stmt::Import(_, _) | Stmt::Export(_, _) => false,
        Stmt::Return(Some(expr), _) => contains_inner_function_expr(expr),
        Stmt::Throw(expr, _) => contains_inner_function_expr(expr),
        Stmt::Block(stmts, _) => stmts.iter().any(contains_inner_function_stmt),
        Stmt::Var(_, decls, _) => decls.iter().any(|d| {
            d.init
                .as_ref()
                .is_some_and(|e| contains_inner_function_expr(e))
        }),
        Stmt::If(cond, then, else_, _) => {
            contains_inner_function_expr(cond)
                || contains_inner_function_stmt(then)
                || else_.as_deref().is_some_and(contains_inner_function_stmt)
        }
        Stmt::While(cond, body, _) => {
            contains_inner_function_expr(cond) || contains_inner_function_stmt(body)
        }
        Stmt::DoWhile(cond, body, _) => {
            contains_inner_function_expr(cond) || contains_inner_function_stmt(body)
        }
        Stmt::For(init, cond, update, body, _) => {
            init.as_deref().is_some_and(contains_inner_function_stmt)
                || cond.as_deref().is_some_and(contains_inner_function_expr)
                || update.as_deref().is_some_and(contains_inner_function_expr)
                || contains_inner_function_stmt(body)
        }
        Stmt::ForIn(_, _, body, _) => contains_inner_function_stmt(body),
        Stmt::ForOf(_, _, body, _) => contains_inner_function_stmt(body),
        Stmt::Switch(target, cases, default_body, _) => {
            contains_inner_function_expr(target)
                || cases.iter().any(|c| {
                    contains_inner_function_expr(&c.test)
                        || c.body.iter().any(contains_inner_function_stmt)
                })
                || default_body
                    .as_deref()
                    .is_some_and(|stmts| stmts.iter().any(contains_inner_function_stmt))
        }
        Stmt::Try(body, catch, finally, _) => {
            body.iter().any(contains_inner_function_stmt)
                || catch
                    .as_ref()
                    .is_some_and(|c| c.body.iter().any(contains_inner_function_stmt))
                || finally
                    .as_deref()
                    .is_some_and(|stmts| stmts.iter().any(contains_inner_function_stmt))
        }
        Stmt::Function(_, _) => true,
        Stmt::Class(_, _) => true,
        Stmt::Break(_, _) | Stmt::Continue(_, _) | Stmt::Return(None, _) | Stmt::Empty(_) => false,
    }
}

/// Recursively check if an expression contains any inner function or arrow.
fn contains_inner_function_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Function(_, _) => true,
        Expr::Call(callee, args, _) => {
            contains_inner_function_expr(callee)
                || args.iter().any(|a| contains_inner_function_expr(&a.expr))
        }
        Expr::New(callee, args, _) => {
            contains_inner_function_expr(callee)
                || args.iter().any(|a| contains_inner_function_expr(&a.expr))
        }
        Expr::Member(obj, prop, _, _) => {
            contains_inner_function_expr(obj) || contains_inner_function_expr(prop)
        }
        Expr::PrivateMember(obj, _, _) => contains_inner_function_expr(obj),
        Expr::OptionalChain(base, links, _) => {
            contains_inner_function_expr(base)
                || links.iter().any(|l| match &l.kind {
                    OptionalLinkKind::Computed(e) => contains_inner_function_expr(e),
                    OptionalLinkKind::Call(args) => {
                        args.iter().any(|a| contains_inner_function_expr(&a.expr))
                    }
                    _ => false,
                })
        }
        Expr::Unary(_, arg, _) => contains_inner_function_expr(arg),
        Expr::Update(_, arg, _, _) => contains_inner_function_expr(arg),
        Expr::Binary(_, lhs, rhs, _) | Expr::CompoundAssign(_, lhs, rhs, _) => {
            contains_inner_function_expr(lhs) || contains_inner_function_expr(rhs)
        }
        Expr::DestructureAssign(_, rhs, _) => contains_inner_function_expr(rhs),
        Expr::Conditional(cond, then, else_, _) => {
            contains_inner_function_expr(cond)
                || contains_inner_function_expr(then)
                || contains_inner_function_expr(else_)
        }
        Expr::Array(elems, _) => elems.iter().any(|e| contains_inner_function_expr(&e.expr)),
        Expr::Object(props, _) => props.iter().any(|p| {
            let key_fn = match &p.key {
                PropKey::Computed(e) => contains_inner_function_expr(e),
                _ => false,
            };
            key_fn || contains_inner_function_expr(&p.value)
        }),
        Expr::Template { exprs, .. } => exprs.iter().any(contains_inner_function_expr),
        Expr::Identifier(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::Boolean(_, _)
        | Expr::Null(_)
        | Expr::Undefined(_)
        | Expr::This(_)
        | Expr::Assign(_, _, _)
        | Expr::Yield(_, _)
        | Expr::RegExp(_, _, _) => false,
        Expr::Super(_) => false,
        Expr::Class(_, _) => true,
        Expr::Await(expr, _) => contains_inner_function_expr(expr),
    }
}

/// Collect all `var` declaration names from a statement tree.
fn collect_var_names_stmt(stmt: &Stmt, names: &mut Vec<String>) {
    match stmt {
        Stmt::Var(VarKind::Var, decls, _) => {
            for d in decls {
                if !names.contains(&d.name.to_string()) {
                    names.push(d.name.to_string());
                }
            }
        }
        Stmt::Block(stmts, _) => stmts.iter().for_each(|s| collect_var_names_stmt(s, names)),
        Stmt::If(_, then, else_, _) => {
            collect_var_names_stmt(then, names);
            if let Some(s) = else_ {
                collect_var_names_stmt(s, names);
            }
        }
        Stmt::While(_, body, _) => collect_var_names_stmt(body, names),
        Stmt::DoWhile(_, body, _) => collect_var_names_stmt(body, names),
        Stmt::For(init, _, _, body, _) => {
            if let Some(s) = init {
                collect_var_names_stmt(s, names);
            }
            collect_var_names_stmt(body, names);
        }
        Stmt::ForIn(_, _, body, _) => collect_var_names_stmt(body, names),
        Stmt::ForOf(_, _, body, _) => collect_var_names_stmt(body, names),
        Stmt::Switch(_, cases, default, _) => {
            for c in cases {
                c.body.iter().for_each(|s| collect_var_names_stmt(s, names));
            }
            if let Some(stmts) = default {
                stmts.iter().for_each(|s| collect_var_names_stmt(s, names));
            }
        }
        Stmt::Try(body, catch, finally, _) => {
            body.iter().for_each(|s| collect_var_names_stmt(s, names));
            if let Some(c) = catch {
                c.body.iter().for_each(|s| collect_var_names_stmt(s, names));
            }
            if let Some(stmts) = finally {
                stmts.iter().for_each(|s| collect_var_names_stmt(s, names));
            }
        }
        _ => {}
    }
}

/// Check if a statement tree references the `arguments` identifier, skipping
/// nested non-arrow function declarations (which have their own `arguments`).
fn uses_arguments_stmt(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Expr(expr, _) => uses_arguments_expr(expr),
        Stmt::Import(_, _) | Stmt::Export(_, _) => false,
        Stmt::Block(stmts, _) => stmts.iter().any(uses_arguments_stmt),
        Stmt::If(cond, then, else_, _) => {
            uses_arguments_expr(cond)
                || uses_arguments_stmt(then)
                || else_.as_deref().is_some_and(uses_arguments_stmt)
        }
        Stmt::While(cond, body, _) => uses_arguments_expr(cond) || uses_arguments_stmt(body),
        Stmt::DoWhile(cond, body, _) => uses_arguments_expr(cond) || uses_arguments_stmt(body),
        Stmt::For(init, cond, update, body, _) => {
            let cond_uses = cond
                .as_ref()
                .map(|e| uses_arguments_expr(e))
                .unwrap_or(false);
            let update_uses = update
                .as_ref()
                .map(|e| uses_arguments_expr(e))
                .unwrap_or(false);
            (init.as_deref().is_some_and(uses_arguments_stmt))
                || cond_uses
                || update_uses
                || uses_arguments_stmt(body)
        }
        Stmt::ForIn(lhs, rhs, body, _) => {
            uses_arguments_expr(lhs) || uses_arguments_expr(rhs) || uses_arguments_stmt(body)
        }
        Stmt::ForOf(lhs, rhs, body, _) => {
            uses_arguments_expr(lhs) || uses_arguments_expr(rhs) || uses_arguments_stmt(body)
        }
        Stmt::Var(_, decls, _) => decls
            .iter()
            .any(|d| d.init.as_ref().is_some_and(|e| uses_arguments_expr(e))),
        Stmt::Return(expr, _) => expr
            .as_ref()
            .map(|e| uses_arguments_expr(e))
            .unwrap_or(false),
        Stmt::Throw(expr, _) => uses_arguments_expr(expr),
        Stmt::Break(_, _) | Stmt::Continue(_, _) => false,
        Stmt::Try(body, catch, finally, _) => {
            body.iter().any(uses_arguments_stmt)
                || catch
                    .as_ref()
                    .is_some_and(|c| c.body.iter().any(uses_arguments_stmt))
                || finally
                    .as_ref()
                    .is_some_and(|stmts| stmts.iter().any(uses_arguments_stmt))
        }
        Stmt::Switch(discr, cases, default, _) => {
            uses_arguments_expr(discr)
                || cases
                    .iter()
                    .any(|c| uses_arguments_expr(&c.test) || c.body.iter().any(uses_arguments_stmt))
                || default
                    .as_ref()
                    .is_some_and(|stmts| stmts.iter().any(uses_arguments_stmt))
        }
        Stmt::Function(fn_node, _) => {
            // Non-arrow functions have their own `arguments` — don't scan inside.
            // Arrow functions inherit `arguments` from the enclosing scope.
            if fn_node.is_arrow {
                uses_arguments_stmt(&fn_node.body)
            } else {
                false
            }
        }
        Stmt::Class(_, _) => false,
        Stmt::Empty(_) => false,
    }
}

/// Check if an expression references the `arguments` identifier.
fn uses_arguments_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Identifier(name, _) => name.as_ref() == "arguments",
        Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::Boolean(_, _)
        | Expr::Null(_)
        | Expr::Undefined(_)
        | Expr::This(_) => false,
        Expr::Array(elements, _) => elements.iter().any(|e| uses_arguments_expr(&e.expr)),
        Expr::Object(props, _) => props.iter().any(|p| uses_arguments_expr(&p.value)),
        Expr::Unary(_, expr, _) => uses_arguments_expr(expr),
        Expr::Binary(_, left, right, _) => uses_arguments_expr(left) || uses_arguments_expr(right),
        Expr::Conditional(cond, then, else_, _) => {
            uses_arguments_expr(cond) || uses_arguments_expr(then) || uses_arguments_expr(else_)
        }
        Expr::Call(callee, args, _) => {
            uses_arguments_expr(callee) || args.iter().any(|a| uses_arguments_expr(&a.expr))
        }
        Expr::New(callee, args, _) => {
            uses_arguments_expr(callee) || args.iter().any(|a| uses_arguments_expr(&a.expr))
        }
        Expr::Member(obj, prop, _, _) => uses_arguments_expr(obj) || uses_arguments_expr(prop),
        Expr::PrivateMember(obj, _, _) => uses_arguments_expr(obj),
        Expr::OptionalChain(base, links, _) => {
            uses_arguments_expr(base)
                || links.iter().any(|l| match &l.kind {
                    OptionalLinkKind::Computed(e) => uses_arguments_expr(e),
                    OptionalLinkKind::Call(args) => {
                        args.iter().any(|a| uses_arguments_expr(&a.expr))
                    }
                    _ => false,
                })
        }
        Expr::Assign(lhs, rhs, _) => uses_arguments_expr(lhs) || uses_arguments_expr(rhs),
        Expr::CompoundAssign(_, lhs, rhs, _) => {
            uses_arguments_expr(lhs) || uses_arguments_expr(rhs)
        }
        Expr::DestructureAssign(_, rhs, _) => uses_arguments_expr(rhs),
        Expr::Function(fn_node, _) => {
            // Non-arrow function expressions have their own `arguments`.
            // Arrow function expressions inherit `arguments` from enclosing scope.
            if fn_node.is_arrow {
                uses_arguments_stmt(&fn_node.body)
            } else {
                false
            }
        }
        Expr::Template { exprs, .. } => exprs.iter().any(uses_arguments_expr),
        Expr::Update(_, expr, _, _) => uses_arguments_expr(expr),
        Expr::Yield(expr, _) => expr.as_ref().is_some_and(|e| uses_arguments_expr(e)),
        Expr::Await(expr, _) => uses_arguments_expr(expr),
        Expr::Super(_) => false,
        Expr::RegExp(_, _, _) => false,
        Expr::Class(_, _) => false,
    }
}

/// Extract the string name from a property expression in dot access.
fn prop_name_as_string(expr: &Expr) -> String {
    match expr {
        Expr::Identifier(name, _) => name.to_string(),
        Expr::String(s, _) => s.to_string(),
        _ => String::new(),
    }
}

/// Map a CompoundAssign BinaryOp to the bytecode opcode for the underlying operation.
fn compound_binary_opcode(op: BinaryOp) -> Opcode {
    match op {
        BinaryOp::Add => Opcode::Add,
        BinaryOp::Sub => Opcode::Sub,
        BinaryOp::Mul => Opcode::Mul,
        BinaryOp::Div => Opcode::Div,
        BinaryOp::Mod => Opcode::Mod,
        BinaryOp::Exp => Opcode::Exp,
        BinaryOp::Shl => Opcode::Shl,
        BinaryOp::Shr => Opcode::Shr,
        BinaryOp::ShrU => Opcode::ShrU,
        BinaryOp::BitAnd => Opcode::BitAnd,
        BinaryOp::BitOr => Opcode::BitOr,
        BinaryOp::BitXor => Opcode::BitXor,
        _ => Opcode::Add,
    }
}
