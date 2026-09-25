use super::lexer::Token;

// ── AST types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AwkProgram {
    pub rules: Vec<AwkRule>,
    pub functions: Vec<AwkFunction>,
}

/// A user-defined function (`function name(a, b) { ... }`). Extra
/// parameters beyond the call's arguments act as local variables (awk
/// convention). Arrays are passed by reference, scalars by value.
#[derive(Debug, Clone)]
pub struct AwkFunction {
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<AwkStatement>,
}

#[derive(Debug, Clone)]
pub struct AwkRule {
    pub pattern: Option<AwkPattern>,
    pub action: Option<Vec<AwkStatement>>,
}

#[derive(Debug, Clone)]
pub enum AwkPattern {
    Begin,
    End,
    Expression(Expr),
    Regex(String),
    Range(Expr, Expr),
}

/// Output redirection on print/printf statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectKind {
    /// `>` — truncate on first open, then append.
    Truncate,
    /// `>>` — append.
    Append,
    /// `| cmd` — pipe to a command.
    Pipe,
}

/// Input source on a getline expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GetlineSource {
    /// `< file`
    File,
    /// `cmd |`
    Pipe,
}

#[derive(Debug, Clone)]
pub enum AwkStatement {
    Print {
        exprs: Vec<Expr>,
        redirect: Option<(RedirectKind, Expr)>,
    },
    Printf {
        format: Expr,
        exprs: Vec<Expr>,
        redirect: Option<(RedirectKind, Expr)>,
    },
    If {
        cond: Expr,
        then: Box<AwkStatement>,
        else_: Option<Box<AwkStatement>>,
    },
    While {
        cond: Expr,
        body: Box<AwkStatement>,
    },
    DoWhile {
        body: Box<AwkStatement>,
        cond: Expr,
    },
    For {
        init: Option<Box<AwkStatement>>,
        cond: Option<Expr>,
        step: Option<Box<AwkStatement>>,
        body: Box<AwkStatement>,
    },
    ForIn {
        var: String,
        array: String,
        body: Box<AwkStatement>,
    },
    Block(Vec<AwkStatement>),
    Expression(Expr),
    Break,
    Continue,
    Next,
    Exit(Option<Expr>),
    Delete {
        array: String,
        indices: Option<Vec<Expr>>,
    },
    Return(Option<Expr>),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Number(f64),
    String(String),
    Regex(String),
    Var(String),
    FieldRef(Box<Expr>),
    ArrayRef {
        name: String,
        indices: Vec<Expr>,
    },
    BinaryOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Assign {
        target: Box<Expr>,
        op: AssignOp,
        value: Box<Expr>,
    },
    Ternary {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },
    FuncCall {
        name: String,
        args: Vec<Expr>,
    },
    Concat {
        left: Box<Expr>,
        right: Box<Expr>,
    },
    InArray {
        index: Box<Expr>,
        array: String,
    },
    /// Parenthesized comma group as an `in` subscript: `(a, b) in arr`
    /// (keys join with SUBSEP at eval). Only produced in `in` position.
    ParenGroup(Vec<Expr>),
    Match {
        expr: Box<Expr>,
        regex: Box<Expr>,
        negated: bool,
    },
    PreIncrement(Box<Expr>),
    PreDecrement(Box<Expr>),
    PostIncrement(Box<Expr>),
    PostDecrement(Box<Expr>),
    Getline {
        /// Assignment target: a Var, FieldRef, or ArrayRef lvalue
        /// (`getline x`, `getline $2`, `getline arr[i]`); None sets $0.
        target: Option<Box<Expr>>,
        source: Option<(GetlineSource, Box<Expr>)>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOp {
    Neg,
    Pos,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AssignOp {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    ModAssign,
    PowAssign,
}

// ── Parser ─────────────────────────────────────────────────────────────

/// Names that parse as function calls when followed by `(`: awk builtins
/// plus user functions discovered in the token stream (two-pass). An
/// unknown identifier followed by `(` is NOT a call — `x (++i)` is
/// concatenation (gawk requires no space before `(` for user functions,
/// and undefined-function calls are its parse error; we choose the
/// concatenation reading, which matches BWK behavior in practice).
const AWK_BUILTIN_FUNCTIONS: &[&str] = &[
    "atan2", "close", "cos", "exp", "gsub", "index", "int", "length", "log", "match", "rand",
    "sin", "split", "sprintf", "sqrt", "srand", "sub", "substr", "system", "tolower", "toupper",
    "fflush",
];

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// While parsing print/printf arguments, `>` is redirection, not a
    /// comparison (awk grammar: only the parenthesized form is comparison).
    suppress_gt: bool,
    /// User function names found in the token stream (parse is two-pass:
    /// definitions are collected before parsing bodies).
    known_functions: std::collections::HashSet<String>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            suppress_gt: false,
            known_functions: std::collections::HashSet::new(),
        }
    }

