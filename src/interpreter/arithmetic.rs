//! Arithmetic expression evaluator for `$((...))`, `(( ))`, `let`, and
//! C-style `for (( ; ; ))` loops.
//!
//! Structure: `brush_parser::arithmetic::parse` produces a full expression
//! AST (it is brush's own shell-arithmetic grammar, pinned with the rest of
//! our parser), and this module tree-walks it with rust-bash's variable
//! semantics (wrapping i64, base-N literals, namerefs, array elements,
//! `RANDOM` draws, nounset). Short-circuiting is structural — untaken
//! ternary/&&/|| branches are simply never evaluated — which removes the
//! old textual skip family and its parenthesis-tracking bugs.
//!
//! Pipeline: quoted assoc-subscript placeholder pass → double-quote
//! stripping → literal/character validation (keeps our exact error
//! messages) → brush structural parse → tree-walk evaluation.

use std::collections::HashMap;

use brush_parser::ast::{
    ArithmeticExpr, ArithmeticTarget, BinaryOperator, UnaryAssignmentOperator, UnaryOperator,
};

use crate::error::RustBashError;
use crate::interpreter::{InterpreterState, set_variable};

// ── Public API ──────────────────────────────────────────────────────

/// Evaluate an arithmetic expression string, returning its i64 result.
/// Variables are read from / written to `state.env`.
pub(crate) fn eval_arithmetic(
    expr: &str,
    state: &mut InterpreterState,
) -> Result<i64, RustBashError> {
    let (prepped, placeholders) = substitute_quoted_assoc_subscripts(expr, state);
    let stripped = strip_and_validate(&prepped, state.shopt_opts.strict_arith)?;
    if stripped.trim().is_empty() {
        return Ok(0);
    }
    let ast = brush_parser::arithmetic::parse(&stripped).map_err(|e| {
        RustBashError::Execution(format!("arithmetic: syntax error in expression: {e}"))
    })?;
    let mut evaluator = Evaluator {
        state,
        placeholders: &placeholders,
    };
    evaluator.eval(&ast)
}

// ── Pre-pass 1: quoted associative-array subscripts ─────────────────
//
// bash uses the *verbatim text* of an associative subscript as the key
// (`m["my key"]`, `m['k+1']`), but brush's arithmetic grammar cannot parse
// quotes. Replace `name["..."]`/`name['...']` on variables that are
// associative arrays with a placeholder identifier; the evaluator maps the
// placeholder back to the exact key text.

const PLACEHOLDER_PREFIX: &str = "__RB_ASSOC_KEY_";

