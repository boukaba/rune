use rune_bytecode::opcode::{BytecodeProgram, Instruction, Opcode};
use crate::ast::*;

/// Bytecode emitter. Walks an AST and produces instructions.
pub struct Emitter {
    pub instructions: Vec<Instruction>,
    pub is_generator: bool,
    pub named_function: bool,
    pub string_pool: Vec<String>,
    pub float_pool: Vec<f64>,
    pub nested_funcs: Vec<BytecodeProgram>,
    locals: Vec<String>,
    loop_exit_stack: Vec<usize>,
    loop_cont_stack: Vec<usize>,
    switch_exit_stack: Vec<usize>,
    switch_break_jumps: Vec<usize>,
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
        if let Some(idx) = self.float_pool.iter().position(|x| x.to_bits() == v.to_bits()) {
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
        let last_idx = prog.body.len() - 1;
        for stmt in &prog.body[..last_idx] {
            self.emit_statement(stmt);
        }
        self.emit_last_statement(&prog.body[last_idx]);
    }

    /// Compile a function node into a nested BytecodeProgram and return its index.
    fn compile_function(&mut self, func: &FnNode) -> usize {
        let mut sub = Emitter::new();
        sub.is_generator = func.is_generator;
        if let Some(name) = &func.name {
            sub.named_function = true;
            sub.locals.push(name.to_string());
        }
        for param in &func.params {
            sub.locals.push(param.to_string());
        }
        // Emit body (could be Block, Expr for arrow, or single statement)
        match &func.body {
            Stmt::Block(stmts, _) => {
                for stmt in stmts {
                    sub.emit_statement(stmt);
                }
            }
            other => {
                sub.emit_statement(other);
            }
        }
        // Add implicit undefined return if body doesn't end with Return
        let needs_return = match sub.instructions.last() {
            Some(last) => last.opcode != Opcode::Return,
            None => true,
        };
        if needs_return {
            sub.emit(Opcode::LoadUndefined, vec![]);
            sub.emit(Opcode::Return, vec![]);
        }
        let program = sub.into_bytecode();
        let idx = self.nested_funcs.len();
        self.nested_funcs.push(program);
        idx
    }

