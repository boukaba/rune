use crate::ast::Span;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    // Literals
    Number,
    String,
    Template,
    TemplateHead,
    TemplateMiddle,
    TemplateTail,
    TemplateNoSub,
    Identifier,
    True,
    False,
    Null,
    This,
    // Keywords
    Function,
    Var,
    Let,
    Const,
    If,
    Else,
    While,
    For,
    Do,
    Return,
    Break,
    Continue,
    New,
    Typeof,
    Void,
    Delete,
    In,
    Instanceof,
    Class,
    Extends,
    Super,
    Import,
    Export,
    Default,
    Try,
    Catch,
    Finally,
    Throw,
    Switch,
    Case,
    Await,
    Yield,
    Async,
    Of,
    // Punctuators
    LParen,   // (
    RParen,   // )
    LBrace,   // {
    RBrace,   // }
    LBracket, // [
    RBracket, // ]
    Comma,
    Dot,
    Semicolon,
    Colon,
    Question,
    Arrow,
    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    StarStar,   // **
    PlusPlus,   // ++
    MinusMinus, // --
    Eq,         // ==
    Ne,         // !=
    StrictEq,   // ===
    StrictNe,   // !===
    Lt,
    Gt,
    Le,
    Ge,
    EqAssign,    // =
    PlusAssign,  // +=
    MinusAssign, // -=
    StarAssign,
    SlashAssign,
    PercentAssign,
    StarStarAssign,
    ShlAssign,
    ShrAssign,
    ShrUAssign,
    BitAndAssign,
    BitOrAssign,
    BitXorAssign,
    LogicalAnd,
    LogicalOr,
    BitAnd,
    BitOr,
    BitXor,
    BitNot,
    Shl,
    Shr,
    ShrU,
    Not,
    AndAssign,        // &&=
    OrAssign,         // ||=
    QuestionQuestion, // ??
    NullishAssign,    // ??=
    Ellipsis,         // ...
    // Misc
    Eof,
    Illegal,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    pub value: String,
}

impl Token {
    pub fn new(kind: TokenKind, start: usize, end: usize, value: String) -> Self {
        Token {
            kind,
            span: Span { start, end },
            value,
        }
    }
}

pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
    start: usize,
    pub errors: Vec<String>,
    pub had_newline: bool,
    template_brace_stack: Vec<usize>,
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        Lexer {
            chars: source.chars().collect(),
            pos: 0,
            start: 0,
            errors: Vec::new(),
            had_newline: false,
            template_brace_stack: Vec::new(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        Some(ch)
    }

    #[allow(dead_code)]
    fn skip_line_terminator(&mut self) {
        while let Some(ch) = self.peek() {
            if ch == '\n' || ch == '\r' || ch == '\u{2028}' || ch == '\u{2029}' {
                self.advance();
                if ch == '\r' && self.peek() == Some('\n') {
                    self.advance();
                }
            } else {
                break;
            }
        }
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.peek() {
                Some(ch) if ch.is_ascii_whitespace() && ch != '\n' && ch != '\r' => {
                    self.advance();
                }
                Some('\n') | Some('\r') => {
                    self.had_newline = true;
                    self.advance();
                }
                Some('/') if self.peek_next() == Some('/') => {
                    // Line comment
                    self.advance();
                    self.advance();
                    while let Some(ch) = self.peek() {
                        if ch == '\n' || ch == '\r' {
                            break;
                        }
                        self.advance();
                    }
                }
                Some('/') if self.peek_next() == Some('*') => {
                    // Block comment
                    self.advance();
                    self.advance();
                    while let (Some(a), Some(b)) = (self.advance(), self.peek()) {
                        if a == '*' && b == '/' {
                            self.advance();
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    fn scan_number(&mut self, first: char) -> Token {
        self.start = self.pos - 1;
        let mut is_hex = false;

        if first == '0' {
            match self.peek() {
                Some('x') | Some('X') => {
                    is_hex = true;
                    self.advance();
                    while let Some(ch) = self.peek() {
                        if ch.is_ascii_hexdigit() || ch == '_' {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                }
                Some('o') | Some('O') | Some('b') | Some('B') => {
                    self.advance();
                    while let Some(ch) = self.peek() {
                        if ch.is_alphanumeric() || ch == '_' {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                }
                _ => {
                    // decimal or octal
                    while let Some(ch) = self.peek() {
                        if ch.is_ascii_digit() || ch == '_' {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                }
            }
        } else {
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() || ch == '_' {
                    self.advance();
                } else {
                    break;
                }
            }
        }

        // Decimal fraction?
        if !is_hex
            && self.peek() == Some('.')
            && self.peek_next().is_some_and(|c| c.is_ascii_digit())
        {
            self.advance();
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() || ch == '_' {
                    self.advance();
                } else {
                    break;
                }
            }
        }

        // Exponent?
        if !is_hex && matches!(self.peek(), Some('e') | Some('E')) {
            self.advance();
            if matches!(self.peek(), Some('+') | Some('-')) {
                self.advance();
            }
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() {
                    self.advance();
                } else {
                    break;
                }
            }
        }

        Token::new(TokenKind::Number, self.start, self.pos, self.source_slice())
    }

    fn scan_string(&mut self) -> Token {
        self.start = self.pos - 1;
        let quote = self.chars[self.start];
        let mut value = String::new();
        loop {
            match self.advance() {
                None | Some('\n') | Some('\r') => {
                    self.errors.push("Unterminated string literal".into());
                    break;
                }
                Some(ch) if ch == quote => break,
                Some('\\') => {
                    match self.advance() {
                        Some('n') => value.push('\n'),
                        Some('t') => value.push('\t'),
                        Some('r') => value.push('\r'),
                        Some('b') => value.push('\u{0008}'),
                        Some('f') => value.push('\u{000C}'),
                        Some('v') => value.push('\u{000B}'),
                        Some('0') => value.push('\0'),
                        Some('x') => {
                            let hex = self
                                .chars
                                .get(self.pos..self.pos + 2)
                                .map(|s| s.iter().collect::<String>());
                            if let Some(hex) = hex
                                && let Ok(codepoint) = u32::from_str_radix(&hex, 16)
                            {
                                value.push(char::from_u32(codepoint).unwrap_or('\u{FFFD}'));
                                self.pos += 2;
                            }
                        }
                        Some('u') => {
                            if self.peek() == Some('{') {
                                // Unicode code point escape
                                self.advance();
                                let mut code = String::new();
                                while let Some(ch) = self.peek() {
                                    if ch == '}' {
                                        break;
                                    }
                                    code.push(ch);
                                    self.advance();
                                }
                                if self.peek() == Some('}') {
                                    self.advance();
                                }
                                if let Ok(cp) = u32::from_str_radix(&code, 16) {
                                    value.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                                }
                            } else {
                                let hex = self
                                    .chars
                                    .get(self.pos..self.pos + 4)
                                    .map(|s| s.iter().collect::<String>());
                                if let Some(hex) = hex
                                    && let Ok(codepoint) = u32::from_str_radix(&hex, 16)
                                {
                                    value.push(char::from_u32(codepoint).unwrap_or('\u{FFFD}'));
                                    self.pos += 4;
                                }
                            }
                        }
                        Some(ch) => value.push(ch),
                        None => {}
                    }
                }
                Some(ch) => value.push(ch),
            }
        }
        let end = self.pos;
        Token::new(TokenKind::String, self.start, end, value)
    }

    fn scan_template(&mut self) -> Token {
        self.start = self.pos - 1;
        let mut value = String::new();
        loop {
            match self.advance() {
                None => {
                    self.errors.push("Unterminated template literal".into());
                    break;
                }
                Some('`') => {
                    let end = self.pos;
                    return Token::new(TokenKind::TemplateNoSub, self.start, end, value);
                }
                Some('$') if self.peek() == Some('{') => {
                    self.advance(); // consume {
                    self.template_brace_stack.push(0);
                    let end = self.pos;
                    return Token::new(TokenKind::TemplateHead, self.start, end, value);
                }
                Some('\\') => match self.advance() {
                    Some('n') => value.push('\n'),
                    Some('t') => value.push('\t'),
                    Some('r') => value.push('\r'),
                    Some('b') => value.push('\u{0008}'),
                    Some('f') => value.push('\u{000C}'),
                    Some('v') => value.push('\u{000B}'),
                    Some('0') => value.push('\0'),
                    Some('`') => value.push('`'),
                    Some('$') => value.push('$'),
                    Some('\\') => value.push('\\'),
                    Some('x') => {
                        let hex = self
                            .chars
                            .get(self.pos..self.pos + 2)
                            .map(|s| s.iter().collect::<String>());
                        if let Some(hex) = hex
                            && let Ok(codepoint) = u32::from_str_radix(&hex, 16)
                        {
                            value.push(char::from_u32(codepoint).unwrap_or('\u{FFFD}'));
                            self.pos += 2;
                        }
                    }
                    Some('u') => {
                        if self.peek() == Some('{') {
                            self.advance();
                            let mut code = String::new();
                            while let Some(ch) = self.peek() {
                                if ch == '}' {
                                    break;
                                }
                                code.push(ch);
                                self.advance();
                            }
                            if self.peek() == Some('}') {
                                self.advance();
                            }
                            if let Ok(cp) = u32::from_str_radix(&code, 16) {
                                value.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                            }
                        } else {
                            let hex = self
                                .chars
                                .get(self.pos..self.pos + 4)
                                .map(|s| s.iter().collect::<String>());
                            if let Some(hex) = hex
                                && let Ok(codepoint) = u32::from_str_radix(&hex, 16)
                            {
                                value.push(char::from_u32(codepoint).unwrap_or('\u{FFFD}'));
                                self.pos += 4;
                            }
                        }
                    }
                    Some(c) => value.push(c),
                    None => {}
                },
                Some(c) => value.push(c),
            }
        }
        let end = self.pos;
        Token::new(TokenKind::TemplateNoSub, self.start, end, value)
    }

    fn scan_template_continuation(&mut self, start: usize) -> Token {
        let mut value = String::new();
        loop {
            match self.advance() {
                None => {
                    self.errors.push("Unterminated template literal".into());
                    break;
                }
                Some('`') => {
                    let end = self.pos;
                    return Token::new(TokenKind::TemplateTail, start, end, value);
                }
                Some('$') if self.peek() == Some('{') => {
                    self.advance(); // consume {
                    self.template_brace_stack.push(0);
                    let end = self.pos;
                    return Token::new(TokenKind::TemplateMiddle, start, end, value);
                }
                Some('\\') => match self.advance() {
                    Some('n') => value.push('\n'),
                    Some('t') => value.push('\t'),
                    Some('r') => value.push('\r'),
                    Some('b') => value.push('\u{0008}'),
                    Some('f') => value.push('\u{000C}'),
                    Some('v') => value.push('\u{000B}'),
                    Some('0') => value.push('\0'),
                    Some('`') => value.push('`'),
                    Some('$') => value.push('$'),
                    Some('\\') => value.push('\\'),
                    Some('x') => {
                        let hex = self
                            .chars
                            .get(self.pos..self.pos + 2)
                            .map(|s| s.iter().collect::<String>());
                        if let Some(hex) = hex
                            && let Ok(codepoint) = u32::from_str_radix(&hex, 16)
                        {
                            value.push(char::from_u32(codepoint).unwrap_or('\u{FFFD}'));
                            self.pos += 2;
                        }
                    }
                    Some('u') => {
                        if self.peek() == Some('{') {
                            self.advance();
                            let mut code = String::new();
                            while let Some(ch) = self.peek() {
                                if ch == '}' {
                                    break;
                                }
                                code.push(ch);
                                self.advance();
                            }
                            if self.peek() == Some('}') {
                                self.advance();
                            }
                            if let Ok(cp) = u32::from_str_radix(&code, 16) {
                                value.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                            }
                        } else {
                            let hex = self
                                .chars
                                .get(self.pos..self.pos + 4)
                                .map(|s| s.iter().collect::<String>());
                            if let Some(hex) = hex
                                && let Ok(codepoint) = u32::from_str_radix(&hex, 16)
                            {
                                value.push(char::from_u32(codepoint).unwrap_or('\u{FFFD}'));
                                self.pos += 4;
                            }
                        }
                    }
                    Some(c) => value.push(c),
                    None => {}
                },
                Some(c) => value.push(c),
            }
        }
        let end = self.pos;
        Token::new(TokenKind::TemplateTail, start, end, value)
    }

    fn scan_identifier(&mut self, _first: char) -> Token {
        self.start = self.pos - 1;
        while let Some(ch) = self.peek() {
            if ch.is_alphanumeric() || ch == '_' || ch == '$' || ch > '\u{007F}' {
                self.advance();
            } else {
                break;
            }
        }
        let word = self.source_slice();
        let kind = match word.as_str() {
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "null" => TokenKind::Null,
            "this" => TokenKind::This,
            "function" => TokenKind::Function,
            "var" => TokenKind::Var,
            "let" => TokenKind::Let,
            "const" => TokenKind::Const,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "do" => TokenKind::Do,
            "return" => TokenKind::Return,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "new" => TokenKind::New,
            "typeof" => TokenKind::Typeof,
            "void" => TokenKind::Void,
            "delete" => TokenKind::Delete,
            "in" => TokenKind::In,
            "instanceof" => TokenKind::Instanceof,
            "class" => TokenKind::Class,
            "extends" => TokenKind::Extends,
            "super" => TokenKind::Super,
            "import" => TokenKind::Import,
            "export" => TokenKind::Export,
            "default" => TokenKind::Default,
            "try" => TokenKind::Try,
            "catch" => TokenKind::Catch,
            "finally" => TokenKind::Finally,
            "throw" => TokenKind::Throw,
            "switch" => TokenKind::Switch,
            "case" => TokenKind::Case,
            "await" => TokenKind::Await,
            "yield" => TokenKind::Yield,
            "async" => TokenKind::Async,
            "of" => TokenKind::Of,
            _ => TokenKind::Identifier,
        };
        Token::new(kind, self.start, self.pos, word)
    }

    fn source_slice(&self) -> String {
        self.chars[self.start..self.pos].iter().collect()
    }

    pub fn next_token(&mut self) -> Token {
        self.had_newline = false;
        self.skip_whitespace_and_comments();
        self.start = self.pos;

        let Some(ch) = self.advance() else {
            return Token::new(TokenKind::Eof, self.pos, self.pos, String::new());
        };

        match ch {
            // Punctuation
            '(' => Token::new(TokenKind::LParen, self.start, self.pos, "(".into()),
            ')' => Token::new(TokenKind::RParen, self.start, self.pos, ")".into()),
            '{' => {
                if !self.template_brace_stack.is_empty() {
                    *self.template_brace_stack.last_mut().unwrap() += 1;
                }
                Token::new(TokenKind::LBrace, self.start, self.pos, "{".into())
            }
            '}' => {
                if let Some(depth) = self.template_brace_stack.last_mut() {
                    if *depth > 0 {
                        *depth -= 1;
                        Token::new(TokenKind::RBrace, self.start, self.pos, "}".into())
                    } else {
                        self.template_brace_stack.pop();
                        self.scan_template_continuation(self.pos)
                    }
                } else {
                    Token::new(TokenKind::RBrace, self.start, self.pos, "}".into())
                }
            }
            '[' => Token::new(TokenKind::LBracket, self.start, self.pos, "[".into()),
            ']' => Token::new(TokenKind::RBracket, self.start, self.pos, "]".into()),
            ',' => Token::new(TokenKind::Comma, self.start, self.pos, ",".into()),
            ';' => Token::new(TokenKind::Semicolon, self.start, self.pos, ";".into()),
            ':' => Token::new(TokenKind::Colon, self.start, self.pos, ":".into()),
            '?' => {
                if self.peek() == Some('?') {
                    self.advance();
                    let kind = if self.peek() == Some('=') {
                        self.advance();
                        TokenKind::NullishAssign
                    } else {
                        TokenKind::QuestionQuestion
                    };
                    Token::new(kind, self.start, self.pos, "??".into())
                } else if self.peek() == Some('.')
                    && self.peek_next().is_none_or(|c| !c.is_ascii_digit())
                {
                    // ?. optional chaining — treat as .
                    self.advance();
                    Token::new(TokenKind::Dot, self.start, self.pos, "?.".into())
                } else {
                    Token::new(TokenKind::Question, self.start, self.pos, "?".into())
                }
            }
            '.' => {
                if self.peek() == Some('.') && self.peek_next() == Some('.') {
                    self.advance();
                    self.advance();
                    Token::new(TokenKind::Ellipsis, self.start, self.pos, "...".into())
                } else {
                    Token::new(TokenKind::Dot, self.start, self.pos, ".".into())
                }
            }

            // String
            '\'' | '"' => self.scan_string(),
            '`' => self.scan_template(),

            // Number
            '0'..='9' => self.scan_number(ch),

            // Identifier or keyword
            c if c.is_alphabetic() || c == '_' || c == '$' => self.scan_identifier(ch),

            // Operators
            '+' => {
                if self.peek() == Some('+') {
                    self.advance();
                    Token::new(TokenKind::PlusPlus, self.start, self.pos, "++".into())
                } else if self.peek() == Some('=') {
                    self.advance();
                    Token::new(TokenKind::PlusAssign, self.start, self.pos, "+=".into())
                } else {
                    Token::new(TokenKind::Plus, self.start, self.pos, "+".into())
                }
            }
            '-' => {
                if self.peek() == Some('-') {
                    self.advance();
                    Token::new(TokenKind::MinusMinus, self.start, self.pos, "--".into())
                } else if self.peek() == Some('=') {
                    self.advance();
                    Token::new(TokenKind::MinusAssign, self.start, self.pos, "-=".into())
                } else {
                    Token::new(TokenKind::Minus, self.start, self.pos, "-".into())
                }
            }
            '*' => {
                if self.peek() == Some('*') {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Token::new(
                            TokenKind::StarStarAssign,
                            self.start,
                            self.pos,
                            "**=".into(),
                        )
                    } else {
                        Token::new(TokenKind::StarStar, self.start, self.pos, "**".into())
                    }
                } else if self.peek() == Some('=') {
                    self.advance();
                    Token::new(TokenKind::StarAssign, self.start, self.pos, "*=".into())
                } else {
                    Token::new(TokenKind::Star, self.start, self.pos, "*".into())
                }
            }
            '/' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::new(TokenKind::SlashAssign, self.start, self.pos, "/=".into())
                } else {
                    Token::new(TokenKind::Slash, self.start, self.pos, "/".into())
                }
            }
            '%' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::new(TokenKind::PercentAssign, self.start, self.pos, "%=".into())
                } else {
                    Token::new(TokenKind::Percent, self.start, self.pos, "%".into())
                }
            }
            '=' => {
                if self.peek() == Some('=') {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Token::new(TokenKind::StrictEq, self.start, self.pos, "===".into())
                    } else {
                        Token::new(TokenKind::Eq, self.start, self.pos, "==".into())
                    }
                } else if self.peek() == Some('>') {
                    self.advance();
                    Token::new(TokenKind::Arrow, self.start, self.pos, "=>".into())
                } else {
                    Token::new(TokenKind::EqAssign, self.start, self.pos, "=".into())
                }
            }
            '!' => {
                if self.peek() == Some('=') {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Token::new(TokenKind::StrictNe, self.start, self.pos, "!==".into())
                    } else {
                        Token::new(TokenKind::Ne, self.start, self.pos, "!=".into())
                    }
                } else {
                    Token::new(TokenKind::Not, self.start, self.pos, "!".into())
                }
            }
            '<' => {
                if self.peek() == Some('<') {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Token::new(TokenKind::ShlAssign, self.start, self.pos, "<<=".into())
                    } else {
                        Token::new(TokenKind::Shl, self.start, self.pos, "<<".into())
                    }
                } else if self.peek() == Some('=') {
                    self.advance();
                    Token::new(TokenKind::Le, self.start, self.pos, "<=".into())
                } else {
                    Token::new(TokenKind::Lt, self.start, self.pos, "<".into())
                }
            }
            '>' => {
                if self.peek() == Some('>') {
                    self.advance();
                    if self.peek() == Some('>') {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            Token::new(TokenKind::ShrUAssign, self.start, self.pos, ">>>=".into())
                        } else {
                            Token::new(TokenKind::ShrU, self.start, self.pos, ">>>".into())
                        }
                    } else if self.peek() == Some('=') {
                        self.advance();
                        Token::new(TokenKind::ShrAssign, self.start, self.pos, ">>=".into())
                    } else {
                        Token::new(TokenKind::Shr, self.start, self.pos, ">>".into())
                    }
                } else if self.peek() == Some('=') {
                    self.advance();
                    Token::new(TokenKind::Ge, self.start, self.pos, ">=".into())
                } else {
                    Token::new(TokenKind::Gt, self.start, self.pos, ">".into())
                }
            }
            '&' => {
                if self.peek() == Some('&') {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Token::new(TokenKind::AndAssign, self.start, self.pos, "&&=".into())
                    } else {
                        Token::new(TokenKind::LogicalAnd, self.start, self.pos, "&&".into())
                    }
                } else if self.peek() == Some('=') {
                    self.advance();
                    Token::new(TokenKind::BitAndAssign, self.start, self.pos, "&=".into())
                } else {
                    Token::new(TokenKind::BitAnd, self.start, self.pos, "&".into())
                }
            }
            '|' => {
                if self.peek() == Some('|') {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Token::new(TokenKind::OrAssign, self.start, self.pos, "||=".into())
                    } else {
                        Token::new(TokenKind::LogicalOr, self.start, self.pos, "||".into())
                    }
                } else if self.peek() == Some('=') {
                    self.advance();
                    Token::new(TokenKind::BitOrAssign, self.start, self.pos, "|=".into())
                } else {
                    Token::new(TokenKind::BitOr, self.start, self.pos, "|".into())
                }
            }
            '^' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::new(TokenKind::BitXorAssign, self.start, self.pos, "^=".into())
                } else {
                    Token::new(TokenKind::BitXor, self.start, self.pos, "^".into())
                }
            }
            '~' => Token::new(TokenKind::BitNot, self.start, self.pos, "~".into()),

            _ => Token::new(TokenKind::Illegal, self.start, self.pos, ch.to_string()),
        }
    }

    /// Peek at the next token without consuming it.
    pub fn peek_token(&mut self) -> Token {
        let saved = self.pos;
        let token = self.next_token();
        self.pos = saved;
        token
    }

    /// Check if the next token is a semicolon or would be inserted by ASI.
    pub fn has_semicolon_or_asi(&mut self) -> bool {
        // Only returns true for newline-triggered ASI.
        // Explicit `;`, `}`, and EOF are handled by the parser.
        if self.had_newline {
            return true;
        }
        let saved = self.pos;
        loop {
            match self.peek() {
                Some(ch) if ch.is_ascii_whitespace() && ch != '\n' && ch != '\r' => {
                    self.advance();
                }
                Some('\n') | Some('\r') => {
                    self.pos = saved;
                    return true;
                }
                Some('/') if self.peek_next() == Some('/') => {
                    while let Some(c) = self.advance() {
                        if c == '\n' || c == '\r' {
                            break;
                        }
                    }
                    self.pos = saved;
                    return true;
                }
                Some('/') if self.peek_next() == Some('*') => {
                    self.advance();
                    self.advance();
                    while let (Some(a), Some(b)) = (self.advance(), self.peek()) {
                        if a == '*' && b == '/' {
                            self.advance();
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
        let peek = self.peek();
        let is_eof = peek.is_none();
        self.pos = saved;
        is_eof
    }
}
