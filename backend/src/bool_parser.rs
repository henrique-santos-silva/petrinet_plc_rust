//! Boolean expression parser.
//!
//! Port of the Python `BoolParser` (`src/implementation/boolParser.py`).
//!
//! Parses C++-style boolean expressions containing:
//! - `!`  : NOT
//! - `^`  : XOR
//! - `|`  : OR
//! - `&`  : AND
//! - parentheses
//! - `true` / `false`
//! - "extra tokens" (digital inputs starting with `d`, places starting with `p`)
//!
//! The original implementation lowercased everything and relied on Python's
//! `eval`. This port reproduces the same operator precedence as Python:
//! (lowest) `or` < `and` < `not` < `^` (highest), with parentheses overriding.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError(pub String);

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SyntaxError: {}", self.0)
    }
}

impl std::error::Error for SyntaxError {}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    LParen,
    RParen,
    Not,
    Xor,
    Or,
    And,
    Const(bool),
    Var(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Const(bool),
    Var(String),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Xor(Box<Expr>, Box<Expr>),
}

impl Expr {
    fn eval(&self, inputs: &HashMap<String, bool>) -> bool {
        match self {
            Expr::Const(b) => *b,
            // Missing variables default to false, mirroring `bool(undefined)`
            // never happening because Python passes all kwargs.
            Expr::Var(name) => *inputs.get(name).unwrap_or(&false),
            Expr::Not(e) => !e.eval(inputs),
            Expr::And(a, b) => a.eval(inputs) && b.eval(inputs),
            Expr::Or(a, b) => a.eval(inputs) || b.eval(inputs),
            Expr::Xor(a, b) => a.eval(inputs) ^ b.eval(inputs),
        }
    }
}

/// A compiled boolean expression.
#[derive(Debug, Clone)]
pub struct BoolParser {
    ast: Expr,
    inputs_set: HashSet<String>,
    raw: String,
}

impl BoolParser {
    /// Build a parser from a raw C++-style boolean expression.
    ///
    /// `valid_extra_tokens` are the allowed identifier tokens (digital inputs /
    /// places). They are matched case-insensitively (lowercased), exactly like
    /// the Python `set_valid_extra_tokens`.
    pub fn new(raw_expression: &str, valid_extra_tokens: &[String]) -> Result<Self, SyntaxError> {
        let valid: Vec<String> = valid_extra_tokens
            .iter()
            .map(|t| t.to_lowercase())
            .collect();

        let raw: String = raw_expression
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_lowercase();

        let tokens = Self::tokenize(&raw, &valid)?;

        let mut inputs_set = HashSet::new();
        for t in &tokens {
            if let Token::Var(name) = t {
                inputs_set.insert(name.clone());
            }
        }

        // The Python implementation rejects expressions made only of operators
        // (no `true`/`false`/identifier present).
        let has_atom = tokens
            .iter()
            .any(|t| matches!(t, Token::Const(_) | Token::Var(_)));
        if !has_atom {
            return Err(SyntaxError("invalid expression".into()));
        }

        let mut parser = TokenParser {
            tokens: &tokens,
            pos: 0,
        };
        let ast = parser.parse_or()?;
        if parser.pos != tokens.len() {
            return Err(SyntaxError("Invalid expression".into()));
        }

        Ok(BoolParser {
            ast,
            inputs_set,
            raw,
        })
    }

    /// Evaluate the expression against a set of named boolean values.
    /// Keys are matched case-insensitively.
    pub fn evaluate(&self, inputs: &HashMap<String, bool>) -> bool {
        let lowered: HashMap<String, bool> =
            inputs.iter().map(|(k, v)| (k.to_lowercase(), *v)).collect();
        self.ast.eval(&lowered)
    }

    /// Returns a boxed closure equivalent to the Python `generate_function`.
    ///
    /// The returned closure expects the input map to already have lowercased
    /// keys (the AST variable names are lowercased at parse time). Callers in
    /// the hot path lowercase their inputs once per step rather than once per
    /// evaluation, so the closure here does no allocation.
    pub fn generate_function(self) -> BoolFn {
        let ast = self.ast;
        Box::new(move |inputs: &HashMap<String, bool>| ast.eval(inputs))
    }

