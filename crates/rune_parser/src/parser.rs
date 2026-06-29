use crate::ast::*;
use crate::lexer::{Lexer, Token, TokenKind};

pub struct Parser {
    lexer: Lexer,
    tok: Token,
    pub errors: Vec<String>,
}

impl Parser {
    pub fn new(source: &str) -> Self {
        let mut lexer = Lexer::new(source);
        let tok = lexer.next_token();
        Parser {
            lexer,
            tok,
            errors: Vec::new(),
        }
    }

    fn advance(&mut self) {
        // Update regex_allowed based on current token kind
        // After value-producing tokens, / is division; after operators/keywords, / is regex.
        self.lexer.regex_allowed = !matches!(self.tok.kind,
            TokenKind::Number | TokenKind::String | TokenKind::RegExp
            | TokenKind::Identifier
            | TokenKind::True | TokenKind::False | TokenKind::Null | TokenKind::This
            | TokenKind::RParen | TokenKind::RBracket
            | TokenKind::PlusPlus | TokenKind::MinusMinus
            | TokenKind::Template | TokenKind::TemplateTail
        );
        self.tok = self.lexer.next_token();
    }

    fn expect(&mut self, kind: TokenKind) -> Token {
        if self.tok.kind == kind {
            let t = self.tok.clone();
            self.advance();
            t
        } else {
            self.error(format!("Expected {kind:?}, got {:?}", self.tok.kind));
            self.tok.clone()
        }
    }

    fn error(&mut self, msg: String) {
        self.errors
            .push(format!("{} at {}", msg, self.tok.span.start));
    }

    fn span(&self) -> Span {
        self.tok.span
    }

    // ---- Program ----

    pub fn parse(&mut self) -> Program {
        let start = self.tok.span.start;
        let mut body = Vec::new();
        while self.tok.kind != TokenKind::Eof {
            let stmt = self.parse_statement();
            body.push(stmt);
        }
        let end = self.tok.span.end;
        Program {
            body,
            span: Span { start, end },
        }
    }

    // ---- Statements ----

    fn parse_statement(&mut self) -> Stmt {
        match self.tok.kind {
            TokenKind::Function => self.parse_function_decl(),
            TokenKind::Async if self.lexer.peek_token().kind == TokenKind::Function => self.parse_async_function_decl(),
            TokenKind::Class => self.parse_class_decl(),
            TokenKind::Var | TokenKind::Let | TokenKind::Const => self.parse_var_decl(),
            TokenKind::Return => self.parse_return(),
            TokenKind::Throw => self.parse_throw(),
            TokenKind::If => self.parse_if(),
            TokenKind::While => self.parse_while(),
            TokenKind::Do => self.parse_do_while(),
            TokenKind::For => self.parse_for(),
            TokenKind::Break => self.parse_break_continue(true),
            TokenKind::Continue => self.parse_break_continue(false),
            TokenKind::Try => self.parse_try(),
            TokenKind::Switch => self.parse_switch(),
            TokenKind::LBrace => self.parse_block(),
            TokenKind::Semicolon => {
                let s = self.span();
                self.advance();
                Stmt::Empty(s)
            }
            _ => {
                let expr = self.parse_expr_comma();
                self.consume_semicolon();
                Stmt::Expr(expr, self.span())
            }
        }
    }

    fn parse_block(&mut self) -> Stmt {
        let start = self.span();
        self.expect(TokenKind::LBrace);
        let mut stmts = Vec::new();
        while self.tok.kind != TokenKind::RBrace && self.tok.kind != TokenKind::Eof {
            stmts.push(self.parse_statement());
        }
        let end = self.span();
        self.expect(TokenKind::RBrace);
        Stmt::Block(
            stmts,
            Span {
                start: start.start,
                end: end.end,
            },
        )
    }