    pub fn parse(mut self) -> Result<AwkProgram, String> {
        // Two-pass: collect user function names first so calls to them are
        // recognized regardless of definition order.
        for w in self.tokens.windows(2) {
            if let (Token::Function, Token::Ident(name)) = (&w[0], &w[1]) {
                // gawk: redefinition is a parse-time error (exit 1).
                if !self.known_functions.insert(name.clone()) {
                    return Err(format!("function name `{name}' previously defined"));
                }
            }
        }
        let mut rules = Vec::new();
        let mut functions = Vec::new();
        self.skip_terminators();
        while !self.at_eof() {
            if matches!(self.peek(), Token::Function) {
                functions.push(self.parse_function()?);
            } else {
                rules.push(self.parse_rule()?);
            }
            self.skip_terminators();
        }
        Ok(AwkProgram { rules, functions })
    }

    fn parse_function(&mut self) -> Result<AwkFunction, String> {
        self.advance(); // `function` / `func`
        let name = match self.advance() {
            Token::Ident(n) => n,
            t => return Err(format!("expected function name, got {t}")),
        };

        self.expect(&Token::LParen)?;
        let mut params = Vec::new();
        if !matches!(self.peek(), Token::RParen) {
            loop {
                match self.advance() {
                    Token::Ident(n) => params.push(n),
                    t => return Err(format!("expected parameter name, got {t}")),
                }
                if matches!(self.peek(), Token::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect(&Token::RParen)?;
        self.skip_newlines();
        let body = self.parse_block_body()?;
        Ok(AwkFunction { name, params, body })
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        self.pos += 1;
        tok
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), Token::Eof)
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        let tok = self.advance();
        if std::mem::discriminant(&tok) == std::mem::discriminant(expected) {
            Ok(())
        } else {
            Err(format!("expected {expected}, got {tok}"))
        }
    }

    fn skip_terminators(&mut self) {
        while matches!(self.peek(), Token::Newline | Token::Semicolon) {
            self.advance();
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Token::Newline) {
            self.advance();
        }
    }

    // ── Rule parsing ─────────────────────────────────────────────────

    fn parse_rule(&mut self) -> Result<AwkRule, String> {
        let pattern = self.try_parse_pattern()?;
        self.skip_newlines();
        let action = if matches!(self.peek(), Token::LBrace) {
            Some(self.parse_block_body()?)
        } else {
            None
        };
        // Validate: at least one of pattern or action must be present.
        // Unreachable by construction: try_parse_pattern returns None only when
        // peek is LBrace (an action is then parsed above) or Eof (parse()'s
        // at_eof check prevents parse_rule from being called at all).
        if pattern.is_none() && action.is_none() {
            return Err(format!("expected pattern or action, got {}", self.peek()));
        }
        // gawk: BEGIN/END must have an action (exit 1).
        if matches!(pattern, Some(AwkPattern::Begin | AwkPattern::End)) && action.is_none() {
            return Err("BEGIN/END blocks must have an action part".to_string());
        }
        Ok(AwkRule { pattern, action })
    }

    fn try_parse_pattern(&mut self) -> Result<Option<AwkPattern>, String> {
        match self.peek() {
            Token::Begin => {
                self.advance();
                Ok(Some(AwkPattern::Begin))
            }
            Token::End => {
                self.advance();
                Ok(Some(AwkPattern::End))
            }
            Token::LBrace => Ok(None),
            // Unreachable: parse() only calls parse_rule when !at_eof().
            Token::Eof => Ok(None),
            Token::Regex(_) => {
                let regex = if let Token::Regex(r) = self.advance() {
                    r
                } else {
                    // Unreachable: peek() was just matched as Token::Regex above.
                    unreachable!()
                };
                // Compound boolean pattern: /a/ || /b/ — rewind and parse as
                // an expression (bare Regex in expression context is $0~re).
                if matches!(self.peek(), Token::And | Token::Or) {
                    self.pos -= 1;
                    return self.parse_expr().map(|e| Some(AwkPattern::Expression(e)));
                }
                // Check for range pattern: /regex1/,/regex2/
                if matches!(self.peek(), Token::Comma) {
                    self.advance(); // consume ,
                    self.skip_newlines();
                    let end_expr = self.parse_expr()?;
                    Ok(Some(AwkPattern::Range(Expr::Regex(regex), end_expr)))
                } else {
                    Ok(Some(AwkPattern::Regex(regex)))
                }
            }
            _ => {
                let expr = self.parse_expr()?;
                // Check for range pattern: expr1,expr2
                if matches!(self.peek(), Token::Comma) {
                    // Only treat as range if next is not { — but in awk, comma in pattern
                    // context is always a range. We need to peek ahead.
                    self.advance(); // consume ,
                    self.skip_newlines();
                    let end_expr = self.parse_expr()?;
                    Ok(Some(AwkPattern::Range(expr, end_expr)))
                } else {
                    Ok(Some(AwkPattern::Expression(expr)))
                }
            }
        }
    }

    // ── Block / statement parsing ────────────────────────────────────

    fn parse_block_body(&mut self) -> Result<Vec<AwkStatement>, String> {
        self.expect(&Token::LBrace)?;
        self.skip_terminators();
        let mut stmts = Vec::new();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            stmts.push(self.parse_statement()?);
            self.skip_terminators();
        }
        self.expect(&Token::RBrace)?;
        Ok(stmts)
    }