fn substitute_quoted_assoc_subscripts(
    expr: &str,
    state: &InterpreterState,
) -> (String, HashMap<String, String>) {
    let bytes = expr.as_bytes();
    let mut out = String::with_capacity(expr.len());
    let mut placeholders = HashMap::new();
    let mut i = 0usize;

    while i < bytes.len() {
        // Identifier followed by `[`?
        if bytes[i] == b'_' || bytes[i].is_ascii_alphabetic() {
            let start = i;
            while i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            let name = &expr[start..i];
            // Only assoc arrays get the placeholder treatment.
            if i < bytes.len() && bytes[i] == b'[' && is_assoc_array(state, name) {
                let mut j = i + 1;
                while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
                    j += 1;
                }
                if j < bytes.len()
                    && matches!(bytes[j], b'\'' | b'"')
                    && let Some((key, end)) = take_quoted_key(expr, j)
                {
                    let ph = format!("{PLACEHOLDER_PREFIX}{}__", placeholders.len());
                    out.push_str(name);
                    out.push('[');
                    out.push_str(&ph);
                    out.push(']');
                    placeholders.insert(ph, key);
                    i = end;
                    continue;
                }
            }
            out.push_str(name);
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    (out, placeholders)
}

/// Read a quoted key starting at `bytes[start]` (a quote char), returning the
/// inner text and the index just past the closing `]`. Returns None if the
/// shape doesn't match.
fn take_quoted_key(expr: &str, start: usize) -> Option<(String, usize)> {
    let bytes = expr.as_bytes();
    let quote = bytes[start];
    let mut i = start + 1;
    // Double quotes unescape \" \\ \$ and \` per bash; single quotes
    // are verbatim.
    let mut inner = String::new();
    while i < bytes.len() && bytes[i] != quote {
        if quote == b'"'
            && bytes[i] == b'\\'
            && i + 1 < bytes.len()
            && matches!(bytes[i + 1], b'"' | b'\\' | b'$' | b'`')
        {
            inner.push(bytes[i + 1] as char);
            i += 2;
            continue;
        }
        inner.push(bytes[i] as char);
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    i += 1; // past the quote
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b']' {
        Some((inner, i + 1))
    } else {
        None
    }
}

// ── Pre-pass 2/3: double-quote stripping + literal/char validation ──
//
// Mirrors the old tokenizer's acceptance rules and exact error messages:
// double-quoted regions are treated as arithmetic (`$(( "1+2" ))` == 3),
// single quotes outside assoc subscripts are an operand error, literals
// (decimal/hex/octal/base#N) are validated with our historical messages.

fn strip_and_validate(input: &str, strict_arith: bool) -> Result<String, RustBashError> {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;

    while i < bytes.len() {
        if is_arithmetic_whitespace(bytes[i]) {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }

        if bytes[i].is_ascii_digit() {
            let start = i;
            let num = parse_number(bytes, &mut i)?;
            if i < bytes.len() && bytes[i] == b'.' {
                return Err(RustBashError::Execution(
                    "arithmetic: syntax error: invalid arithmetic operator".into(),
                ));
            }
            if i < bytes.len() && bytes[i] == b'#' {
                if strict_arith && i - start > 1 && bytes[start] == b'0' {
                    return Err(RustBashError::Execution(format!(
                        "arithmetic: invalid base constant `{}`",
                        std::str::from_utf8(&bytes[start..=i]).unwrap_or("0#")
                    )));
                }
                let base = num;
                i += 1;
                let val_start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'@' || bytes[i] == b'_')
                {
                    i += 1;
                }
                let val_str = std::str::from_utf8(&bytes[val_start..i]).unwrap();
                // Validate with our exact error messages; the brush parser
                // re-parses the same text for the actual value.
                parse_base_n_value(base, val_str)?;
            }
            out.push_str(&input[start..i]);
            continue;
        }

        if bytes[i] == b'\'' {
            return Err(RustBashError::Execution(
                "arithmetic: syntax error: operand expected".into(),
            ));
        }

        if bytes[i] == b'"' {
            // Double-quoted region: contents are arithmetic. Strip the quotes
            // and validate the inner text recursively (same semantics as the
            // old tokenizer's recursive tokenization).
            i += 1;
            let inner_start = i;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                }
                i += 1;
            }
            let inner = &input[inner_start..i];
            if i < bytes.len() {
                i += 1; // closing quote
            }
            let inner_stripped = strip_and_validate(inner, strict_arith)?;
            out.push_str(&inner_stripped);
            continue;
        }

        if bytes[i] == b'$' {
            // Arithmetic treats `$x` / `${x}` / `$1` / `$#` / `$?` like the
            // bare variable (bash expands `$` inside arithmetic). brush's
            // grammar has no `$`, so map them to the plain name; anything
            // else keeps the `$` for the structural parser to reject.
            i += 1;
            if i < bytes.len() && bytes[i] == b'{' {
                let var_start = i + 1;
                let mut j = var_start;
                while j < bytes.len() && bytes[j] != b'}' {
                    j += 1;
                }
                let inner = &input[var_start..j];
                if j < bytes.len()
                    && !inner.is_empty()
                    && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    out.push_str(inner);
                    i = j + 1;
                }
                // Otherwise leave `i` just after `$` so the remainder is
                // validated/parsed normally (and errors if malformed).
                continue;
            }
            if i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphabetic()) {
                let var_start = i;
                while i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                out.push_str(&input[var_start..i]);
                continue;
            }
            if i < bytes.len() && bytes[i].is_ascii_digit() {
                // Positional parameter read, not a number literal — brush
                // can't express it, so use a placeholder the evaluator
                // maps back to read_var("<n>").
                let var_start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                out.push_str("__RB_ARITH_POS_");
                out.push_str(&input[var_start..i]);
                out.push_str("__");
                continue;
            }
            if i < bytes.len() && matches!(bytes[i], b'#' | b'?') {
                // Positional-count/exit-status pseudo targets resolve via
                // read_var's special cases; brush can't parse them, so use
                // a placeholder variable the evaluator maps back.
                let special = if bytes[i] == b'#' {
                    "__RB_ARITH_HASH__"
                } else {
                    "__RB_ARITH_QUESTION__"
                };
                out.push_str(special);
                i += 1;
                continue;
            }
            // Lone `$` (not followed by an ident/digit/`{`/`#`/`?`)
            // produces no token, mirroring the old tokenizer.
            continue;
        }

        if bytes[i] == b'\\' {
            // A backslash-escaped `$` reaches arithmetic through expansion
            // (`\$1`, `\$#`, `\$?`); step onto the `$` and let the dollar
            // branch map it. Any other escape is invalid.
            if i + 1 < bytes.len() && bytes[i + 1] == b'$' {
                // (llvm-cov region artifact: this arm executes — pinned by
                // escaped_dollar_idents_reach_the_tokenizer.)
                i += 1;
                continue;
            }
            return Err(RustBashError::Execution(
                "arithmetic: unexpected character `\\`".into(),
            ));
        }

        if bytes[i] == b'_' || bytes[i].is_ascii_alphabetic() {
            let start = i;
            while i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            let name = &input[start..i];
            // An empty subscript (`a[]`) is a bad-array-subscript error,
            // not a syntax error (bash parity).
            if i < bytes.len() && bytes[i] == b'[' {
                let mut j = i + 1;
                while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b']' {
                    return Err(RustBashError::Execution(format!(
                        "{name}: bad array subscript"
                    )));
                }
            }
            out.push_str(name);
            continue;
        }

        const OPERATOR_CHARS: &[u8] = b"+-*/%&|^~!<>=?():[],";
        if !OPERATOR_CHARS.contains(&bytes[i]) {
            return Err(RustBashError::Execution(format!(
                "arithmetic: unexpected character `{}`",
                bytes[i] as char
            )));
        }

        out.push(bytes[i] as char);
        i += 1;
    }
    Ok(out)
}

fn is_arithmetic_whitespace(byte: u8) -> bool {
    // `\r` is deliberately excluded: most shells (incl. bash) reject it in
    // arithmetic, and the catch-all turns it into "unexpected character".
    matches!(byte, b' ' | b'\t' | b'\n')
}