    /// Set of identifier tokens referenced by the expression.
    pub fn inputs(&self) -> &HashSet<String> {
        &self.inputs_set
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    fn tokenize(raw: &str, valid: &[String]) -> Result<Vec<Token>, SyntaxError> {
        let chars: Vec<char> = raw.chars().collect();
        let mut tokens = Vec::new();
        let mut i = 0usize;
        while i < chars.len() {
            let c = chars[i];
            match c {
                '(' => {
                    tokens.push(Token::LParen);
                    i += 1;
                    continue;
                }
                ')' => {
                    tokens.push(Token::RParen);
                    i += 1;
                    continue;
                }
                '!' => {
                    tokens.push(Token::Not);
                    i += 1;
                    continue;
                }
                '^' => {
                    tokens.push(Token::Xor);
                    i += 1;
                    continue;
                }
                '|' => {
                    tokens.push(Token::Or);
                    i += 1;
                    continue;
                }
                '&' => {
                    tokens.push(Token::And);
                    i += 1;
                    continue;
                }
                _ => {}
            }

            let rest: String = chars[i..].iter().collect();
            if rest.starts_with("true") {
                tokens.push(Token::Const(true));
                i += 4;
                continue;
            }
            if rest.starts_with("false") {
                tokens.push(Token::Const(false));
                i += 5;
                continue;
            }

            // Greedy longest-match against the valid extra tokens.
            let mut best: Option<&String> = None;
            for tok in valid {
                if rest.starts_with(tok.as_str()) && best.is_none_or(|b| tok.len() > b.len()) {
                    best = Some(tok);
                }
            }
            if let Some(tok) = best {
                tokens.push(Token::Var(tok.clone()));
                i += tok.chars().count();
                continue;
            }

            return Err(SyntaxError("Invalid expression".into()));
        }
        Ok(tokens)
    }
}

pub type BoolFn = Box<dyn Fn(&HashMap<String, bool>) -> bool + Send + Sync>;

struct TokenParser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> TokenParser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    // or_expr := and_expr ('or' and_expr)*
    fn parse_or(&mut self) -> Result<Expr, SyntaxError> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(Token::Or)) {
            self.pos += 1;
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    // and_expr := not_expr ('and' not_expr)*
    fn parse_and(&mut self) -> Result<Expr, SyntaxError> {
        let mut left = self.parse_not()?;
        while matches!(self.peek(), Some(Token::And)) {
            self.pos += 1;
            let right = self.parse_not()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    // not_expr := 'not' not_expr | xor_expr
    fn parse_not(&mut self) -> Result<Expr, SyntaxError> {
        if matches!(self.peek(), Some(Token::Not)) {
            self.pos += 1;
            let inner = self.parse_not()?;
            Ok(Expr::Not(Box::new(inner)))
        } else {
            self.parse_xor()
        }
    }

    // xor_expr := atom ('^' atom)*
    fn parse_xor(&mut self) -> Result<Expr, SyntaxError> {
        let mut left = self.parse_atom()?;
        while matches!(self.peek(), Some(Token::Xor)) {
            self.pos += 1;
            let right = self.parse_atom()?;
            left = Expr::Xor(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    // atom := const | var | '(' or_expr ')'
    fn parse_atom(&mut self) -> Result<Expr, SyntaxError> {
        match self.peek() {
            Some(Token::Const(b)) => {
                let b = *b;
                self.pos += 1;
                Ok(Expr::Const(b))
            }
            Some(Token::Var(name)) => {
                let name = name.clone();
                self.pos += 1;
                Ok(Expr::Var(name))
            }
            Some(Token::LParen) => {
                self.pos += 1;
                let inner = self.parse_or()?;
                if !matches!(self.peek(), Some(Token::RParen)) {
                    return Err(SyntaxError("unbalanced parenthesis".into()));
                }
                self.pos += 1;
                Ok(inner)
            }
            _ => Err(SyntaxError("invalid expression".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(expr: &str, valid: &[&str]) -> Result<BoolParser, SyntaxError> {
        let valid: Vec<String> = valid.iter().map(|s| s.to_string()).collect();
        BoolParser::new(expr, &valid)
    }

    fn map(pairs: &[(&str, bool)]) -> HashMap<String, bool> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn test_raises_for_invalid_syntax() {
        let valid = ["di1"];
        for bad in [
            "",
            "|",
            "&",
            "!",
            "^",
            "true true",
            "^true",
            "[]",
            "()",
            "ii",
        ] {
            assert!(parse(bad, &valid).is_err(), "expected error for {bad:?}");
        }
    }

    #[test]
    fn test_returns_the_expected_value() {
        let valid = ["di1"];
        let f = parse("di1", &valid).unwrap();
        assert!(f.evaluate(&map(&[("di1", true)])));
        assert!(!f.evaluate(&map(&[("di1", false)])));

        let inputs = [
            ("di0", true),
            ("di1", true),
            ("di2", false),
            ("di3", true),
            ("di4", false),
        ];
        let valid = ["di0", "di1", "di2", "di3", "di4", "p0"];
        let m = map(&inputs);
        let check = |expr: &str, expected: bool| {
            let p = parse(expr, &valid).unwrap();
            assert_eq!(p.evaluate(&m), expected, "expr={expr}");
        };

        check("!(di0)", false);
        check("!(di2)", true);
        check("di0 ^ di1 ", false);
        check("di2 ^ di4 ", false);
        check("di0 ^ di4 ", true);
        check("di4 ^ di0 ", true);
        check("true ^ true", false);
        check("false ^ false", false);
        check("true ^ false", true);
        check("!true | !false", true);
        check("!true & !false", false);
        check("true & false | false", false);
        check("false | true & false", false);
        check("(!false) ^ (!true)", true);

        let p0 = parse("P0", &valid).unwrap();
        assert!(p0.evaluate(&map(&[("P0", true)])));
    }

    #[test]
    fn test_generate_function() {
        let valid: Vec<String> = vec!["di0".to_string()];
        let f = BoolParser::new("di0", &valid).unwrap().generate_function();
        assert!(f(&map(&[("di0", true)])));
        assert!(!f(&map(&[("di0", false)])));
    }
}
