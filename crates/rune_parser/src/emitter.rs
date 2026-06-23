use crate::ast::*;
use rune_bytecode::opcode::{BytecodeProgram, Instruction, Opcode};

/// A lexical scope binding with its allocated absolute slot index.
struct LexicalBinding {
    name: String,
    slot: usize,
}

/// Bytecode emitter. Walks an AST and produces instructions.
pub struct Emitter {
    pub instructions: Vec<Instruction>,
    pub is_generator: bool,
    pub named_function: bool,
    pub string_pool: Vec<String>,
    pub float_pool: Vec<f64>,
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
    switch_exit_stack: Vec<usize>,
    switch_break_jumps: Vec<usize>,
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
            named_function: false,
            string_pool: Vec::new(),
            float_pool: Vec::new(),
            nested_funcs: Vec::new(),
            locals: Vec::new(),
            lexical_scopes: Vec::new(),
            lexical_slot_count: 0,
            captured_names: Vec::new(),
            captured_env_size: 0,
            env_scope_stack: Vec::new(),
            loop_exit_stack: Vec::new(),
            loop_cont_stack: Vec::new(),
            switch_exit_stack: Vec::new(),
            switch_break_jumps: Vec::new(),
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

    /// Compile a function node into a nested BytecodeProgram and return its index.
    fn compile_function(&mut self, func: &FnNode) -> usize {
        let mut sub = Emitter::new();
        sub.env_scope_stack = self.env_scope_stack.clone();
        sub.is_generator = func.is_generator;
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
                        sub.emit_store_with_default(name.as_ref(), &VarKind::Var, default);
                    }
                }
                _ => {
                    sub.emit(Opcode::LoadLocal, vec![param_idx as i64]);
                    sub.emit_destructuring(param, &VarKind::Var);
                }
            }
        }
        // For non-arrow functions: materialize `arguments` object
        if !func.is_arrow {
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

    /// Emit bytecode for destructuring a value according to the pattern.
    fn emit_destructuring(&mut self, pattern: &Pattern, kind: &VarKind) {
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
    fn emit_destructuring_binding(&mut self, pattern: &Pattern, kind: &VarKind) {
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
    fn emit_store_with_default(&mut self, name: &str, kind: &VarKind, default: &Option<Box<Expr>>) {
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
    fn emit_store_binding(&mut self, name: &str, kind: &VarKind) {
        match kind {
            VarKind::Var => {
                if !self.locals.contains(&name.to_string()) {
                    self.locals.push(name.to_string());
                }
                if let Some((depth, env_slot)) = self.env_captured_slot(name) {
                    // StoreCaptured pops the value — no Pop needed
                    self.emit(Opcode::StoreCaptured, vec![depth as i64, env_slot as i64]);
                } else if let Some(idx) = self.local_index(name) {
                    self.emit(Opcode::StoreLocal, vec![idx as i64]);
                    self.emit(Opcode::Pop, vec![]);
                }
            }
            VarKind::Let | VarKind::Const => {
                if let Some(slot) = self.lexical_slot(name) {
                    let op = if *kind == VarKind::Const {
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
            Stmt::Block(stmts, _) => {
                let lexical_count = self.count_lexicals(stmts);
                if lexical_count > 0 {
                    self.enter_lexical_scope(stmts, lexical_count);
                    self.emit(Opcode::BlockEnter, vec![lexical_count as i64]);
                }
                for s in stmts {
                    self.emit_statement(s);
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
                self.loop_exit_stack.push(exit_jump);
                self.loop_cont_stack.push(loop_start);
                self.emit_statement(body);
                self.loop_cont_stack.pop();
                self.loop_exit_stack.pop();
                self.emit(Opcode::Jump, vec![loop_start as i64]);
                self.patch(exit_jump, self.current());
            }
            Stmt::DoWhile(cond, body, _) => {
                let loop_start = self.current();
                self.loop_cont_stack.push(loop_start);
                self.loop_exit_stack.push(0);
                self.emit_statement(body);
                let exit_jump = self.loop_exit_stack.pop().unwrap();
                self.loop_cont_stack.pop();
                self.emit_expression(cond);
                self.emit(Opcode::JumpIfTrue, vec![loop_start as i64]);
                if exit_jump != 0 {
                    self.patch(exit_jump, self.current());
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
                if init_has_scope && let Some(scope) = self.lexical_scopes.last() {
                    for b in scope {
                        per_iteration_vars.push((b.name.clone(), b.slot));
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
                self.loop_exit_stack.push(exit_jump);
                self.loop_cont_stack.push(loop_start);
                self.emit_statement(body);
                self.loop_cont_stack.pop();
                self.loop_exit_stack.pop();
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
                // Exit path (JumpIfFalse lands here): restore env before leaving
                if per_iteration_count > 0 {
                    self.patch(exit_jump, self.current());
                    self.emit(Opcode::RestoreEnv, vec![]);
                } else {
                    self.patch(exit_jump, self.current());
                }
                // Leave for-init lexical scope
                if init_has_scope {
                    self.emit(Opcode::BlockLeave, vec![]);
                    self.leave_lexical_scope();
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
                                self.emit_destructuring(pattern, kind);
                            }
                        } else {
                            if !self.locals.contains(&decl.name.to_string()) {
                                self.locals.push(decl.name.to_string());
                            }
                            if let Some(init) = &decl.init {
                                self.emit_expression(init);
                                if let Some((depth, env_slot)) = self.env_captured_slot(&decl.name) {
                                    // StoreCaptured pops the value — no Pop needed
                                    self.emit(Opcode::StoreCaptured, vec![depth as i64, env_slot as i64]);
                                } else if let Some(idx) = self.local_index(&decl.name) {
                                    self.emit(Opcode::StoreLocal, vec![idx as i64]);
                                    self.emit(Opcode::Pop, vec![]);
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
                                self.emit_destructuring(pattern, kind);
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
                    self.emit(Opcode::Jump, vec![*exit as i64]);
                }
            }
            Stmt::Continue(_label, _) => {
                if let Some(cont) = self.loop_cont_stack.last() {
                    self.emit(Opcode::Jump, vec![*cont as i64]);
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
                if let Expr::Identifier(name, _) = lhs.as_ref()
                    && !self.locals.contains(&name.to_string())
                {
                    self.locals.push(name.to_string());
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
                _ => {
                    self.emit_expression(arg);
                    self.emit(Opcode::Pop, vec![]);
                    self.emit(Opcode::LoadUndefined, vec![]);
                }
            },
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
                    BinaryOp::LogicalAnd | BinaryOp::LogicalOr | BinaryOp::Assign => unreachable!(),
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
                            // stack: [receiver, method]
                            self.emit(Opcode::NewArray, vec![0]);
                            for arg in args {
                                self.emit_expression(&arg.expr);
                                if arg.is_spread {
                                    self.emit(Opcode::ArrayExtend, vec![]);
                                } else {
                                    self.emit(Opcode::ArrayPush, vec![]);
                                }
                            }
                            // stack: [receiver, method, args_array] — correct for CallFromArray
                        }
                        _ => {
                            self.emit(Opcode::LoadUndefined, vec![]);
                            self.emit_expression(callee);
                            // stack: [this, callee]
                            self.emit(Opcode::NewArray, vec![0]);
                            for arg in args {
                                self.emit_expression(&arg.expr);
                                if arg.is_spread {
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
            Expr::Member(obj, prop, computed, _) => {
                self.emit_expression(obj);
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
                        self.emit(Opcode::StoreCaptured, vec![depth as i64, slot as i64]);
                    } else if let Some(slot) = self.lexical_slot(name) {
                        self.emit(Opcode::StoreLexical, vec![slot as i64]);
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
                    self.emit_expression(value);
                    self.emit(Opcode::StoreProperty, vec![]);
                }
                _ => {
                    self.emit_expression(value);
                }
            },
            Expr::CompoundAssign(op, target, rhs, _) => {
                let bin_opcode = compound_binary_opcode(*op);
                match target.as_ref() {
                    Expr::Identifier(name, _) => {
                        if let Some((depth, slot)) = self.env_captured_slot(name) {
                            self.emit(Opcode::LoadCaptured, vec![depth as i64, slot as i64]);
                            self.emit_expression(rhs);
                            self.emit(bin_opcode, vec![]);
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
                    _ => {}
                }
            }
            Expr::Array(elems, _) => {
                self.emit(Opcode::NewArray, vec![0]);
                for elem in elems {
                    if elem.is_spread {
                        self.emit_expression(&elem.expr);
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
                    if prop.is_spread || matches!(prop.key, PropKey::Computed(_)) {
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
            if let Stmt::Var(kind, decls, _) = stmt
                && matches!(kind, VarKind::Let | VarKind::Const)
            {
                for decl in decls {
                    self.collect_lexical_bindings(&decl.pattern, &decl.name, &mut bindings);
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
        program.local_names = self.locals;
        program.captured_env_size = self.captured_env_size;
        program.float_pool = self.float_pool;
        program.assign_ic_indices();
        program
    }
}

/// Recursively check if a statement contains any inner function or arrow.
fn contains_inner_function_stmt(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Expr(expr, _) => contains_inner_function_expr(expr),
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
                || update
                    .as_deref()
                    .is_some_and(contains_inner_function_expr)
                || contains_inner_function_stmt(body)
        }
        Stmt::ForIn(_, _, body, _) => contains_inner_function_stmt(body),
        Stmt::Switch(target, cases, default_body, _) => {
            contains_inner_function_expr(target)
                || cases.iter().any(|c| {
                    contains_inner_function_expr(&c.test)
                        || c.body.iter().any(contains_inner_function_stmt)
                })
                || default_body.as_deref().is_some_and(|stmts| {
                    stmts.iter().any(contains_inner_function_stmt)
                })
        }
        Stmt::Try(body, catch, finally, _) => {
            body.iter().any(contains_inner_function_stmt)
                || catch
                    .as_ref()
                    .is_some_and(|c| c.body.iter().any(contains_inner_function_stmt))
                || finally.as_deref().is_some_and(|stmts| {
                    stmts.iter().any(contains_inner_function_stmt)
                })
        }
        Stmt::Function(_, _) => true,
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
        Expr::Unary(_, arg, _) => contains_inner_function_expr(arg),
        Expr::Update(_, arg, _, _) => contains_inner_function_expr(arg),
        Expr::Binary(_, lhs, rhs, _) | Expr::CompoundAssign(_, lhs, rhs, _) => {
            contains_inner_function_expr(lhs) || contains_inner_function_expr(rhs)
        }
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
        | Expr::Yield(_, _) => false,
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