fn parse_number(bytes: &[u8], i: &mut usize) -> Result<i64, RustBashError> {
    let start = *i;
    if bytes[*i] == b'0' {
        // Hex?
        if *i + 1 < bytes.len() && (bytes[*i + 1] == b'x' || bytes[*i + 1] == b'X') {
            *i += 2;
            let hex_start = *i;
            while *i < bytes.len() && bytes[*i].is_ascii_hexdigit() {
                *i += 1;
            }
            if *i == hex_start {
                return Err(RustBashError::Execution(
                    "arithmetic: invalid hex number".into(),
                ));
            }
            let hex_str = std::str::from_utf8(&bytes[hex_start..*i]).unwrap();
            return i64::from_str_radix(hex_str, 16).map_err(|_| {
                RustBashError::Execution(format!("arithmetic: invalid hex number `0x{hex_str}`"))
            });
        }
        // Octal (leading 0)
        while *i < bytes.len() && bytes[*i].is_ascii_digit() {
            *i += 1;
        }
        let oct_str = std::str::from_utf8(&bytes[start..*i]).unwrap();
        return i64::from_str_radix(oct_str, 8).map_err(|_| {
            RustBashError::Execution(format!("arithmetic: invalid octal number `{oct_str}`"))
        });
    }
    while *i < bytes.len() && bytes[*i].is_ascii_digit() {
        *i += 1;
    }
    let num_str = std::str::from_utf8(&bytes[start..*i]).unwrap();
    num_str.parse::<i64>().map_err(|_| {
        RustBashError::Execution(format!("arithmetic: invalid decimal number `{num_str}`"))
    })
}

fn parse_base_n_value(base: i64, digits: &str) -> Result<i64, RustBashError> {
    if !(2..=64).contains(&base) {
        return Err(RustBashError::Execution(format!(
            "arithmetic: invalid arithmetic base: {base}"
        )));
    }
    if digits.is_empty() {
        return Err(RustBashError::Execution(format!(
            "arithmetic: invalid base constant `{base}#`"
        )));
    }
    let base_u = base as u64;
    let mut result: i64 = 0;
    for ch in digits.chars() {
        let digit_val = match ch {
            '0'..='9' => (ch as u64) - (b'0' as u64),
            'a'..='z' => (ch as u64) - (b'a' as u64) + 10,
            'A'..='Z' => {
                if base_u <= 36 {
                    (ch as u64) - (b'A' as u64) + 10
                } else {
                    (ch as u64) - (b'A' as u64) + 36
                }
            }
            '@' => 62,
            '_' => 63,
            // Unreachable: the caller only accepts [0-9a-zA-Z@_] inside
            // base-N literals, and every one of those is handled above.
            _ => {
                return Err(RustBashError::Execution(format!(
                    "arithmetic: value too great for base: {digits} (base {base})"
                )));
            }
        };
        if digit_val >= base_u {
            return Err(RustBashError::Execution(format!(
                "arithmetic: value too great for base: {digits} (base {base})"
            )));
        }
        result = result.wrapping_mul(base).wrapping_add(digit_val as i64);
    }
    Ok(result)
}

// ── Tree-walk evaluator ─────────────────────────────────────────────

struct Evaluator<'a, 'b> {
    state: &'a mut InterpreterState,
    placeholders: &'b HashMap<String, String>,
}

impl Evaluator<'_, '_> {
    fn eval(&mut self, expr: &ArithmeticExpr) -> Result<i64, RustBashError> {
        match expr {
            ArithmeticExpr::Literal(n) => Ok(*n),
            ArithmeticExpr::Reference(target) => self.read_target(target),
            ArithmeticExpr::UnaryOp(op, inner) => {
                let v = self.eval(inner)?;
                Ok(match op {
                    UnaryOperator::LogicalNot => i64::from(v == 0),
                    UnaryOperator::BitwiseNot => !v,
                    UnaryOperator::UnaryPlus => v,
                    UnaryOperator::UnaryMinus => v.wrapping_neg(),
                })
            }
            ArithmeticExpr::BinaryOp(op, lhs, rhs) => self.eval_binary(op, lhs, rhs),
            // Short-circuiting is structural: only the taken branch is
            // evaluated, so side effects in the other branch never happen.
            ArithmeticExpr::Conditional(cond, then_branch, else_branch) => {
                if self.eval(cond)? != 0 {
                    self.eval(then_branch)
                } else {
                    self.eval(else_branch)
                }
            }
            ArithmeticExpr::Assignment(target, rhs) => {
                let val = self.eval(rhs)?;
                self.write_target(target, val)?;
                Ok(val)
            }
            ArithmeticExpr::BinaryAssignment(op, target, rhs) => {
                let rhs_val = self.eval(rhs)?;
                let lhs_val = self.read_target(target)?;
                let val = apply_binary_arith(op, lhs_val, rhs_val)?;
                self.write_target(target, val)?;
                Ok(val)
            }
            ArithmeticExpr::UnaryAssignment(op, target) => {
                let old = self.read_target(target)?;
                let new = old.wrapping_add(match op {
                    UnaryAssignmentOperator::PrefixIncrement
                    | UnaryAssignmentOperator::PostfixIncrement => 1,
                    UnaryAssignmentOperator::PrefixDecrement
                    | UnaryAssignmentOperator::PostfixDecrement => -1,
                });
                self.write_target(target, new)?;
                Ok(match op {
                    UnaryAssignmentOperator::PrefixIncrement
                    | UnaryAssignmentOperator::PrefixDecrement => new,
                    UnaryAssignmentOperator::PostfixIncrement
                    | UnaryAssignmentOperator::PostfixDecrement => old,
                })
            }
        }
    }