    fn parse_statement(&mut self) -> Result<AwkStatement, String> {
        match self.peek() {
            Token::Print => self.parse_print(),
            Token::Printf => self.parse_printf(),
            Token::If => self.parse_if(),
            Token::While => self.parse_while(),
            Token::Do => self.parse_do_while(),
            Token::For => self.parse_for(),
            Token::LBrace => {
                let stmts = self.parse_block_body()?;
                Ok(AwkStatement::Block(stmts))
            }
            Token::Break => {
                self.advance();
                Ok(AwkStatement::Break)
            }
            Token::Continue => {
                self.advance();
                Ok(AwkStatement::Continue)
            }
            Token::Next => {
                self.advance();
                Ok(AwkStatement::Next)
            }
            Token::Exit => {
                self.advance();
                let code = if self.is_expr_start() {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                Ok(AwkStatement::Exit(code))
            }
            Token::Return => {
                self.advance();
                let value = if self.is_expr_start() {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                Ok(AwkStatement::Return(value))
            }
            Token::Delete => self.parse_delete(),
            _ => {
                let expr = self.parse_expr()?;
                Ok(AwkStatement::Expression(expr))
            }
        }
    }

    /// Run `f` with `suppress_gt` forced to `value`, restoring the previous
    /// value afterwards — even on error or internal backtracking. Every
    /// scoped reset of the flag goes through here so a backtrack path can
    /// never leak it (the class of bug where a failed paren probe left
    /// `suppress_gt = false` and a following print argument misparsed
    /// `> "/f"` as a comparison).
    fn with_suppress_gt<T>(
        &mut self,
        value: bool,
        f: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        let saved = self.suppress_gt;
        self.suppress_gt = value;
        let result = f(self);
        self.suppress_gt = saved;
        result
    }

    /// Parse one print/printf argument. gawk/BWK extension: a
    /// parenthesized comma group flattens into separate arguments
    /// (`print (a, b)` == `print a, b`); assignments are full expressions
    /// here (`print " ", i /= 10` assigns).
    fn parse_print_arg(&mut self) -> Result<Vec<Expr>, String> {
        if matches!(self.peek(), Token::LParen) {
            let save = self.pos;
            // Inside explicit parens, `>` is a comparison (awk grammar).
            let group = self.with_suppress_gt(false, |p| {
                p.advance();
                let first = p.parse_expr()?;
                if !matches!(p.peek(), Token::Comma) {
                    return Ok(None);
                }
                let mut group = vec![first];
                while matches!(p.peek(), Token::Comma) {
                    p.advance();
                    p.skip_newlines();
                    group.push(p.parse_expr()?);
                }
                p.expect(&Token::RParen)?;
                Ok(Some(group))
            })?;
            if let Some(group) = group {
                return Ok(group);
            }
            self.pos = save; // not a group — reparse as a normal argument
        }
        Ok(vec![self.parse_expr()?])
    }

    fn parse_print(&mut self) -> Result<AwkStatement, String> {
        self.advance(); // consume 'print'
        let exprs = self.with_suppress_gt(true, |p| {
            let mut exprs = Vec::new();
            if p.is_print_expr_start() {
                loop {
                    exprs.extend(p.parse_print_arg()?);
                    if !matches!(p.peek(), Token::Comma) {
                        break;
                    }
                    p.advance();
                    p.skip_newlines();
                }
            }
            Ok(exprs)
        })?;
        let redirect = self.parse_redirect()?;
        Ok(AwkStatement::Print { exprs, redirect })
    }

    fn parse_printf(&mut self) -> Result<AwkStatement, String> {
        self.advance(); // consume 'printf'
        // `printf(fmt, args)` — the whole argument list may be parenthesized
        // (function-call form). parse_print_arg flattens paren groups.
        let (format, exprs) = self.with_suppress_gt(true, |p| {
            let mut all = p.parse_print_arg()?;
            let format = all.remove(0);
            let mut exprs: Vec<Expr> = all;
            while matches!(p.peek(), Token::Comma) {
                p.advance();
                p.skip_newlines();
                exprs.extend(p.parse_print_arg()?);
            }
            Ok((format, exprs))
        })?;
        let redirect = self.parse_redirect()?;
        Ok(AwkStatement::Printf {
            format,
            exprs,
            redirect,
        })
    }

    /// Parse an optional `>`/`>>`/`|` output redirection after print/printf.
    fn parse_redirect(&mut self) -> Result<Option<(RedirectKind, Expr)>, String> {
        let kind = match self.peek() {
            Token::Gt => RedirectKind::Truncate,
            Token::Append => RedirectKind::Append,
            Token::Pipe => RedirectKind::Pipe,
            _ => return Ok(None),
        };
        self.advance();
        let target = self.parse_non_assign_expr()?;
        Ok(Some((kind, target)))
    }

    fn parse_if(&mut self) -> Result<AwkStatement, String> {
        self.advance(); // consume 'if'
        self.expect(&Token::LParen)?;
        let cond = self.parse_expr()?;
        self.expect(&Token::RParen)?;
        self.skip_terminators();
        let then = self.parse_statement()?;
        self.skip_terminators();
        let else_ = if matches!(self.peek(), Token::Else) {
            self.advance();
            self.skip_terminators();
            Some(Box::new(self.parse_statement()?))
        } else {
            None
        };
        Ok(AwkStatement::If {
            cond,
            then: Box::new(then),
            else_,
        })
    }

    fn parse_while(&mut self) -> Result<AwkStatement, String> {
        self.advance(); // consume 'while'
        self.expect(&Token::LParen)?;
        let cond = self.parse_expr()?;
        self.expect(&Token::RParen)?;
        // A lone `;` is the empty loop body — skip only newlines here.
        self.skip_newlines();
        let body = if matches!(self.peek(), Token::Semicolon) {
            self.advance();
            AwkStatement::Expression(Expr::Number(0.0)) // empty body
        } else {
            self.parse_statement()?
        };
        Ok(AwkStatement::While {
            cond,
            body: Box::new(body),
        })
    }

    fn parse_do_while(&mut self) -> Result<AwkStatement, String> {
        self.advance(); // consume 'do'
        self.skip_terminators();
        let body = self.parse_statement()?;
        self.skip_terminators();
        if !matches!(self.peek(), Token::While) {
            return Err("expected 'while' after 'do' body".to_string());
        }
        self.advance();
        self.expect(&Token::LParen)?;
        let cond = self.parse_expr()?;
        self.expect(&Token::RParen)?;
        Ok(AwkStatement::DoWhile {
            body: Box::new(body),
            cond,
        })
    }

    fn parse_for(&mut self) -> Result<AwkStatement, String> {
        self.advance(); // consume 'for'
        self.expect(&Token::LParen)?;

        // Check for for-in: for (var in array)
        if let Token::Ident(name) = self.peek().clone() {
            let saved = self.pos;
            self.advance();
            if matches!(self.peek(), Token::In) {
                self.advance();
                if let Token::Ident(array) = self.advance() {
                    self.expect(&Token::RParen)?;
                    self.skip_terminators();
                    let body = self.parse_statement()?;
                    return Ok(AwkStatement::ForIn {
                        var: name,
                        array,
                        body: Box::new(body),
                    });
                } else {
                    return Err("expected array name in for-in".to_string());
                }
            }
            // Not a for-in, backtrack
            self.pos = saved;
        }

        // C-style for (awk allows newlines after the header semicolons)
        self.skip_newlines();
        let init = if matches!(self.peek(), Token::Semicolon) {
            None
        } else {
            Some(Box::new(self.parse_statement()?))
        };
        self.expect(&Token::Semicolon)?;
        self.skip_newlines();

        let cond = if matches!(self.peek(), Token::Semicolon) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(&Token::Semicolon)?;
        self.skip_newlines();

        let step = if matches!(self.peek(), Token::RParen) {
            None
        } else {
            self.skip_newlines();
            Some(Box::new(self.parse_statement()?))
        };
        self.skip_newlines();
        self.expect(&Token::RParen)?;
        // A lone `;` is the empty loop body — skip only newlines here
        // (skip_terminators would eat the `;` and steal the next statement).
        self.skip_newlines();
        let body = if matches!(self.peek(), Token::Semicolon) {
            self.advance();
            AwkStatement::Expression(Expr::Number(0.0)) // empty body
        } else {
            self.parse_statement()?
        };
        Ok(AwkStatement::For {
            init,
            cond,
            step,
            body: Box::new(body),
        })
    }

    fn parse_delete(&mut self) -> Result<AwkStatement, String> {
        self.advance(); // consume 'delete'
        if let Token::Ident(name) = self.advance() {
            if matches!(self.peek(), Token::LBracket) {
                self.advance();
                let mut indices = Vec::new();
                indices.push(self.parse_expr()?);
                while matches!(self.peek(), Token::Comma) {
                    self.advance();
                    indices.push(self.parse_expr()?);
                }
                self.expect(&Token::RBracket)?;
                Ok(AwkStatement::Delete {
                    array: name,
                    indices: Some(indices),
                })
            } else {
                Ok(AwkStatement::Delete {
                    array: name,
                    indices: None,
                })
            }
        } else {
            Err("expected array name after 'delete'".to_string())
        }
    }

    // ── Expression parsing (precedence climbing) ─────────────────────

    fn is_expr_start(&self) -> bool {
        matches!(
            self.peek(),
            Token::Number(_)
                | Token::StringLit(_)
                | Token::Regex(_)
                | Token::Ident(_)
                | Token::Dollar
                | Token::LParen
                | Token::Not
                | Token::Minus
                | Token::Plus
                | Token::Increment
                | Token::Decrement
        )
    }

    fn is_print_expr_start(&self) -> bool {
        // print arguments can be followed by > or >> or | for redirection
        // but cannot start with those. Check if current token starts an expression.
        self.is_expr_start()
    }

    /// Parse a full expression (including assignments).
    pub fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_assign()
    }

    /// Parse without assignment — used for print arguments to avoid
    /// `print x = 5` being parsed as `print (x = 5)`.
    fn parse_non_assign_expr(&mut self) -> Result<Expr, String> {
        self.parse_ternary()
    }

    fn parse_assign(&mut self) -> Result<Expr, String> {
        let expr = self.parse_ternary()?;
        match self.peek() {
            Token::Assign
            | Token::PlusAssign
            | Token::MinusAssign
            | Token::StarAssign
            | Token::SlashAssign
            | Token::PercentAssign
            | Token::CaretAssign => {
                let op = match self.advance() {
                    Token::Assign => AssignOp::Assign,
                    Token::PlusAssign => AssignOp::AddAssign,
                    Token::MinusAssign => AssignOp::SubAssign,
                    Token::StarAssign => AssignOp::MulAssign,
                    Token::SlashAssign => AssignOp::DivAssign,
                    Token::PercentAssign => AssignOp::ModAssign,
                    Token::CaretAssign => AssignOp::PowAssign,
                    // Unreachable: peek() matched one of the assign tokens above.
                    _ => unreachable!(),
                };
                let value = self.parse_assign()?; // right-associative
                Ok(Expr::Assign {
                    target: Box::new(expr),
                    op,
                    value: Box::new(value),
                })
            }
            _ => Ok(expr),
        }
    }

    /// Parse the optional `var` and `< file` after a `getline` token; when
    /// the getline came from `cmd | getline`, `pipe_source` is that command.
    fn parse_getline_tail(&mut self, pipe_source: Option<Expr>) -> Result<Expr, String> {
        // gawk lvalues: `getline var`, `getline $n`, `getline arr[i]`.
        let target = match self.peek() {
            Token::Ident(name) => {
                let name = name.clone();
                self.advance();
                if matches!(self.peek(), Token::LBracket) {
                    self.advance();
                    let mut indices = vec![self.parse_expr()?];
                    while matches!(self.peek(), Token::Comma) {
                        self.advance();
                        indices.push(self.parse_expr()?);
                    }
                    self.expect(&Token::RBracket)?;
                    Some(Box::new(Expr::ArrayRef { name, indices }))
                } else {
                    Some(Box::new(Expr::Var(name)))
                }
            }
            Token::Dollar => {
                self.advance();
                // Field index is a unary-level expression ($2, $i, $(i+1)).
                let idx = self.parse_unary()?;
                Some(Box::new(Expr::FieldRef(Box::new(idx))))
            }
            _ => None,
        };
        let source = if matches!(self.peek(), Token::Lt) {
            self.advance();
            Some((GetlineSource::File, Box::new(self.parse_non_assign_expr()?)))
        } else {
            pipe_source.map(|e| (GetlineSource::Pipe, Box::new(e)))
        };
        Ok(Expr::Getline { target, source })
    }

    fn peek2(&self) -> Token {
        self.tokens.get(self.pos + 1).cloned().unwrap_or(Token::Eof)
    }

    fn parse_ternary(&mut self) -> Result<Expr, String> {
        let expr = self.parse_or()?;
        // `cmd | getline [var]` — pipe-getline form.
        if matches!(self.peek(), Token::Pipe) && matches!(self.peek2(), Token::Getline) {
            self.advance(); // `|`
            self.advance(); // `getline`
            return self.parse_getline_tail(Some(expr));
        }
        if matches!(self.peek(), Token::Question) {
            self.advance();
            let then = self.parse_assign()?;
            self.expect(&Token::Colon)?;
            let else_ = self.parse_assign()?;
            Ok(Expr::Ternary {
                cond: Box::new(expr),
                then: Box::new(then),
                else_: Box::new(else_),
            })
        } else {
            Ok(expr)
        }
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Token::Or) {
            self.advance();
            self.skip_newlines();
            let right = self.parse_and()?;
            left = Expr::BinaryOp {
                op: BinOp::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_in_expr()?;
        while matches!(self.peek(), Token::And) {
            self.advance();
            self.skip_newlines();
            let right = self.parse_in_expr()?;
            left = Expr::BinaryOp {
                op: BinOp::And,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_in_expr(&mut self) -> Result<Expr, String> {
        // Multi-dimensional membership: `(a, b) in arr` — the key is a
        // parenthesized comma list (only meaningful before `in`).
        if matches!(self.peek(), Token::LParen) && self.peek2() != Token::RParen {
            let save = self.pos;
            let group = self.with_suppress_gt(false, |p| {
                p.advance();
                let first = p.parse_expr()?;
                if !matches!(p.peek(), Token::Comma) {
                    return Ok(None);
                }
                let mut group = vec![first];
                while matches!(p.peek(), Token::Comma) {
                    p.advance();
                    p.skip_newlines();
                    group.push(p.parse_expr()?);
                }
                p.expect(&Token::RParen)?;
                Ok(Some(group))
            })?;
            if let Some(group) = group {
                if matches!(self.peek(), Token::In) {
                    self.advance();
                    return match self.advance() {
                        Token::Ident(array) => Ok(Expr::InArray {
                            index: Box::new(Expr::ParenGroup(group)),
                            array,
                        }),
                        t => Err(format!("expected array name after 'in', got {t}")),
                    };
                }
                // Not an `in` expression — fall through with the group as
                // the left value (gawk errors here; we keep the first item).
                return Ok(Expr::ParenGroup(group));
            }
            self.pos = save;
        }
        let left = self.parse_match()?;
        if matches!(self.peek(), Token::In) {
            self.advance();
            if let Token::Ident(array) = self.advance() {
                return Ok(Expr::InArray {
                    index: Box::new(left),
                    array,
                });
            } else {
                return Err("expected array name after 'in'".to_string());
            }
        }
        Ok(left)
    }

    fn parse_match(&mut self) -> Result<Expr, String> {
        let left = self.parse_comparison()?;
        match self.peek() {
            Token::Match => {
                self.advance();
                let right = self.parse_comparison()?;
                Ok(Expr::Match {
                    expr: Box::new(left),
                    regex: Box::new(right),
                    negated: false,
                })
            }
            Token::NotMatch => {
                self.advance();
                let right = self.parse_comparison()?;
                Ok(Expr::Match {
                    expr: Box::new(left),
                    regex: Box::new(right),
                    negated: true,
                })
            }
            _ => Ok(left),
        }
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_concatenation()?;
        while matches!(
            self.peek(),
            Token::Lt | Token::Le | Token::Gt | Token::Ge | Token::Eq | Token::Ne
        ) {
            if self.suppress_gt && matches!(self.peek(), Token::Gt) {
                // In print/printf argument position, `>` starts an output
                // redirection (see parse_print); leave it unconsumed.
                break;
            }
            let op = match self.advance() {
                Token::Lt => BinOp::Lt,
                Token::Le => BinOp::Le,
                Token::Gt => BinOp::Gt,
                Token::Ge => BinOp::Ge,
                Token::Eq => BinOp::Eq,
                Token::Ne => BinOp::Ne,
                // Unreachable: the while guard only admits comparison tokens.
                _ => unreachable!(),
            };
            self.skip_newlines();
            let right = self.parse_concatenation()?;
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_concatenation(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_addition()?;
        // Implicit concatenation: two adjacent expressions with no operator
        while self.is_concat_start() {
            let right = self.parse_addition()?;
            left = Expr::Concat {
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn is_concat_start(&self) -> bool {
        // Implicit concatenation happens when the next token starts a value
        // but is NOT an operator or terminator
        matches!(
            self.peek(),
            Token::Number(_)
                | Token::StringLit(_)
                | Token::Ident(_)
                | Token::Dollar
                | Token::LParen
                | Token::Not
                | Token::Increment
                | Token::Decrement
        )
    }

    fn parse_addition(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_multiplication()?;
        while matches!(self.peek(), Token::Plus | Token::Minus) {
            let op = if matches!(self.advance(), Token::Plus) {
                BinOp::Add
            } else {
                BinOp::Sub
            };
            self.skip_newlines();
            let right = self.parse_multiplication()?;
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_multiplication(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_power()?;
        while matches!(self.peek(), Token::Star | Token::Slash | Token::Percent) {
            let op = match self.advance() {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                Token::Percent => BinOp::Mod,
                // Unreachable: the while guard only admits *, / and %.
                _ => unreachable!(),
            };
            self.skip_newlines();
            let right = self.parse_power()?;
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_power(&mut self) -> Result<Expr, String> {
        let base = self.parse_unary()?;
        if matches!(self.peek(), Token::Caret) {
            self.advance();
            self.skip_newlines();
            let exp = self.parse_power()?; // right-associative
            Ok(Expr::BinaryOp {
                op: BinOp::Pow,
                left: Box::new(base),
                right: Box::new(exp),
            })
        } else {
            Ok(base)
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Token::Not => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                })
            }
            Token::Minus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                })
            }
            Token::Plus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::Pos,
                    expr: Box::new(expr),
                })
            }
            Token::Increment => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::PreIncrement(Box::new(expr)))
            }
            Token::Decrement => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::PreDecrement(Box::new(expr)))
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek() {
                Token::Increment => {
                    self.advance();
                    expr = Expr::PostIncrement(Box::new(expr));
                }
                Token::Decrement => {
                    self.advance();
                    expr = Expr::PostDecrement(Box::new(expr));
                }
                Token::LBracket => {
                    // Array subscript
                    if let Expr::Var(name) = expr {
                        self.advance();
                        let mut indices = vec![self.parse_expr()?];
                        while matches!(self.peek(), Token::Comma) {
                            self.advance();
                            indices.push(self.parse_expr()?);
                        }
                        self.expect(&Token::RBracket)?;
                        expr = Expr::ArrayRef { name, indices };
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            Token::Number(n) => {
                self.advance();
                Ok(Expr::Number(n))
            }
            Token::StringLit(s) => {
                self.advance();
                Ok(Expr::String(s))
            }
            Token::Regex(r) => {
                self.advance();
                Ok(Expr::Regex(r))
            }
            Token::Dollar => {
                self.advance();
                let expr = self.parse_primary()?;
                Ok(Expr::FieldRef(Box::new(expr)))
            }
            Token::LParen => {
                self.advance();
                // Check for (expr) in array — parenthesized in-expression.
                // Parentheses reset the print-argument `>` suppression: awk
                // requires parens for a real comparison in print position.
                let expr = self.with_suppress_gt(false, |p| p.parse_expr())?;
                self.expect(&Token::RParen)?;
                Ok(expr)
            }
            Token::Ident(name) => {
                self.advance();
                if matches!(self.peek(), Token::LParen)
                    && (AWK_BUILTIN_FUNCTIONS.contains(&name.as_str())
                        || self.known_functions.contains(&name))
                {
                    // Function call. Call arguments are full expressions:
                    // reset the print-argument `>` suppression so
                    // `print substr($0, 1, NF > 1)` parses (awk grammar).
                    self.advance();
                    let args = self.with_suppress_gt(false, |p| {
                        let mut args = Vec::new();
                        if !matches!(p.peek(), Token::RParen) {
                            args.push(p.parse_expr()?);
                            while matches!(p.peek(), Token::Comma) {
                                p.advance();
                                args.push(p.parse_expr()?);
                            }
                        }
                        Ok(args)
                    })?;
                    self.expect(&Token::RParen)?;
                    Ok(Expr::FuncCall { name, args })
                } else if matches!(self.peek(), Token::LBracket) {
                    // gawk: using a function name as an array is a
                    // parse-time error (exit 1).
                    if self.known_functions.contains(&name) {
                        return Err(format!("function `{name}' used as an array"));
                    }
                    self.advance();
                    let mut indices = vec![self.parse_expr()?];
                    while matches!(self.peek(), Token::Comma) {
                        self.advance();
                        indices.push(self.parse_expr()?);
                    }
                    self.expect(&Token::RBracket)?;
                    Ok(Expr::ArrayRef { name, indices })
                } else if name == "length" {
                    // Bare `length` means length($0); gawk reserves the name
                    // (`length = 5` is a syntax error there).
                    Ok(Expr::FuncCall {
                        name,
                        args: Vec::new(),
                    })
                } else {
                    // gawk: using a function name as a variable is a
                    // parse-time error (exit 1).
                    if self.known_functions.contains(&name) {
                        return Err(format!("function `{name}' used as a variable"));
                    }
                    Ok(Expr::Var(name))
                }
            }
            Token::Getline => {
                self.advance();
                self.parse_getline_tail(None)
            }
            _ => Err(format!("unexpected token in expression: {}", self.peek())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::awk::lexer::Lexer;

    fn parse(input: &str) -> AwkProgram {
        let tokens = Lexer::new(input).tokenize().unwrap();
        Parser::new(tokens).parse().unwrap()
    }

    #[test]
    fn parse_simple_print() {
        let prog = parse("{print $1}");
        assert_eq!(prog.rules.len(), 1);
        assert!(prog.rules[0].pattern.is_none());
    }

    #[test]
    fn parse_begin_end() {
        let prog = parse("BEGIN{x=0} {x++} END{print x}");
        assert_eq!(prog.rules.len(), 3);
        assert!(matches!(prog.rules[0].pattern, Some(AwkPattern::Begin)));
        assert!(matches!(prog.rules[2].pattern, Some(AwkPattern::End)));
    }

    #[test]
    fn parse_regex_pattern() {
        let prog = parse("/error/ {print}");
        assert!(matches!(prog.rules[0].pattern, Some(AwkPattern::Regex(_))));
    }

    #[test]
    fn parse_if_else() {
        let prog = parse("{if ($1 > 10) print \"big\"; else print \"small\"}");
        assert_eq!(prog.rules.len(), 1);
    }

    #[test]
    fn parse_for_in() {
        let prog = parse("{for (k in arr) print k}");
        let stmts = prog.rules[0].action.as_ref().unwrap();
        assert!(matches!(stmts[0], AwkStatement::ForIn { .. }));
    }

    #[test]
    fn parse_assignment_ops() {
        let prog = parse("{x += 1; y -= 2; z *= 3}");
        let stmts = prog.rules[0].action.as_ref().unwrap();
        assert_eq!(stmts.len(), 3);
    }

    #[test]
    fn parse_ternary() {
        let prog = parse("{print ($1 > 0) ? \"pos\" : \"neg\"}");
        assert_eq!(prog.rules.len(), 1);
    }

    #[test]
    fn parse_range_pattern() {
        let prog = parse("/start/,/end/ {print}");
        assert!(matches!(prog.rules[0].pattern, Some(AwkPattern::Range(..))));
    }
}