    fn parse_function_decl(&mut self) -> Stmt {
        let start = self.span();
        self.expect(TokenKind::Function);
        let is_generator = if self.tok.kind == TokenKind::Star {
            self.advance();
            true
        } else {
            false
        };
        let name = if self.tok.kind == TokenKind::Identifier {
            let t = self.tok.clone();
            self.advance();
            Some(t.value.into_boxed_str())
        } else {
            None
        };
        let body = self.parse_function_body(name, is_generator, false, start);
        Stmt::Function(
            Box::new(body),
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_async_function_decl(&mut self) -> Stmt {
        let start = self.span();
        self.expect(TokenKind::Async);
        self.expect(TokenKind::Function);
        let is_generator = if self.tok.kind == TokenKind::Star {
            self.advance();
            true
        } else {
            false
        };
        let name = if self.tok.kind == TokenKind::Identifier {
            let t = self.tok.clone();
            self.advance();
            Some(t.value.into_boxed_str())
        } else {
            None
        };
        let body = self.parse_function_body(name, is_generator, true, start);
        Stmt::Function(
            Box::new(body),
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_class_decl(&mut self) -> Stmt {
        let start = self.span();
        self.expect(TokenKind::Class);
        let name = if self.tok.kind == TokenKind::Identifier {
            let t = self.tok.clone();
            self.advance();
            Some(t.value.into_boxed_str())
        } else {
            None
        };
        let heritage = if self.tok.kind == TokenKind::Extends {
            self.advance();
            Some(Box::new(self.parse_expr(0)))
        } else {
            None
        };
        let methods = self.parse_class_body();
        Stmt::Class(
            Box::new(ClassNode {
                name: name.clone(),
                heritage,
                methods,
                span: Span {
                    start: start.start,
                    end: self.span().end,
                },
            }),
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_class_expr(&mut self, name_hint: Option<Box<str>>) -> Expr {
        let start = self.span();
        self.expect(TokenKind::Class);
        let name = if self.tok.kind == TokenKind::Identifier {
            let t = self.tok.clone();
            self.advance();
            Some(t.value.into_boxed_str())
        } else {
            name_hint
        };
        let heritage = if self.tok.kind == TokenKind::Extends {
            self.advance();
            Some(Box::new(self.parse_expr(0)))
        } else {
            None
        };
        let methods = self.parse_class_body();
        Expr::Class(
            Box::new(ClassNode {
                name,
                heritage,
                methods,
                span: Span {
                    start: start.start,
                    end: self.span().end,
                },
            }),
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_class_body(&mut self) -> Vec<ClassMethod> {
        self.expect(TokenKind::LBrace);
        let mut methods = Vec::new();
        while self.tok.kind != TokenKind::RBrace && self.tok.kind != TokenKind::Eof {
            let mstart = self.span();
            let is_static = if self.tok.kind == TokenKind::Identifier
                && self.tok.value == "static"
                && self.lexer.peek_token().kind != TokenKind::LParen
            {
                self.advance();
                true
            } else {
                false
            };
            // Detect getter/setter: `get foo() {}` or `set foo(v) {}`
            let (is_getter, is_setter) = if self.tok.kind == TokenKind::Identifier
                && self.tok.value == "get"
                && self.lexer.peek_token().kind != TokenKind::LParen
            {
                self.advance();
                (true, false)
            } else if self.tok.kind == TokenKind::Identifier
                && self.tok.value == "set"
                && self.lexer.peek_token().kind != TokenKind::LParen
            {
                self.advance();
                (false, true)
            } else {
                (false, false)
            };
            let key = if self.tok.kind == TokenKind::LBracket {
                // Computed property key: [expr]() {}
                self.advance();
                let key_expr = self.parse_expr(0);
                self.expect(TokenKind::RBracket);
                PropKey::Computed(Box::new(key_expr))
            } else {
                self.parse_prop_key()
            };
            // Method definition: key(params) { body }
            if self.tok.kind == TokenKind::LParen {
                let name = match &key {
                    PropKey::Identifier(n) => Some(n.clone()),
                    PropKey::String(n) => Some(n.clone()),
                    PropKey::Number(n) => Some(Box::from(n.to_string())),
                    _ => None,
                };
                let func = self.parse_function_body(name, false, false, mstart);
                // Validate getter/setter param counts
                if is_getter && func.params.len() != 0 {
                    self.error("Getter must have 0 parameters".to_string());
                }
                if is_setter && func.params.len() != 1 {
                    self.error("Setter must have exactly 1 parameter".to_string());
                }
                methods.push(ClassMethod {
                    key,
                    func,
                    is_static,
                    is_getter,
                    is_setter,
                    span: Span {
                        start: mstart.start,
                        end: self.span().end,
                    },
                });
            } else {
                self.error("Expected method definition".to_string());
                self.advance();
            }
            // Semicolons are optional between class members
            if self.tok.kind == TokenKind::Semicolon {
                self.advance();
            }
        }
        self.expect(TokenKind::RBrace);
        methods
    }

    fn parse_function_body(
        &mut self,
        name: Option<Box<str>>,
        is_generator: bool,
        is_async: bool,
        start: Span,
    ) -> FnNode {
        self.expect(TokenKind::LParen);
        let mut params = Vec::new();
        let mut rest_param = None;
        while self.tok.kind != TokenKind::RParen && self.tok.kind != TokenKind::Eof {
            if self.tok.kind == TokenKind::Ellipsis {
                self.advance();
                if self.tok.kind == TokenKind::Identifier {
                    let t = self.tok.clone();
                    self.advance();
                    rest_param = Some(t.value.into_boxed_str());
                    break;
                }
                self.error("Expected parameter name after ...".into());
                break;
            }
            if self.tok.kind == TokenKind::Identifier {
                let t = self.tok.clone();
                self.advance();
                let default = if self.tok.kind == TokenKind::EqAssign {
                    self.advance(); // skip =
                    Some(Box::new(self.parse_expr(0)))
                } else {
                    None
                };
                params.push(Pattern::Identifier(
                    t.value.into_boxed_str(),
                    Span {
                        start: t.span.start,
                        end: t.span.end,
                    },
                    default,
                ));
            } else if matches!(self.tok.kind, TokenKind::LBrace | TokenKind::LBracket) {
                let pat = self.parse_binding_pattern();
                let default = if self.tok.kind == TokenKind::EqAssign {
                    self.advance();
                    Some(Box::new(self.parse_expr(0)))
                } else {
                    None
                };
                if let Some(expr) = default {
                    params.push(Pattern::Default(Box::new(pat), expr));
                } else {
                    params.push(pat);
                }
            }
            if self.tok.kind == TokenKind::Comma {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(TokenKind::RParen);
        let body = if self.tok.kind == TokenKind::LBrace {
            self.parse_block()
        } else {
            Stmt::Empty(self.span())
        };
        FnNode {
            name,
            params,
            rest_param,
            body,
            is_generator,
            is_async,
            is_arrow: false,
            span: Span {
                start: start.start,
                end: self.span().end,
            },
        }
    }

    fn parse_var_decl(&mut self) -> Stmt {
        let start = self.span();
        let kind = match self.tok.kind {
            TokenKind::Var => {
                self.advance();
                VarKind::Var
            }
            TokenKind::Let => {
                self.advance();
                VarKind::Let
            }
            TokenKind::Const => {
                self.advance();
                VarKind::Const
            }
            _ => unreachable!(),
        };
        let mut decls = Vec::new();
        loop {
            let dstart = self.span();
            let (name, pattern) =
                if matches!(self.tok.kind, TokenKind::LBrace | TokenKind::LBracket) {
                    let pat = self.parse_binding_pattern();
                    (Box::from("_destructure"), Some(pat))
                } else if self.tok.kind == TokenKind::Identifier {
                    let t = self.tok.clone();
                    self.advance();
                    (t.value.into_boxed_str(), None)
                } else {
                    self.error("Expected identifier".into());
                    (Box::from("_error"), None)
                };
            let init = if self.tok.kind == TokenKind::EqAssign {
                self.advance();
                Some(Box::new(self.parse_expr(0)))
            } else {
                None
            };
            if kind == VarKind::Const && init.is_none() {
                self.error("const declaration must be initialized".to_string());
            }
            decls.push(Decl {
                name,
                pattern,
                init,
                span: Span {
                    start: dstart.start,
                    end: self.span().end,
                },
            });
            if self.tok.kind == TokenKind::Comma {
                self.advance();
            } else {
                break;
            }
        }
        self.consume_semicolon();
        Stmt::Var(
            kind,
            decls,
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_return(&mut self) -> Stmt {
        let start = self.span();
        self.advance();
        let value = if self.has_semicolon_or_asi() {
            None
        } else {
            Some(Box::new(self.parse_expr_comma()))
        };
        self.consume_semicolon();
        Stmt::Return(
            value,
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_throw(&mut self) -> Stmt {
        let start = self.span();
        self.advance();
        if self.lexer.had_newline {
            self.error("Illegal newline after throw".to_string());
        }
        let value = self.parse_expr(0);
        self.consume_semicolon();
        Stmt::Throw(
            Box::new(value),
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_try(&mut self) -> Stmt {
        let start = self.span();
        self.expect(TokenKind::Try);
        let body = match self.parse_block() {
            Stmt::Block(stmts, _) => stmts.into_boxed_slice(),
            _ => vec![].into_boxed_slice(),
        };
        let catch = if self.tok.kind == TokenKind::Catch {
            self.advance();
            self.expect(TokenKind::LParen);
            let param = if self.tok.kind == TokenKind::Identifier {
                let t = self.tok.clone();
                self.advance();
                t.value.to_string()
            } else {
                String::new()
            };
            self.expect(TokenKind::RParen);
            let body = match self.parse_block() {
                Stmt::Block(stmts, _) => stmts.into_boxed_slice(),
                _ => vec![].into_boxed_slice(),
            };
            Some(CatchClause {
                param: param.into_boxed_str(),
                body,
                span: self.span(),
            })
        } else {
            None
        };
        let finalizer = if self.tok.kind == TokenKind::Finally {
            self.advance();
            match self.parse_block() {
                Stmt::Block(stmts, _) => Some(stmts.into_boxed_slice()),
                _ => None,
            }
        } else {
            None
        };
        Stmt::Try(
            body,
            catch,
            finalizer,
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_switch(&mut self) -> Stmt {
        let start = self.span();
        self.expect(TokenKind::Switch);
        self.expect(TokenKind::LParen);
        let discriminant = Box::new(self.parse_expr(0));
        self.expect(TokenKind::RParen);
        self.expect(TokenKind::LBrace);
        let mut cases: Vec<SwitchCase> = Vec::new();
        let mut default_body: Option<Box<[Stmt]>> = None;
        loop {
            match self.tok.kind {
                TokenKind::Case => {
                    let cs = self.span();
                    self.advance();
                    let test = self.parse_expr(0);
                    self.expect(TokenKind::Colon);
                    let mut body = Vec::new();
                    while self.tok.kind != TokenKind::Case
                        && self.tok.kind != TokenKind::Default
                        && self.tok.kind != TokenKind::RBrace
                        && self.tok.kind != TokenKind::Eof
                    {
                        body.push(self.parse_statement());
                    }
                    cases.push(SwitchCase {
                        test,
                        body,
                        span: cs,
                    });
                }
                TokenKind::Default => {
                    self.advance();
                    self.expect(TokenKind::Colon);
                    let mut body = Vec::new();
                    while self.tok.kind != TokenKind::Case
                        && self.tok.kind != TokenKind::Default
                        && self.tok.kind != TokenKind::RBrace
                        && self.tok.kind != TokenKind::Eof
                    {
                        body.push(self.parse_statement());
                    }
                    default_body = Some(body.into_boxed_slice());
                }
                TokenKind::RBrace => {
                    self.advance();
                    break;
                }
                _ => {
                    self.advance();
                }
            }
        }
        Stmt::Switch(
            discriminant,
            cases,
            default_body,
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_if(&mut self) -> Stmt {
        let start = self.span();
        self.expect(TokenKind::If);
        self.expect(TokenKind::LParen);
        let cond = self.parse_expr(0);
        self.expect(TokenKind::RParen);
        let then = Box::new(self.parse_statement());
        let else_branch = if self.tok.kind == TokenKind::Else {
            self.advance();
            Some(Box::new(self.parse_statement()))
        } else {
            None
        };
        Stmt::If(
            Box::new(cond),
            then,
            else_branch,
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_while(&mut self) -> Stmt {
        let start = self.span();
        self.expect(TokenKind::While);
        self.expect(TokenKind::LParen);
        let cond = self.parse_expr(0);
        self.expect(TokenKind::RParen);
        let body = Box::new(self.parse_statement());
        Stmt::While(
            Box::new(cond),
            body,
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_do_while(&mut self) -> Stmt {
        let start = self.span();
        self.expect(TokenKind::Do);
        let body = Box::new(self.parse_statement());
        self.expect(TokenKind::While);
        self.expect(TokenKind::LParen);
        let cond = self.parse_expr(0);
        self.expect(TokenKind::RParen);
        self.consume_semicolon();
        Stmt::DoWhile(
            Box::new(cond),
            body,
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_for(&mut self) -> Stmt {
        let start = self.span();
        self.expect(TokenKind::For);
        self.expect(TokenKind::LParen);
        // Check for no-initializer C-style: `for (; cond; update)`
        if self.tok.kind == TokenKind::Semicolon {
            self.advance();
            return self.parse_for_c_style(None, start);
        }
        // Check for `for (var x in obj)` — peek ahead for `in` after the first var decl
        if matches!(
            self.tok.kind,
            TokenKind::Var | TokenKind::Let | TokenKind::Const
        ) {
            let var_stmt = self.parse_var_decl();
            if self.tok.kind == TokenKind::In {
                // for (var x in obj)
                self.advance();
                let obj = self.parse_expr(0);
                self.expect(TokenKind::RParen);
                let body = Box::new(self.parse_statement());
                // Extract the variable name from the var declaration
                let name = match &var_stmt {
                    Stmt::Var(_, decls, _) if decls.len() == 1 => decls[0].name.clone(),
                    _ => panic!("for-in must have exactly one loop variable"),
                };
                let lhs = Box::new(Expr::Identifier(name, Span::default()));
                return Stmt::ForIn(
                    lhs,
                    Box::new(obj),
                    body,
                    Span {
                        start: start.start,
                        end: self.span().end,
                    },
                );
            }
            // C-style for with var: cond and update follow
            return self.parse_for_c_style(Some(Box::new(var_stmt)), start);
        }
        // Try `for (lhs in obj)` — parse expression and check for `in`
        let lhs = self.parse_expr_comma();
        if self.tok.kind == TokenKind::In {
            self.advance();
            let obj = self.parse_expr(0);
            self.expect(TokenKind::RParen);
            let body = Box::new(self.parse_statement());
            return Stmt::ForIn(
                Box::new(lhs),
                Box::new(obj),
                body,
                Span {
                    start: start.start,
                    end: self.span().end,
                },
            );
        }
        // C-style for: `for (init; cond; update)`
        self.consume_semicolon();
        self.parse_for_c_style(Some(Box::new(Stmt::Expr(lhs, Span::default()))), start)
    }

    /// Parse the remaining parts of a C-style for loop (after init).
    fn parse_for_c_style(&mut self, init: Option<Box<Stmt>>, start: Span) -> Stmt {
        let cond = if self.tok.kind != TokenKind::Semicolon {
            let e = self.parse_expr(0);
            self.consume_semicolon();
            Some(Box::new(e))
        } else {
            self.advance();
            None
        };
        let update = if self.tok.kind != TokenKind::RParen {
            let e = self.parse_expr_comma();
            Some(Box::new(e))
        } else {
            None
        };
        self.expect(TokenKind::RParen);
        let body = Box::new(self.parse_statement());
        Stmt::For(
            init,
            cond,
            update,
            body,
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_break_continue(&mut self, is_break: bool) -> Stmt {
        let start = self.span();
        self.advance();
        let label = if self.tok.kind == TokenKind::Identifier {
            let t = self.tok.clone();
            self.advance();
            Some(t.value.into_boxed_str())
        } else {
            None
        };
        self.consume_semicolon();
        if is_break {
            Stmt::Break(
                label,
                Span {
                    start: start.start,
                    end: self.span().end,
                },
            )
        } else {
            Stmt::Continue(
                label,
                Span {
                    start: start.start,
                    end: self.span().end,
                },
            )
        }
    }

    // ---- Expressions ----

    fn parse_expr(&mut self, min_prec: u32) -> Expr {
        let mut lhs = self.parse_unary();
        lhs = self.parse_postfix(lhs);

        loop {
            let prec = self.binary_precedence();
            if prec < min_prec {
                break;
            }
            let op = match self.tok.kind {
                TokenKind::LogicalOr => {
                    self.advance();
                    BinaryOp::LogicalOr
                }
                TokenKind::LogicalAnd => {
                    self.advance();
                    BinaryOp::LogicalAnd
                }
                TokenKind::BitOr => {
                    self.advance();
                    BinaryOp::BitOr
                }
                TokenKind::BitXor => {
                    self.advance();
                    BinaryOp::BitXor
                }
                TokenKind::BitAnd => {
                    self.advance();
                    BinaryOp::BitAnd
                }
                TokenKind::Eq => {
                    self.advance();
                    BinaryOp::Eq
                }
                TokenKind::Ne => {
                    self.advance();
                    BinaryOp::Ne
                }
                TokenKind::StrictEq => {
                    self.advance();
                    BinaryOp::StrictEq
                }
                TokenKind::StrictNe => {
                    self.advance();
                    BinaryOp::StrictNe
                }
                TokenKind::Lt => {
                    self.advance();
                    BinaryOp::Lt
                }
                TokenKind::Gt => {
                    self.advance();
                    BinaryOp::Gt
                }
                TokenKind::Le => {
                    self.advance();
                    BinaryOp::Le
                }
                TokenKind::Ge => {
                    self.advance();
                    BinaryOp::Ge
                }
                TokenKind::Instanceof => {
                    self.advance();
                    BinaryOp::Instanceof
                }
                TokenKind::In => {
                    self.advance();
                    BinaryOp::In
                }
                TokenKind::Shl => {
                    self.advance();
                    BinaryOp::Shl
                }
                TokenKind::Shr => {
                    self.advance();
                    BinaryOp::Shr
                }
                TokenKind::ShrU => {
                    self.advance();
                    BinaryOp::ShrU
                }
                TokenKind::Plus => {
                    self.advance();
                    BinaryOp::Add
                }
                TokenKind::Minus => {
                    self.advance();
                    BinaryOp::Sub
                }
                TokenKind::Star => {
                    self.advance();
                    BinaryOp::Mul
                }
                TokenKind::Slash => {
                    self.advance();
                    BinaryOp::Div
                }
                TokenKind::Percent => {
                    self.advance();
                    BinaryOp::Mod
                }
                TokenKind::StarStar => {
                    self.advance();
                    BinaryOp::Exp
                }
                TokenKind::EqAssign
                | TokenKind::PlusAssign
                | TokenKind::MinusAssign
                | TokenKind::StarAssign
                | TokenKind::SlashAssign
                | TokenKind::PercentAssign
                | TokenKind::StarStarAssign
                | TokenKind::ShlAssign
                | TokenKind::ShrAssign
                | TokenKind::ShrUAssign
                | TokenKind::BitAndAssign
                | TokenKind::BitOrAssign
                | TokenKind::BitXorAssign
                | TokenKind::AndAssign
                | TokenKind::OrAssign
                | TokenKind::NullishAssign => {
                    // Assignment is right-associative
                    let op = self.parse_assign_op();
                    let rhs = self.parse_expr(prec); // right-assoc: same precedence
                    let span = Span {
                        start: self.span().start,
                        end: self.span().end,
                    };
                    if op == BinaryOp::Assign {
                        lhs = Expr::Assign(Box::new(lhs), Box::new(rhs), span);
                    } else {
                        lhs = Expr::CompoundAssign(op, Box::new(lhs), Box::new(rhs), span);
                    }
                    break; // assignments consume the rest
                }
                _ => break,
            };
            let rhs = self.parse_expr(prec + 1);
            let span = Span {
                start: self.span().start,
                end: self.span().end,
            };
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs), span);
        }

        // Ternary
        if self.tok.kind == TokenKind::Question {
            self.advance();
            let then = self.parse_expr(0);
            self.expect(TokenKind::Colon);
            let else_ = self.parse_expr(0);
            let span = self.span();
            lhs = Expr::Conditional(Box::new(lhs), Box::new(then), Box::new(else_), span);
        }

        lhs
    }

    /// Parse an expression allowing the comma operator at the top level.
    /// Only called from expression-statement and parenthesized-expression contexts.
    fn parse_expr_comma(&mut self) -> Expr {
        let mut lhs = self.parse_expr(0);
        while self.tok.kind == TokenKind::Comma {
            self.advance();
            let rhs = self.parse_expr(0);
            lhs = Expr::Binary(
                BinaryOp::Comma,
                Box::new(lhs),
                Box::new(rhs),
                Span {
                    start: self.span().start,
                    end: self.span().end,
                },
            );
        }
        lhs
    }

    fn parse_unary(&mut self) -> Expr {
        let start = self.span();
        match self.tok.kind {
            TokenKind::Plus => {
                self.advance();
                self.make_unary(UnaryOp::Plus, start)
            }
            TokenKind::Minus => {
                self.advance();
                self.make_unary(UnaryOp::Minus, start)
            }
            TokenKind::Not => {
                self.advance();
                self.make_unary(UnaryOp::Not, start)
            }
            TokenKind::BitNot => {
                self.advance();
                self.make_unary(UnaryOp::BitNot, start)
            }
            TokenKind::Typeof => {
                self.advance();
                self.make_unary(UnaryOp::Typeof, start)
            }
            TokenKind::Void => {
                self.advance();
                self.make_unary(UnaryOp::Void, start)
            }
            TokenKind::Delete => {
                self.advance();
                self.make_unary(UnaryOp::Delete, start)
            }
            TokenKind::PlusPlus => {
                self.advance();
                let arg = self.parse_unary();
                Expr::Update(
                    UpdateOp::PlusPlus,
                    Box::new(arg),
                    true,
                    Span {
                        start: start.start,
                        end: self.span().end,
                    },
                )
            }
            TokenKind::MinusMinus => {
                self.advance();
                let arg = self.parse_unary();
                Expr::Update(
                    UpdateOp::MinusMinus,
                    Box::new(arg),
                    true,
                    Span {
                        start: start.start,
                        end: self.span().end,
                    },
                )
            }
            TokenKind::New => {
                self.advance();
                // Parse callee as a member expression (dot/bracket postfix only, NO calls)
                let callee = Box::new(self.parse_member_expr());
                let args = if self.tok.kind == TokenKind::LParen {
                    self.advance();
                    let mut a = Vec::new();
                    while self.tok.kind != TokenKind::RParen && self.tok.kind != TokenKind::Eof {
                        let arg_start = self.span();
                        let is_spread = self.tok.kind == TokenKind::Ellipsis;
                        if is_spread {
                            self.advance();
                        }
                        let expr = self.parse_expr(0);
                        a.push(ArrayElement {
                            expr,
                            is_spread,
                            span: Span {
                                start: arg_start.start,
                                end: self.span().end,
                            },
                        });
                        if self.tok.kind == TokenKind::Comma {
                            self.advance();
                        }
                    }
                    self.expect(TokenKind::RParen);
                    a
                } else {
                    Vec::new()
                };
                Expr::New(
                    callee,
                    args,
                    Span {
                        start: start.start,
                        end: self.span().end,
                    },
                )
            }
            TokenKind::Yield => {
                self.advance();
                let has_arg = !self.lexer.had_newline && self.tok_can_start_expr();
                let arg = if has_arg {
                    Some(Box::new(self.parse_expr(0)))
                } else {
                    None
                };
                Expr::Yield(
                    arg,
                    Span {
                        start: start.start,
                        end: self.span().end,
                    },
                )
            }
            TokenKind::Await => {
                self.advance();
                let arg = self.parse_unary();
                Expr::Await(
                    Box::new(arg),
                    Span {
                        start: start.start,
                        end: self.span().end,
                    },
                )
            }
            _ => self.parse_primary(),
        }
    }

    fn make_unary(&mut self, op: UnaryOp, start: Span) -> Expr {
        let arg = self.parse_unary();
        Expr::Unary(
            op,
            Box::new(arg),
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_primary(&mut self) -> Expr {
        let mut expr = self.parse_primary_inner();
        expr = self.parse_postfix(expr);
        expr
    }

    /// Like parse_primary but only applies member-access postfix (dot/bracket), no calls.
    /// Used by `new` so that `(` is not consumed as a function call.
    fn parse_member_expr(&mut self) -> Expr {
        let mut expr = self.parse_primary_inner();
        expr = self.parse_member_tail(expr);
        expr
    }

    /// Parse a primary expression WITHOUT postfix operations.
    fn parse_primary_inner(&mut self) -> Expr {
        let start = self.span();
        match self.tok.kind {
            TokenKind::Number => {
                let t = self.tok.clone();
                self.advance();
                let cleaned = t.value.replace('_', "");
                let val = if cleaned.starts_with("0x") || cleaned.starts_with("0X") {
                    u64::from_str_radix(&cleaned[2..], 16).unwrap_or(0) as f64
                } else if cleaned.starts_with("0o") || cleaned.starts_with("0O") {
                    u64::from_str_radix(&cleaned[2..], 8).unwrap_or(0) as f64
                } else if cleaned.starts_with("0b") || cleaned.starts_with("0B") {
                    u64::from_str_radix(&cleaned[2..], 2).unwrap_or(0) as f64
                } else {
                    cleaned.parse::<f64>().unwrap_or(0.0)
                };
                Expr::Number(
                    val,
                    Span {
                        start: start.start,
                        end: t.span.end,
                    },
                )
            }
            TokenKind::String => {
                let t = self.tok.clone();
                self.advance();
                Expr::String(
                    t.value.into_boxed_str(),
                    Span {
                        start: start.start,
                        end: t.span.end,
                    },
                )
            }
            TokenKind::True => {
                self.advance();
                Expr::Boolean(
                    true,
                    Span {
                        start: start.start,
                        end: self.span().end,
                    },
                )
            }
            TokenKind::False => {
                self.advance();
                Expr::Boolean(
                    false,
                    Span {
                        start: start.start,
                        end: self.span().end,
                    },
                )
            }
            TokenKind::Null => {
                self.advance();
                Expr::Null(Span {
                    start: start.start,
                    end: self.span().end,
                })
            }
            TokenKind::This => {
                self.advance();
                Expr::This(Span {
                    start: start.start,
                    end: self.span().end,
                })
            }
            TokenKind::Super => {
                self.advance();
                Expr::Super(Span {
                    start: start.start,
                    end: self.span().end,
                })
            }
            TokenKind::Identifier => {
                let t = self.tok.clone();
                self.advance();
                // Single-param arrow: ident => body
                if self.tok.kind == TokenKind::Arrow {
                    let span = Span {
                        start: t.span.start,
                        end: t.span.end,
                    };
                    return self.parse_arrow_body(
                        vec![Pattern::Identifier(
                            t.value.clone().into_boxed_str(),
                            span,
                            None,
                        )],
                        None,
                        start,
                    );
                }
                Expr::Identifier(
                    t.value.clone().into_boxed_str(),
                    Span {
                        start: start.start,
                        end: t.span.end,
                    },
                )
            }
            TokenKind::LParen => {
                self.advance();
                // Check for () => (zero-param arrow)
                if self.tok.kind == TokenKind::RParen {
                    self.advance();
                    if self.tok.kind == TokenKind::Arrow {
                        return self.parse_arrow_body(Vec::new(), None, start);
                    }
                    return Expr::Undefined(Span {
                        start: start.start,
                        end: self.span().end,
                    });
                }
                // Handle (...rest) => arrow with rest param only
                if self.tok.kind == TokenKind::Ellipsis {
                    self.advance();
                    if self.tok.kind == TokenKind::Identifier {
                        let t = self.tok.clone();
                        self.advance();
                        self.expect(TokenKind::RParen);
                        if self.tok.kind == TokenKind::Arrow {
                            return self.parse_arrow_body(
                                Vec::new(),
                                Some(t.value.into_boxed_str()),
                                start,
                            );
                        }
                        return Expr::Identifier(
                            t.value.into_boxed_str(),
                            Span {
                                start: start.start,
                                end: self.span().end,
                            },
                        );
                    }
                    self.error("Expected parameter name after ...".into());
                    return Expr::Undefined(Span {
                        start: start.start,
                        end: self.span().end,
                    });
                }
                // Peek-ahead arrow detection: only consume the identifier
                // if what follows is `,` or `)` (arrow param candidates).
                // Otherwise fall through to parse_expr (regular paren expr).
                if self.tok.kind == TokenKind::Identifier {
                    let next = self.lexer.peek_token().kind;
                    if matches!(next, TokenKind::Comma | TokenKind::RParen) {
                        let name = self.tok.value.clone().into_boxed_str();
                        let name_span = self.span();
                        self.advance();
                        if next == TokenKind::Comma {
                            // Multi-param: (a, b, ...) => body or comma expr
                            let mut params =
                                vec![Pattern::Identifier(name.clone(), name_span, None)];
                            while self.tok.kind == TokenKind::Comma {
                                self.advance();
                                let mut rest_name = None;
                                if self.tok.kind == TokenKind::Ellipsis {
                                    self.advance();
                                    if self.tok.kind == TokenKind::Identifier {
                                        rest_name = Some(self.tok.value.clone().into_boxed_str());
                                        self.advance();
                                    } else {
                                        self.error("Expected parameter name after ...".into());
                                    }
                                }
                                if let Some(rn) = rest_name {
                                    self.expect(TokenKind::RParen);
                                    if self.tok.kind == TokenKind::Arrow {
                                        return self.parse_arrow_body(params, Some(rn), start);
                                    }
                                    return Expr::Identifier(
                                        name,
                                        Span {
                                            start: start.start,
                                            end: self.span().end,
                                        },
                                    );
                                }
                                if self.tok.kind == TokenKind::Identifier {
                                    let p_span = self.span();
                                    params.push(Pattern::Identifier(
                                        self.tok.value.clone().into_boxed_str(),
                                        p_span,
                                        None,
                                    ));
                                    self.advance();
                                }
                            }
                            self.expect(TokenKind::RParen);
                            if self.tok.kind == TokenKind::Arrow {
                                return self.parse_arrow_body(params, None, start);
                            }
                            // Not an arrow — reconstruct as comma expression
                            // For now, just return the first identifier
                            return Expr::Identifier(
                                name,
                                Span {
                                    start: start.start,
                                    end: self.span().end,
                                },
                            );
                        }
                        // next == RParen
                        self.expect(TokenKind::RParen);
                        if self.tok.kind == TokenKind::Arrow {
                            // (name) => body — single-param arrow
                            let p_span = Span {
                                start: start.start,
                                end: self.span().end,
                            };
                            let ident = Pattern::Identifier(name, p_span, None);
                            return self.parse_arrow_body(vec![ident], None, start);
                        }
                        // (name) — just a parenthesized identifier
                        return Expr::Identifier(
                            name,
                            Span {
                                start: start.start,
                                end: self.span().end,
                            },
                        );
                    }
                    // Next token is not `,` or `)` — not an arrow.
                    // DON'T consume the identifier; fall through to parse_expr.
                }
                let expr = self.parse_expr_comma();
                self.expect(TokenKind::RParen);
                // Single-param arrow: (expr) => body
                if self.tok.kind == TokenKind::Arrow
                    && let Expr::Identifier(name, id_span) = &expr
                {
                    return self.parse_arrow_body(
                        vec![Pattern::Identifier(name.clone(), *id_span, None)],
                        None,
                        start,
                    );
                }
                expr
            }
            TokenKind::LBracket => {
                self.advance();
                let mut elems = Vec::new();
                while self.tok.kind != TokenKind::RBracket && self.tok.kind != TokenKind::Eof {
                    let estart = self.span();
                    let is_spread = self.tok.kind == TokenKind::Ellipsis;
                    if is_spread {
                        self.advance();
                    }
                    let expr = self.parse_expr(0);
                    let eend = self.span();
                    elems.push(ArrayElement {
                        expr,
                        is_spread,
                        span: Span {
                            start: estart.start,
                            end: eend.end,
                        },
                    });
                    if self.tok.kind == TokenKind::Comma {
                        self.advance();
                    }
                }
                self.expect(TokenKind::RBracket);
                let span = Span {
                    start: start.start,
                    end: self.span().end,
                };
                Expr::Array(elems, span)
            }
            TokenKind::LBrace => {
                self.advance();
                let mut props = Vec::new();
                while self.tok.kind != TokenKind::RBrace && self.tok.kind != TokenKind::Eof {
                    let pstart = self.span();
                    if self.tok.kind == TokenKind::Ellipsis {
                        self.advance();
                        let value = self.parse_expr(0);
                        props.push(Property {
                            key: PropKey::String(Box::from("")),
                            value,
                            is_spread: true,
                            span: Span {
                                start: pstart.start,
                                end: self.span().end,
                            },
                        });
                    } else if self.tok.kind == TokenKind::LBracket {
                        // Computed property key: { [expr]: value }
                        self.advance();
                        let key_expr = self.parse_expr(0);
                        self.expect(TokenKind::RBracket);
                        if self.tok.kind == TokenKind::LParen {
                            // Computed method name: { [expr]() { body } }
                            let name = None;
                            let fn_body = self.parse_function_body(name, false, false, pstart);
                            props.push(Property {
                                key: PropKey::Computed(Box::new(key_expr)),
                                value: Expr::Function(
                                    Box::new(fn_body),
                                    Span {
                                        start: pstart.start,
                                        end: self.span().end,
                                    },
                                ),
                                is_spread: false,
                                span: Span {
                                    start: pstart.start,
                                    end: self.span().end,
                                },
                            });
                        } else {
                            self.expect(TokenKind::Colon);
                            let value = self.parse_expr(0);
                            props.push(Property {
                                key: PropKey::Computed(Box::new(key_expr)),
                                value,
                                is_spread: false,
                                span: Span {
                                    start: pstart.start,
                                    end: self.span().end,
                                },
                            });
                        }
                    } else {
                        let key = self.parse_prop_key();
                        if self.tok.kind == TokenKind::LParen {
                            // Method shorthand: { foo() { body } }
                            let name = match &key {
                                PropKey::Identifier(n) => Some(n.clone()),
                                PropKey::String(n) => Some(n.clone()),
                                PropKey::Number(n) => Some(Box::from(n.to_string())),
                                _ => None,
                            };
                            let fn_body = self.parse_function_body(name, false, false, pstart);
                            props.push(Property {
                                key,
                                value: Expr::Function(
                                    Box::new(fn_body),
                                    Span {
                                        start: pstart.start,
                                        end: self.span().end,
                                    },
                                ),
                                is_spread: false,
                                span: Span {
                                    start: pstart.start,
                                    end: self.span().end,
                                },
                            });
                        } else if self.tok.kind == TokenKind::Colon {
                            // Regular property: { key: value }
                            self.advance();
                            let value = self.parse_expr(0);
                            props.push(Property {
                                key,
                                value,
                                is_spread: false,
                                span: Span {
                                    start: pstart.start,
                                    end: self.span().end,
                                },
                            });
                        } else {
                            // Shorthand: { key } === { key: key }
                            let name = match &key {
                                PropKey::Identifier(n) => n.clone(),
                                _ => {
                                    self.error("Invalid shorthand property".into());
                                    Box::from("_error")
                                }
                            };
                            let value = Expr::Identifier(
                                name,
                                Span {
                                    start: pstart.start,
                                    end: self.span().end,
                                },
                            );
                            props.push(Property {
                                key,
                                value,
                                is_spread: false,
                                span: Span {
                                    start: pstart.start,
                                    end: self.span().end,
                                },
                            });
                        }
                    }
                    if self.tok.kind == TokenKind::Comma {
                        self.advance();
                    }
                }
                self.expect(TokenKind::RBrace);
                let span = Span {
                    start: start.start,
                    end: self.span().end,
                };
                Expr::Object(props, span)
            }
            TokenKind::TemplateNoSub => {
                let t = self.tok.clone();
                self.advance();
                Expr::Template {
                    parts: vec![t.value],
                    exprs: vec![],
                    span: Span {
                        start: start.start,
                        end: t.span.end,
                    },
                }
            }
            TokenKind::TemplateHead => {
                let t = self.tok.clone();
                self.advance();
                let mut parts = vec![t.value];
                let mut exprs = Vec::new();
                // Parse first expression
                let expr = self.parse_expr(0);
                exprs.push(expr);
                // After the expression, lexer produces TemplateMiddle or TemplateTail
                loop {
                    match &self.tok.kind {
                        TokenKind::TemplateMiddle => {
                            let mid = self.tok.clone();
                            self.advance();
                            parts.push(mid.value);
                            let e = self.parse_expr(0);
                            exprs.push(e);
                        }
                        TokenKind::TemplateTail => {
                            let tail = self.tok.clone();
                            self.advance();
                            parts.push(tail.value);
                            break;
                        }
                        _ => {
                            self.error("Expected template continuation after expression".into());
                            break;
                        }
                    }
                }
                Expr::Template {
                    parts,
                    exprs,
                    span: Span {
                        start: start.start,
                        end: self.span().end,
                    },
                }
            }
            TokenKind::Function => {
                self.advance();
                let is_generator = if self.tok.kind == TokenKind::Star {
                    self.advance();
                    true
                } else {
                    false
                };
                let name = if self.tok.kind == TokenKind::Identifier {
                    let t = self.tok.clone();
                    self.advance();
                    Some(t.value.into_boxed_str())
                } else {
                    None
                };
                let body = self.parse_function_body(name.clone(), is_generator, false, start);
                let span = Span {
                    start: start.start,
                    end: self.span().end,
                };
                Expr::Function(Box::new(body), span)
            }
            TokenKind::Class => self.parse_class_expr(None),
            TokenKind::Async if self.lexer.peek_token().kind == TokenKind::Function => {
                self.advance();
                self.expect(TokenKind::Function);
                let is_generator = if self.tok.kind == TokenKind::Star {
                    self.advance();
                    true
                } else {
                    false
                };
                let name = if self.tok.kind == TokenKind::Identifier {
                    let t = self.tok.clone();
                    self.advance();
                    Some(t.value.into_boxed_str())
                } else {
                    None
                };
                let body = self.parse_function_body(name.clone(), is_generator, true, start);
                let span = Span {
                    start: start.start,
                    end: self.span().end,
                };
                Expr::Function(Box::new(body), span)
            }
            TokenKind::RegExp => {
                let t = self.tok.clone();
                self.advance();
                Expr::RegExp(
                    t.value.into_boxed_str(),
                    t.flags.into_boxed_str(),
                    Span {
                        start: start.start,
                        end: self.span().end,
                    },
                )
            }
            _ => {
                self.error(format!("Unexpected token {:?}", self.tok.kind));
                self.advance();
                Expr::Undefined(self.span())
            }
        }
    }

    fn parse_prop_key(&mut self) -> PropKey {
        match self.tok.kind {
            TokenKind::String => {
                let t = self.tok.clone();
                self.advance();
                PropKey::String(t.value.into_boxed_str())
            }
            TokenKind::Number => {
                let t = self.tok.clone();
                self.advance();
                PropKey::Number(t.value.replace('_', "").parse::<f64>().unwrap_or(0.0))
            }
            _ => {
                let t = self.tok.clone();
                self.advance();
                PropKey::Identifier(t.value.into_boxed_str())
            }
        }
    }

    fn parse_binding_pattern(&mut self) -> Pattern {
        match self.tok.kind {
            TokenKind::LBrace => {
                let start = self.span();
                self.advance();
                let mut props = Vec::new();
                let mut rest = None;
                while self.tok.kind != TokenKind::RBrace && self.tok.kind != TokenKind::Eof {
                    let pstart = self.span();
                    if self.tok.kind == TokenKind::Ellipsis {
                        self.advance();
                        let inner = self.parse_binding_pattern();
                        rest = Some(Box::new(inner));
                        break;
                    }
                    let (key, pattern) = if self.tok.kind == TokenKind::LBracket {
                        // Computed property key in destructuring: { [expr]: pattern }
                        self.advance();
                        let key_expr = self.parse_expr(0);
                        self.expect(TokenKind::RBracket);
                        self.expect(TokenKind::Colon);
                        let mut pat = self.parse_binding_pattern();
                        if self.tok.kind == TokenKind::EqAssign {
                            self.advance();
                            let default = self.parse_expr(0);
                            pat = Pattern::Default(Box::new(pat), Box::new(default));
                        }
                        (PropKey::Computed(Box::new(key_expr)), pat)
                    } else {
                        let key = self.parse_prop_key();
                        let pattern = if self.tok.kind == TokenKind::Colon {
                            self.advance();
                            self.parse_binding_pattern()
                        } else if let PropKey::Identifier(id) = &key {
                            // Check for default: {a = expr}
                            let default = if self.tok.kind == TokenKind::EqAssign {
                                self.advance();
                                Some(Box::new(self.parse_expr(0)))
                            } else {
                                None
                            };
                            Pattern::Identifier(
                                id.clone(),
                                Span {
                                    start: pstart.start,
                                    end: self.span().end,
                                },
                                default,
                            )
                        } else {
                            self.error("Expected binding identifier after property name".into());
                            Pattern::Identifier(Box::from("_error"), self.span(), None)
                        };
                        (key, pattern)
                    };
                    props.push(ObjectPatternProp {
                        key,
                        pattern,
                        span: Span {
                            start: pstart.start,
                            end: self.span().end,
                        },
                    });
                    if self.tok.kind == TokenKind::Comma {
                        self.advance();
                    } else {
                        break;
                    }
                }
                self.expect(TokenKind::RBrace);
                Pattern::Object(
                    props,
                    rest,
                    Span {
                        start: start.start,
                        end: self.span().end,
                    },
                )
            }
            TokenKind::LBracket => {
                let start = self.span();
                self.advance();
                let mut items = Vec::new();
                while self.tok.kind != TokenKind::RBracket && self.tok.kind != TokenKind::Eof {
                    if self.tok.kind == TokenKind::Comma {
                        items.push(None);
                        self.advance();
                        continue;
                    }
                    // Rest pattern: ...rest
                    if self.tok.kind == TokenKind::Ellipsis {
                        self.advance();
                        let inner = self.parse_binding_pattern();
                        items.push(Some(Pattern::Rest(Box::new(inner), start)));
                        // Rest must be last — consume remaining up to RBracket
                        while self.tok.kind != TokenKind::RBracket
                            && self.tok.kind != TokenKind::Eof
                        {
                            if self.tok.kind == TokenKind::Comma {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                        break;
                    }
                    let mut pattern = self.parse_binding_pattern();
                    if self.tok.kind == TokenKind::EqAssign {
                        self.advance();
                        let default = self.parse_expr(0);
                        pattern = Pattern::Default(Box::new(pattern), Box::new(default));
                    }
                    items.push(Some(pattern));
                    if self.tok.kind == TokenKind::Comma {
                        self.advance();
                    } else {
                        break;
                    }
                }
                self.expect(TokenKind::RBracket);
                Pattern::Array(
                    items,
                    Span {
                        start: start.start,
                        end: self.span().end,
                    },
                )
            }
            _ => {
                let t = self.tok.clone();
                self.advance();
                Pattern::Identifier(
                    t.value.into_boxed_str(),
                    Span {
                        start: t.span.start,
                        end: t.span.end,
                    },
                    None,
                )
            }
        }
    }

    /// Parse postfix operations: calls, member access, etc.
    fn parse_postfix(&mut self, mut lhs: Expr) -> Expr {
        loop {
            match self.tok.kind {
                TokenKind::LParen => {
                    self.advance();
                    let mut args = Vec::new();
                    while self.tok.kind != TokenKind::RParen && self.tok.kind != TokenKind::Eof {
                        let arg_start = self.span();
                        let is_spread = self.tok.kind == TokenKind::Ellipsis;
                        if is_spread {
                            self.advance();
                        }
                        let expr = self.parse_expr(0);
                        args.push(ArrayElement {
                            expr,
                            is_spread,
                            span: Span {
                                start: arg_start.start,
                                end: self.span().end,
                            },
                        });
                        if self.tok.kind == TokenKind::Comma {
                            self.advance();
                        }
                    }
                    self.expect(TokenKind::RParen);
                    let span = self.span();
                    lhs = Expr::Call(Box::new(lhs), args, span);
                }
                TokenKind::Dot => {
                    self.advance();
                    let name = if self.tok.kind == TokenKind::Identifier
                        || matches!(self.tok.kind,
                            TokenKind::Catch | TokenKind::Finally | TokenKind::Class
                            | TokenKind::Const | TokenKind::Delete | TokenKind::Do
                            | TokenKind::Else | TokenKind::Export | TokenKind::Extends
                            | TokenKind::For | TokenKind::Function | TokenKind::If
                            | TokenKind::Import | TokenKind::Let | TokenKind::New
                            | TokenKind::Return | TokenKind::Switch | TokenKind::This
                            | TokenKind::Throw | TokenKind::Try | TokenKind::Var
                            | TokenKind::While | TokenKind::Yield | TokenKind::Await
                            | TokenKind::Async | TokenKind::Default | TokenKind::Case
                            | TokenKind::Instanceof | TokenKind::In | TokenKind::Void
                            | TokenKind::Typeof | TokenKind::Break | TokenKind::Continue
                            | TokenKind::Super)
                    {
                        let t = self.tok.clone();
                        self.advance();
                        Expr::String(t.value.into_boxed_str(), t.span)
                    } else {
                        Expr::Undefined(self.span())
                    };
                    let span = self.span();
                    lhs = Expr::Member(Box::new(lhs), Box::new(name), false, span);
                }
                TokenKind::LBracket => {
                    self.advance();
                    let index = self.parse_expr(0);
                    self.expect(TokenKind::RBracket);
                    let span = self.span();
                    lhs = Expr::Member(Box::new(lhs), Box::new(index), true, span);
                }
                TokenKind::PlusPlus => {
                    self.advance();
                    let span = self.span();
                    lhs = Expr::Update(UpdateOp::PlusPlus, Box::new(lhs), false, span);
                }
                TokenKind::MinusMinus => {
                    self.advance();
                    let span = self.span();
                    lhs = Expr::Update(UpdateOp::MinusMinus, Box::new(lhs), false, span);
                }
                _ => break,
            }
        }
        lhs
    }

    /// Parse member-access tail (dot and bracket) only — no calls.
    /// Used by `new` to avoid consuming `(` as a call.
    fn parse_member_tail(&mut self, mut lhs: Expr) -> Expr {
        loop {
            match self.tok.kind {
                TokenKind::Dot => {
                    self.advance();
                    let name = if self.tok.kind == TokenKind::Identifier
                        || matches!(self.tok.kind,
                            TokenKind::Catch | TokenKind::Finally | TokenKind::Class
                            | TokenKind::Const | TokenKind::Delete | TokenKind::Do
                            | TokenKind::Else | TokenKind::Export | TokenKind::Extends
                            | TokenKind::For | TokenKind::Function | TokenKind::If
                            | TokenKind::Import | TokenKind::Let | TokenKind::New
                            | TokenKind::Return | TokenKind::Switch | TokenKind::This
                            | TokenKind::Throw | TokenKind::Try | TokenKind::Var
                            | TokenKind::While | TokenKind::Yield | TokenKind::Await
                            | TokenKind::Async | TokenKind::Default | TokenKind::Case
                            | TokenKind::Instanceof | TokenKind::In | TokenKind::Void
                            | TokenKind::Typeof | TokenKind::Break | TokenKind::Continue
                            | TokenKind::Super)
                    {
                        let t = self.tok.clone();
                        self.advance();
                        Expr::String(t.value.into_boxed_str(), t.span)
                    } else {
                        Expr::Undefined(self.span())
                    };
                    let span = self.span();
                    lhs = Expr::Member(Box::new(lhs), Box::new(name), false, span);
                }
                TokenKind::LBracket => {
                    self.advance();
                    let index = self.parse_expr(0);
                    self.expect(TokenKind::RBracket);
                    let span = self.span();
                    lhs = Expr::Member(Box::new(lhs), Box::new(index), true, span);
                }
                _ => break,
            }
        }
        lhs
    }

    /// Parse an arrow function body after the `=>`.
    /// `params` is the list of parameter names already parsed.
    fn parse_arrow_body(
        &mut self,
        params: Vec<Pattern>,
        rest_param: Option<Box<str>>,
        start: Span,
    ) -> Expr {
        self.expect(TokenKind::Arrow);
        let body = if self.tok.kind == TokenKind::LBrace {
            // Block body: { ... }
            self.parse_block()
        } else {
            // Expression body: implicitly returned
            let expr = self.parse_expr(0);
            Stmt::Expr(expr, self.span())
        };
        Expr::Function(
            Box::new(FnNode {
                name: None,
                params,
                rest_param,
                body,
                is_generator: false,
                is_async: false,
                is_arrow: true,
                span: Span {
                    start: start.start,
                    end: self.span().end,
                },
            }),
            Span {
                start: start.start,
                end: self.span().end,
            },
        )
    }

    fn parse_assign_op(&mut self) -> BinaryOp {
        match self.tok.kind {
            TokenKind::EqAssign => {
                self.advance();
                BinaryOp::Assign
            }
            TokenKind::PlusAssign => {
                self.advance();
                BinaryOp::Add
            }
            TokenKind::MinusAssign => {
                self.advance();
                BinaryOp::Sub
            }
            TokenKind::StarAssign => {
                self.advance();
                BinaryOp::Mul
            }
            TokenKind::SlashAssign => {
                self.advance();
                BinaryOp::Div
            }
            TokenKind::PercentAssign => {
                self.advance();
                BinaryOp::Mod
            }
            TokenKind::StarStarAssign => {
                self.advance();
                BinaryOp::Exp
            }
            TokenKind::ShlAssign => {
                self.advance();
                BinaryOp::Shl
            }
            TokenKind::ShrAssign => {
                self.advance();
                BinaryOp::Shr
            }
            TokenKind::ShrUAssign => {
                self.advance();
                BinaryOp::ShrU
            }
            TokenKind::BitAndAssign => {
                self.advance();
                BinaryOp::BitAnd
            }
            TokenKind::BitOrAssign => {
                self.advance();
                BinaryOp::BitOr
            }
            TokenKind::BitXorAssign => {
                self.advance();
                BinaryOp::BitXor
            }
            _ => BinaryOp::Assign,
        }
    }

    fn binary_precedence(&self) -> u32 {
        match self.tok.kind {
            TokenKind::LogicalOr => 1,
            TokenKind::LogicalAnd => 2,
            TokenKind::BitOr => 3,
            TokenKind::BitXor => 4,
            TokenKind::BitAnd => 5,
            TokenKind::Eq | TokenKind::Ne | TokenKind::StrictEq | TokenKind::StrictNe => 6,
            TokenKind::Lt
            | TokenKind::Gt
            | TokenKind::Le
            | TokenKind::Ge
            | TokenKind::Instanceof
            | TokenKind::In => 7,
            TokenKind::Shl | TokenKind::Shr | TokenKind::ShrU => 8,
            TokenKind::Plus | TokenKind::Minus => 9,
            TokenKind::Star | TokenKind::Slash | TokenKind::Percent => 10,
            TokenKind::StarStar => 11,
            TokenKind::EqAssign
            | TokenKind::PlusAssign
            | TokenKind::MinusAssign
            | TokenKind::StarAssign
            | TokenKind::SlashAssign
            | TokenKind::PercentAssign
            | TokenKind::StarStarAssign
            | TokenKind::ShlAssign
            | TokenKind::ShrAssign
            | TokenKind::ShrUAssign
            | TokenKind::BitAndAssign
            | TokenKind::BitOrAssign
            | TokenKind::BitXorAssign
            | TokenKind::AndAssign
            | TokenKind::OrAssign
            | TokenKind::NullishAssign => 0,
            _ => 0,
        }
    }

    fn consume_semicolon(&mut self) {
        if self.tok.kind == TokenKind::Semicolon {
            self.advance();
        } else if self.lexer.has_semicolon_or_asi() {
            // ASI is automatic — just proceed
        }
    }

    fn has_semicolon_or_asi(&mut self) -> bool {
        self.tok.kind == TokenKind::Semicolon
            || self.tok.kind == TokenKind::RBrace
            || self.tok.kind == TokenKind::Eof
            || self.lexer.has_semicolon_or_asi()
    }

    fn tok_can_start_expr(&self) -> bool {
        matches!(
            self.tok.kind,
            TokenKind::Number
                | TokenKind::String
                | TokenKind::TemplateNoSub
                | TokenKind::TemplateHead
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Null
                | TokenKind::This
                | TokenKind::Identifier
                | TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::LBrace
                | TokenKind::Function
                | TokenKind::Class
                | TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Not
                | TokenKind::BitNot
                | TokenKind::Typeof
                | TokenKind::Void
                | TokenKind::Delete
                | TokenKind::PlusPlus
                | TokenKind::MinusMinus
                | TokenKind::Yield
                | TokenKind::Await
                | TokenKind::New
                | TokenKind::Super
                | TokenKind::Slash
                | TokenKind::Star
        )
    }
}

impl std::fmt::Debug for Parser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Parser").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Program {
        let mut p = Parser::new(source);
        let prog = p.parse();
        if !p.errors.is_empty() {
            panic!("Parse errors: {:?}", p.errors);
        }
        prog
    }

    #[test]
    fn test_number_literal() {
        let prog = parse("42;");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_binary_expr() {
        let prog = parse("1 + 2;");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_var_decl() {
        let prog = parse("var x = 10;");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_if_statement() {
        let prog = parse("if (true) { 1; } else { 2; }");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_while_loop() {
        let prog = parse("while (x) { x = x - 1; }");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_for_loop() {
        let prog = parse("for (var i = 0; i < 10; i = i + 1) { }");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_function_decl() {
        let prog = parse("function add(a, b) { return a + b; }");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_call_expr() {
        let prog = parse("add(1, 2);");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_member_access() {
        let prog = parse("obj.prop;");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_computed_member() {
        let prog = parse("obj[prop];");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_precedence() {
        let prog = parse("1 + 2 * 3;");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_nested_function() {
        let prog = parse("function f() { function g() {} }");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_object_literal() {
        let prog = parse("({a: 1, b: 2});");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_array_literal() {
        let prog = parse("[1, 2, 3];");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_unary_not() {
        let prog = parse("!true;");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_complex_expression() {
        let prog = parse("(1 + 2) * (3 - 4) / 5;");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_break_continue() {
        let prog = parse("while (true) { break; }");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_asi_inserts_semicolon() {
        let prog = parse("return\n42;");
        // With ASI, this becomes: return; 42; — two statements
        assert_eq!(prog.body.len(), 2);
    }

    #[test]
    fn test_nested_blocks() {
        let prog = parse("{{1;}}");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_multiple_vars() {
        let prog = parse("var a = 1, b = 2;");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_string_literal() {
        let prog = parse("\"hello world\";");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_template_literal() {
        let prog = parse("`hello`;");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_division_not_regex() {
        // The parser should handle `a / b` as division
        let prog = parse("a / b;");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_unary_minus() {
        let prog = parse("-42;");
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn test_typeof() {
        let prog = parse("typeof x;");
        assert_eq!(prog.body.len(), 1);
    }
}