    fn eval_binary(
        &mut self,
        op: &BinaryOperator,
        lhs: &ArithmeticExpr,
        rhs: &ArithmeticExpr,
    ) -> Result<i64, RustBashError> {
        match op {
            BinaryOperator::Comma => {
                self.eval(lhs)?;
                self.eval(rhs)
            }
            // Short-circuit by construction.
            BinaryOperator::LogicalOr => {
                if self.eval(lhs)? != 0 {
                    Ok(1)
                } else {
                    Ok(i64::from(self.eval(rhs)? != 0))
                }
            }
            BinaryOperator::LogicalAnd => {
                if self.eval(lhs)? == 0 {
                    Ok(0)
                } else {
                    Ok(i64::from(self.eval(rhs)? != 0))
                }
            }
            _ => {
                let l = self.eval(lhs)?;
                let r = self.eval(rhs)?;
                apply_binary_arith(op, l, r)
            }
        }
    }

    fn read_target(&mut self, target: &ArithmeticTarget) -> Result<i64, RustBashError> {
        match target {
            ArithmeticTarget::Variable(name) => {
                let mapped = match name.as_str() {
                    "__RB_ARITH_HASH__" => "#",
                    "__RB_ARITH_QUESTION__" => "?",
                    _ => name
                        .strip_prefix("__RB_ARITH_POS_")
                        .and_then(|rest| rest.strip_suffix("__"))
                        .unwrap_or(name.as_str()),
                };
                read_var(self.state, mapped)
            }
            ArithmeticTarget::ArrayElement(name, index) => {
                let resolved = crate::interpreter::resolve_nameref_or_self(name, self.state);
                if is_assoc_array(self.state, &resolved) {
                    let key = self.render_assoc_key(index);
                    // Unreachable: is_assoc_array returned true, so the
                    // (resolved) variable exists and contains_key passes.
                    if self.state.shell_opts.nounset && !self.state.env.contains_key(&resolved) {
                        return Err(RustBashError::Execution(format!(
                            "{name}[{key}]: unbound variable"
                        )));
                    }
                    read_assoc_element(self.state, &resolved, &key)
                } else {
                    if self.state.shell_opts.nounset && !self.state.env.contains_key(&resolved) {
                        return Err(RustBashError::Execution(format!(
                            "{name}[{}]: unbound variable",
                            render_index_for_message(index)
                        )));
                    }
                    let idx = self.eval(index)?;
                    read_indexed_element(self.state, &resolved, idx)
                }
            }
        }
    }

    fn write_target(&mut self, target: &ArithmeticTarget, value: i64) -> Result<(), RustBashError> {
        match target {
            ArithmeticTarget::Variable(name) => set_variable(self.state, name, value.to_string()),
            ArithmeticTarget::ArrayElement(name, index) => {
                let resolved = crate::interpreter::resolve_nameref_or_self(name, self.state);
                if is_assoc_array(self.state, &resolved) {
                    let key = self.render_assoc_key(index);
                    crate::interpreter::set_assoc_element(
                        self.state,
                        &resolved,
                        key,
                        value.to_string(),
                    )
                } else {
                    let idx = self.eval(index)?;
                    write_indexed_element(self.state, &resolved, idx, value)
                }
            }
        }
    }

    /// bash uses the *text* of an associative subscript as the key — no
    /// arithmetic evaluation (`m[k+1]` is the literal key "k+1"). Render the
    /// index AST back to text; placeholders carry the exact quoted key.
    fn render_assoc_key(&self, index: &ArithmeticExpr) -> String {
        match index {
            ArithmeticExpr::Reference(ArithmeticTarget::Variable(name)) => self
                .placeholders
                .get(name)
                .cloned()
                .unwrap_or_else(|| name.clone()),
            ArithmeticExpr::Literal(n) => n.to_string(),
            other => render_index_for_message(other),
        }
    }
}