    fn emit_statement(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Expr(expr, _) => {
                self.emit_expression(expr);
                self.emit(Opcode::Pop, vec![]);
            }
            Stmt::Block(stmts, _) => {
                for s in stmts {
                    self.emit_statement(s);
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
                if let Some(init_stmt) = init {
                    self.emit_statement(init_stmt);
                }
                let loop_start = self.current();
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
                if let Some(upd) = update {
                    self.emit_expression(upd);
                    self.emit(Opcode::Pop, vec![]);
                }
                self.emit(Opcode::Jump, vec![loop_start as i64]);
                self.patch(exit_jump, self.current());
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
            Stmt::Var(_, decls, _) => {
                for decl in decls {
                    if !self.locals.contains(&decl.name.to_string()) {
                        self.locals.push(decl.name.to_string());
                    }
                    if let Some(init) = &decl.init {
                        self.emit_expression(init);
                        if let Some(idx) = self.local_index(&decl.name) {
                            self.emit(Opcode::StoreLocal, vec![idx as i64]);
                        }
                        self.emit(Opcode::Pop, vec![]);
                    }
                }
            }
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
                for stmt in body.iter() { self.emit_statement(stmt); }

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
                            self.emit(Opcode::StoreLocal, vec![self.local_index(&c.param).unwrap() as i64]);
                        }
                        self.emit(Opcode::Pop, vec![]);
                        for stmt in c.body.iter() { self.emit_statement(stmt); }
                        self.patch(past_catch, self.current());
                    }
                    (None, Some(fin)) => {
                        // try-finally (no catch) — no TryEnd, fall through to finally
                        let fin_entry = self.current();
                        self.patch_operand(try_idx, 1, fin_entry as i64);
                        for stmt in fin.iter() { self.emit_statement(stmt); }
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
                            self.emit(Opcode::StoreLocal, vec![self.local_index(&c.param).unwrap() as i64]);
                        }
                        self.emit(Opcode::Pop, vec![]);
                        for stmt in c.body.iter() { self.emit_statement(stmt); }
                        let fin_entry = self.current();
                        self.patch(past_catch, fin_entry);
                        self.patch_operand(try_idx, 1, fin_entry as i64);
                        for stmt in fin.iter() { self.emit_statement(stmt); }
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
            Expr::Template(val, _) => {
                let idx = self.intern_string(val) as i64;
                self.emit(Opcode::LoadStringConst, vec![idx]);
            }
            Expr::Identifier(name, _) => {
                if let Some(idx) = self.local_index(name) {
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
                self.emit_expression(arg);
                match op {
                    UnaryOp::Minus => self.emit(Opcode::Neg, vec![]),
                    UnaryOp::Plus => {},
                    UnaryOp::Not => self.emit(Opcode::Not, vec![]),
                    UnaryOp::BitNot => self.emit(Opcode::BitNot, vec![]),
                    UnaryOp::Typeof => self.emit(Opcode::TypeOf, vec![]),
                    UnaryOp::Void => { self.emit(Opcode::Pop, vec![]); self.emit(Opcode::LoadUndefined, vec![]); },
                    UnaryOp::Delete => self.emit(Opcode::LoadBoolean, vec![1]),
                }
            }
            Expr::Update(op, arg, prefix, _) => {
                match arg.as_ref() {
                    Expr::Identifier(name, _) => {
                        let opcode = match op {
                            UpdateOp::PlusPlus => {
                                if self.local_index(name).is_some() {
                                    Opcode::IncLocal
                                } else {
                                    Opcode::IncGlobal
                                }
                            }
                            UpdateOp::MinusMinus => {
                                if self.local_index(name).is_some() {
                                    Opcode::DecLocal
                                } else {
                                    Opcode::DecGlobal
                                }
                            }
                        };
                        let is_prefix = if *prefix { 1 } else { 0 };
                        if let Some(idx) = self.local_index(name) {
                            self.emit(opcode, vec![idx as i64, is_prefix]);
                        } else {
                            let name_idx = self.intern_string(name) as i64;
                            self.emit(opcode, vec![name_idx, is_prefix]);
                        }
                    }
                    _ => {
                        self.emit_expression(arg);
                        self.emit(Opcode::Pop, vec![]);
                        self.emit(Opcode::LoadUndefined, vec![]);
                    }
                }
            }
            Expr::Binary(op, lhs, rhs, _) => {
                if *op == BinaryOp::Assign {
                    match lhs.as_ref() {
                        Expr::Identifier(name, _) => {
                            self.emit_expression(rhs);
                            if let Some(idx) = self.local_index(name) {
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
                    BinaryOp::LogicalAnd => Opcode::LogicalAnd,
                    BinaryOp::LogicalOr => Opcode::LogicalOr,
                    BinaryOp::In => Opcode::In,
                    BinaryOp::Instanceof => Opcode::Eq,
                    BinaryOp::Assign => unreachable!(),
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
                    self.emit_expression(arg);
                }
                // stack: [this, callee, arg0, ..., argN-1]
                self.emit(Opcode::Call, vec![args.len() as i64]);
            }
            Expr::New(callee, args, _) => {
                self.emit_expression(callee);
                for arg in args {
                    self.emit_expression(arg);
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
            Expr::Assign(target, value, _) => {
                match target.as_ref() {
                    Expr::Identifier(name, _) => {
                        self.emit_expression(value);
                        if let Some(idx) = self.local_index(name) {
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
                }
            }
            Expr::Array(elems, _) => {
                for elem in elems {
                    self.emit_expression(elem);
                }
                self.emit(Opcode::NewArray, vec![elems.len() as i64]);
            }
            Expr::Object(props, _) => {
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
                    };
                    let idx = self.intern_string(&key_str) as i64;
                    operands.push(idx);
                }
                self.emit(Opcode::NewObject, operands);
            }
            Expr::Function(func, _) => {
                let func_idx = self.compile_function(func) as i64;
                self.emit(Opcode::MakeFunction, vec![func_idx]);
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

    fn local_index(&self, name: &str) -> Option<usize> {
        self.locals.iter().position(|l| l == name)
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
        program.float_pool = self.float_pool;
        program.assign_ic_indices();
        program
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