/// Render an index expression to text for assoc keys / error messages.
/// Note: whitespace is normalized away (brush's AST drops it), so a key
/// written with spaces in the source renders compacted — a documented
/// edge vs bash's verbatim-text semantics.
fn render_index_for_message(expr: &ArithmeticExpr) -> String {
    match expr {
        ArithmeticExpr::Literal(n) => n.to_string(),
        ArithmeticExpr::Reference(ArithmeticTarget::Variable(name)) => name.clone(),
        ArithmeticExpr::Reference(ArithmeticTarget::ArrayElement(name, idx)) => {
            format!("{name}[{}]", render_index_for_message(idx))
        }
        ArithmeticExpr::UnaryOp(op, inner) => {
            let sym = match op {
                UnaryOperator::LogicalNot => "!",
                UnaryOperator::BitwiseNot => "~",
                UnaryOperator::UnaryPlus => "+",
                UnaryOperator::UnaryMinus => "-",
            };
            format!("{sym}{}", render_index_for_message(inner))
        }
        ArithmeticExpr::BinaryOp(op, l, r) => {
            format!(
                "{}{}{}",
                render_index_for_message(l),
                binary_op_symbol(op),
                render_index_for_message(r)
            )
        }
        ArithmeticExpr::Conditional(c, t, f) => format!(
            "{}?{}:{}",
            render_index_for_message(c),
            render_index_for_message(t),
            render_index_for_message(f)
        ),
        ArithmeticExpr::Assignment(t, v) => {
            format!(
                "{}={}",
                render_target_for_message(t),
                render_index_for_message(v)
            )
        }
        ArithmeticExpr::BinaryAssignment(op, t, v) => {
            format!(
                "{}{}={}",
                render_target_for_message(t),
                binary_op_symbol(op),
                render_index_for_message(v)
            )
        }
        ArithmeticExpr::UnaryAssignment(op, t) => {
            let sym = match op {
                UnaryAssignmentOperator::PrefixIncrement
                | UnaryAssignmentOperator::PostfixIncrement => "++",
                UnaryAssignmentOperator::PrefixDecrement
                | UnaryAssignmentOperator::PostfixDecrement => "--",
            };
            match op {
                UnaryAssignmentOperator::PrefixIncrement
                | UnaryAssignmentOperator::PrefixDecrement => {
                    format!("{sym}{}", render_target_for_message(t))
                }
                _ => format!("{}{sym}", render_target_for_message(t)),
            }
        }
    }
}

fn render_target_for_message(target: &ArithmeticTarget) -> String {
    match target {
        ArithmeticTarget::Variable(name) => name.clone(),
        ArithmeticTarget::ArrayElement(name, idx) => {
            format!("{name}[{}]", render_index_for_message(idx))
        }
    }
}

fn binary_op_symbol(op: &BinaryOperator) -> &'static str {
    match op {
        BinaryOperator::Comma => ",",
        BinaryOperator::LogicalOr => "||",
        BinaryOperator::LogicalAnd => "&&",
        BinaryOperator::BitwiseOr => "|",
        BinaryOperator::BitwiseXor => "^",
        BinaryOperator::BitwiseAnd => "&",
        BinaryOperator::Equals => "==",
        BinaryOperator::NotEquals => "!=",
        BinaryOperator::LessThan => "<",
        BinaryOperator::GreaterThan => ">",
        BinaryOperator::LessThanOrEqualTo => "<=",
        BinaryOperator::GreaterThanOrEqualTo => ">=",
        BinaryOperator::ShiftLeft => "<<",
        BinaryOperator::ShiftRight => ">>",
        BinaryOperator::Add => "+",
        BinaryOperator::Subtract => "-",
        BinaryOperator::Multiply => "*",
        BinaryOperator::Modulo => "%",
        BinaryOperator::Divide => "/",
        BinaryOperator::Power => "**",
    }
}

fn apply_binary_arith(op: &BinaryOperator, lhs: i64, rhs: i64) -> Result<i64, RustBashError> {
    match op {
        BinaryOperator::Add => Ok(lhs.wrapping_add(rhs)),
        BinaryOperator::Subtract => Ok(lhs.wrapping_sub(rhs)),
        BinaryOperator::Multiply => Ok(lhs.wrapping_mul(rhs)),
        BinaryOperator::Divide => {
            if rhs == 0 {
                return Err(RustBashError::Execution(
                    "arithmetic: division by zero".into(),
                ));
            }
            Ok(lhs.wrapping_div(rhs))
        }
        BinaryOperator::Modulo => {
            if rhs == 0 {
                return Err(RustBashError::Execution(
                    "arithmetic: division by zero".into(),
                ));
            }
            Ok(lhs.wrapping_rem(rhs))
        }
        BinaryOperator::Power => wrapping_pow(lhs, rhs),
        BinaryOperator::ShiftLeft => Ok(lhs.wrapping_shl(rhs as u32)),
        BinaryOperator::ShiftRight => Ok(lhs.wrapping_shr(rhs as u32)),
        BinaryOperator::BitwiseAnd => Ok(lhs & rhs),
        BinaryOperator::BitwiseOr => Ok(lhs | rhs),
        BinaryOperator::BitwiseXor => Ok(lhs ^ rhs),
        BinaryOperator::Equals => Ok(i64::from(lhs == rhs)),
        BinaryOperator::NotEquals => Ok(i64::from(lhs != rhs)),
        BinaryOperator::LessThan => Ok(i64::from(lhs < rhs)),
        BinaryOperator::GreaterThan => Ok(i64::from(lhs > rhs)),
        BinaryOperator::LessThanOrEqualTo => Ok(i64::from(lhs <= rhs)),
        BinaryOperator::GreaterThanOrEqualTo => Ok(i64::from(lhs >= rhs)),
        // Unreachable: Comma/LogicalAnd/LogicalOr are short-circuited in
        // eval_binary before reaching here.
        _ => unreachable!(),
    }
}

// ── Array element read/write (evaluated indices and assoc keys) ─────

/// Read an associative-array element by string key, then interpret the
/// stored string as an arithmetic value (recursively, as bash does).
fn read_assoc_element(
    state: &mut InterpreterState,
    resolved_name: &str,
    key: &str,
) -> Result<i64, RustBashError> {
    use crate::interpreter::VariableValue;
    let val_str = state
        .env
        .get(resolved_name)
        .and_then(|v| match &v.value {
            VariableValue::AssociativeArray(map) => map.get(key).cloned(),
            // Unreachable: the caller checked is_assoc_array on the same
            // variable and nothing mutates env in between.
            _ => None,
        })
        .unwrap_or_default();
    value_from_string(state, &val_str, &format!("{resolved_name}[{key}]"))
}

/// Read an indexed-array (or scalar) element by evaluated index.
fn read_indexed_element(
    state: &mut InterpreterState,
    resolved_name: &str,
    index: i64,
) -> Result<i64, RustBashError> {
    use crate::interpreter::VariableValue;
    let val_str = match state.env.get(resolved_name) {
        None => return Ok(0),
        Some(v) => match &v.value {
            VariableValue::IndexedArray(map) => {
                let actual_idx = if index < 0 {
                    let max_key = map.keys().next_back().copied().unwrap_or(0);
                    let resolved = max_key as i64 + 1 + index;
                    if resolved < 0 {
                        let ln = state.current_lineno;
                        state.pending_cmdsub_stderr.push_str(&format!(
                            "rust-bash: line {ln}: {resolved_name}: bad array subscript\n"
                        ));
                        return Ok(0);
                    }
                    resolved as usize
                } else {
                    index as usize
                };
                map.get(&actual_idx).cloned().unwrap_or_default()
            }
            VariableValue::Scalar(s) => {
                if index == 0 || index == -1 {
                    s.clone()
                } else {
                    String::new()
                }
            }
            // Unreachable: the caller routes assoc arrays elsewhere.
            VariableValue::AssociativeArray(_) => String::new(),
        },
    };
    value_from_string(state, &val_str, &format!("{resolved_name}[{index}]"))
}

/// Interpret a stored string as an arithmetic value: direct i64 parse, or
/// recursively evaluate it as an expression (bash semantics), with a
/// recursion guard (e.g. `a[0]="a[0]"`).
fn value_from_string(
    state: &mut InterpreterState,
    val_str: &str,
    context: &str,
) -> Result<i64, RustBashError> {
    if val_str.is_empty() {
        return Ok(0);
    }
    match val_str.parse::<i64>() {
        Ok(v) => Ok(v),
        Err(_) => {
            use std::cell::Cell;
            thread_local! {
                static DEPTH: Cell<usize> = const { Cell::new(0) };
            }
            DEPTH.with(|d| {
                let cur = d.get();
                if cur >= 10 {
                    return Err(RustBashError::Execution(format!(
                        "{context}: recursive evaluation depth exceeded"
                    )));
                }
                d.set(cur + 1);
                let result = eval_arithmetic(val_str, state);
                d.set(cur);
                result
            })
        }
    }
}

/// Write to an indexed-array (or scalar) element by evaluated index.
fn write_indexed_element(
    state: &mut InterpreterState,
    resolved_name: &str,
    index: i64,
    value: i64,
) -> Result<(), RustBashError> {
    use crate::interpreter::VariableValue;
    if index < 0 {
        let max_key = state.env.get(resolved_name).and_then(|v| match &v.value {
            VariableValue::IndexedArray(map) => map.keys().next_back().copied(),
            VariableValue::Scalar(_) => Some(0),
            // Unreachable: the caller routes assoc arrays elsewhere.
            VariableValue::AssociativeArray(_) => None,
        });
        return match max_key {
            Some(mk) => {
                let resolved = mk as i64 + 1 + index;
                if resolved < 0 {
                    Err(RustBashError::Execution(format!(
                        "{resolved_name}: bad array subscript"
                    )))
                } else {
                    crate::interpreter::set_array_element(
                        state,
                        resolved_name,
                        resolved as usize,
                        value.to_string(),
                    )
                }
            }
            None => Err(RustBashError::Execution(format!(
                "{resolved_name}: bad array subscript"
            ))),
        };
    }
    crate::interpreter::set_array_element(state, resolved_name, index as usize, value.to_string())
}

// ── Variable resolution helpers ─────────────────────────────────────

fn read_var(state: &mut InterpreterState, name: &str) -> Result<i64, RustBashError> {
    // Handle special parameters
    match name {
        "#" => return Ok(state.positional_params.len() as i64),
        "?" => return Ok(state.last_exit_code as i64),
        "LINENO" => return Ok(state.current_lineno as i64),
        "SECONDS" => return Ok(state.shell_start_time.elapsed().as_secs() as i64),
        _ => {}
    }
    // Handle positional parameters ($0, $1, $2, ...)
    if let Ok(n) = name.parse::<usize>() {
        if n == 0 {
            return Ok(state.shell_name.parse::<i64>().unwrap_or(0));
        }
        return Ok(state
            .positional_params
            .get(n - 1)
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0));
    }
    // Check nounset before resolving
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    if state.shell_opts.nounset && !state.env.contains_key(&resolved) {
        return Err(RustBashError::Execution(format!(
            "{name}: unbound variable"
        )));
    }
    resolve_var_recursive(state, name, 0)
}

fn resolve_var_recursive(
    state: &mut InterpreterState,
    name: &str,
    depth: usize,
) -> Result<i64, RustBashError> {
    const MAX_DEPTH: usize = 10;
    // Call-stack pseudo-variables (BASH_LINENO, etc.) are not stored in env;
    // resolve them via the expansion helper so $((BASH_LINENO)) works.
    if matches!(name, "BASH_LINENO" | "BASH_SOURCE" | "FUNCNAME") {
        let scalar = crate::interpreter::expansion::resolve_call_stack_scalar(name, state);
        return Ok(scalar.parse::<i64>().unwrap_or(0));
    }
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    let val_str = state
        .env
        .get(&resolved)
        .map(|v| v.value.as_scalar().to_string())
        .unwrap_or_default();
    if val_str.is_empty() {
        return Ok(0);
    }
    if let Ok(n) = val_str.parse::<i64>() {
        return Ok(n);
    }
    // If the value looks like a valid variable name, resolve recursively.
    if depth < MAX_DEPTH
        && val_str
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !val_str.chars().next().unwrap_or('0').is_ascii_digit()
    {
        return resolve_var_recursive(state, &val_str, depth + 1);
    }
    // Bash evaluates the variable's string value as an arithmetic expression.
    if depth < MAX_DEPTH {
        return eval_arithmetic(&val_str, state);
    }
    Ok(0)
}

/// Determine if a variable is an associative array.
fn is_assoc_array(state: &InterpreterState, name: &str) -> bool {
    use crate::interpreter::VariableValue;
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    state
        .env
        .get(&resolved)
        .is_some_and(|v| matches!(v.value, VariableValue::AssociativeArray(_)))
}

fn wrapping_pow(mut base: i64, mut exp: i64) -> Result<i64, RustBashError> {
    if exp < 0 {
        return Err(RustBashError::Execution(
            "arithmetic: exponent less than 0".into(),
        ));
    }
    let mut result: i64 = 1;
    while exp > 0 {
        if exp & 1 == 1 {
            result = result.wrapping_mul(base);
        }
        exp >>= 1;
        base = base.wrapping_mul(base);
    }
    Ok(result)
}

// ── Unit tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::{
        ExecutionCounters, ExecutionLimits, InterpreterState, ShellOpts, ShoptOpts,
        new_unresolved_record,
    };
    use crate::vfs::InMemoryFs;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_state() -> InterpreterState {
        InterpreterState {
            fs: Arc::new(InMemoryFs::new()),
            env: HashMap::new(),
            cwd: "/".to_string(),
            functions: HashMap::new(),
            last_exit_code: 0,
            commands: HashMap::new(),
            shell_opts: ShellOpts::default(),
            shopt_opts: ShoptOpts::default(),
            limits: ExecutionLimits::default(),
            counters: ExecutionCounters::default(),
            should_exit: false,
            abort_command_list: false,
            loop_depth: 0,
            control_flow: None,
            positional_params: Vec::new(),
            shell_name: "rust-bash".to_string(),
            shell_pid: 1000,
            bash_pid: 1000,
            parent_pid: 1,
            next_process_id: 1001,
            last_background_pid: None,
            last_background_status: None,
            interactive_shell: false,
            invoked_with_c: false,
            random_seed: 42,
            local_scopes: Vec::new(),
            temp_binding_scopes: Vec::new(),
            in_function_depth: 0,
            source_depth: 0,
            getopts_subpos: 0,
            getopts_args_signature: String::new(),
            traps: HashMap::new(),
            in_trap: false,
            errexit_suppressed: 0,
            errexit_bang_suppressed: 0,
            stdin_offset: 0,
            current_stdin_persistent_fd: None,
            dir_stack: Vec::new(),
            command_hash: HashMap::new(),
            aliases: HashMap::new(),
            current_lineno: 0,
            current_source: "main".to_string(),
            current_source_text: String::new(),
            last_verbose_line: 0,
            shell_start_time: crate::platform::Instant::now(),
            last_argument: String::new(),
            call_stack: Vec::new(),
            machtype: "x86_64-pc-linux-gnu".to_string(),
            hosttype: "x86_64".to_string(),
            persistent_fds: HashMap::new(),
            persistent_fd_offsets: HashMap::new(),
            next_auto_fd: 10,
            proc_sub_counter: 0,
            proc_sub_prealloc: HashMap::new(),
            pipe_stdin_bytes: None,
            pending_cmdsub_stderr: String::new(),
            pending_test_stderr: String::new(),
            script_source: None,
            unresolved_record: new_unresolved_record(),
            abort_on_unresolved: false,
            fatal_expansion_error: false,
            last_command_had_error: false,
            last_status_immune_to_errexit: false,
        }
    }

    fn eval(expr: &str) -> i64 {
        let mut state = make_state();
        eval_arithmetic(expr, &mut state).unwrap()
    }

    fn eval_with(expr: &str, state: &mut InterpreterState) -> i64 {
        eval_arithmetic(expr, state).unwrap()
    }

    #[test]
    fn assoc_unterminated_quoted_key_declines_placeholder() {
        // Direct eval path (no shell tokenizer): the placeholder scan hits
        // the unterminated quote and declines; quote-stripping then leaves
        // an unbalanced `[`, which the structural parser rejects.
        let mut state = make_state();
        state.env.insert(
            "m".to_string(),
            crate::interpreter::Variable {
                value: crate::interpreter::VariableValue::AssociativeArray(
                    std::collections::BTreeMap::new(),
                ),
                attrs: crate::interpreter::VariableAttrs::empty(),
            },
        );
        let result = eval_arithmetic("m[\"k", &mut state);
        let Err(RustBashError::Execution(msg)) = result else {
            // Only evaluated when the assertion fails (test-only path).
            panic!("expected syntax error, got {result:?}");
        };
        assert!(msg.contains("syntax error"), "msg: {msg}");
    }

    #[test]
    fn basic_addition() {
        assert_eq!(eval("1 + 2"), 3);
    }

    #[test]
    fn multiplication() {
        assert_eq!(eval("5 * 3"), 15);
    }

    #[test]
    fn division() {
        assert_eq!(eval("10 / 3"), 3);
    }

    #[test]
    fn modulo() {
        assert_eq!(eval("10 % 3"), 1);
    }

    #[test]
    fn exponentiation() {
        assert_eq!(eval("2 ** 10"), 1024);
    }

    #[test]
    fn precedence_add_mul() {
        assert_eq!(eval("2 + 3 * 4"), 14);
    }

    #[test]
    fn parenthesized() {
        assert_eq!(eval("(1 + 2) * 3"), 9);
    }

    #[test]
    fn comparison_gt() {
        assert_eq!(eval("5 > 3"), 1);
    }

    #[test]
    fn comparison_lt() {
        assert_eq!(eval("5 < 3"), 0);
    }

    #[test]
    fn comparison_le() {
        assert_eq!(eval("3 <= 3"), 1);
    }

    #[test]
    fn comparison_ge() {
        assert_eq!(eval("3 >= 4"), 0);
    }

    #[test]
    fn equality() {
        assert_eq!(eval("5 == 5"), 1);
        assert_eq!(eval("5 != 5"), 0);
        assert_eq!(eval("5 != 3"), 1);
    }

    #[test]
    fn logical_and() {
        assert_eq!(eval("1 && 0"), 0);
        assert_eq!(eval("1 && 1"), 1);
    }

    #[test]
    fn logical_or() {
        assert_eq!(eval("1 || 0"), 1);
        assert_eq!(eval("0 || 0"), 0);
    }

    #[test]
    fn bitwise_and() {
        assert_eq!(eval("0xFF & 0x0F"), 15);
    }

    #[test]
    fn bitwise_or() {
        assert_eq!(eval("0xF0 | 0x0F"), 255);
    }

    #[test]
    fn bitwise_xor() {
        assert_eq!(eval("0xFF ^ 0x0F"), 240);
    }

    #[test]
    fn bitwise_shift() {
        assert_eq!(eval("1 << 8"), 256);
        assert_eq!(eval("256 >> 4"), 16);
    }

    #[test]
    fn ternary() {
        assert_eq!(eval("5 > 3 ? 10 : 20"), 10);
        assert_eq!(eval("5 < 3 ? 10 : 20"), 20);
    }

    #[test]
    fn unary_minus() {
        assert_eq!(eval("-5"), -5);
    }

    #[test]
    fn unary_plus() {
        assert_eq!(eval("+5"), 5);
    }

    #[test]
    fn bitwise_not() {
        assert_eq!(eval("~0"), -1);
    }

    #[test]
    fn logical_not() {
        assert_eq!(eval("! 0"), 1);
        assert_eq!(eval("! 1"), 0);
    }

    #[test]
    fn hex_literal() {
        assert_eq!(eval("0xFF"), 255);
    }

    #[test]
    fn octal_literal() {
        assert_eq!(eval("077"), 63);
    }

    #[test]
    fn variable_read() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("x + 3", &mut state), 8);
    }

    #[test]
    fn variable_with_dollar() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("$x + 3", &mut state), 8);
    }

    #[test]
    fn variable_assignment() {
        let mut state = make_state();
        let result = eval_with("x = 5", &mut state);
        assert_eq!(result, 5);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "5");
    }

    #[test]
    fn compound_assignment() {
        let mut state = make_state();
        set_variable(&mut state, "x", "10".into()).unwrap();
        assert_eq!(eval_with("x += 5", &mut state), 15);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "15");
    }

    #[test]
    fn pre_increment() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("++x", &mut state), 6);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "6");
    }

    #[test]
    fn post_increment() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("x++", &mut state), 5);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "6");
    }

    #[test]
    fn pre_decrement() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("--x", &mut state), 4);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "4");
    }

    #[test]
    fn post_decrement() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("x--", &mut state), 5);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "4");
    }

    #[test]
    fn division_by_zero() {
        let mut state = make_state();
        assert!(eval_arithmetic("1 / 0", &mut state).is_err());
    }

    #[test]
    fn modulo_by_zero() {
        let mut state = make_state();
        assert!(eval_arithmetic("1 % 0", &mut state).is_err());
    }

    #[test]
    fn undefined_variable_defaults_to_zero() {
        assert_eq!(eval("undefined_var"), 0);
    }

    #[test]
    fn empty_expression() {
        assert_eq!(eval(""), 0);
    }

    #[test]
    fn nested_parens() {
        assert_eq!(eval("((2 + 3) * (4 - 1))"), 15);
    }

    #[test]
    fn comma_operator() {
        let mut state = make_state();
        let result = eval_with("x = 1, y = 2, x + y", &mut state);
        assert_eq!(result, 3);
    }

    #[test]
    fn complex_expression() {
        assert_eq!(eval("2 + 3 * 4 - 1"), 13);
    }

    #[test]
    fn dollar_brace_variable() {
        let mut state = make_state();
        set_variable(&mut state, "foo", "42".into()).unwrap();
        assert_eq!(eval_with("${foo} + 1", &mut state), 43);
    }
}
