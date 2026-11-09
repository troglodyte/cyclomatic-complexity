//! Function discovery and cyclomatic complexity counting.
//!
//! Counting follows the same rules as ESLint's `complexity` rule, so numbers
//! are directly comparable:
//!
//! ```text
//! complexity = 1
//!   + if
//!   + for / for-in / for-of / while / do-while
//!   + each `case` (but not `default`)
//!   + catch
//!   + each `&&`, `||`, `??` (and `&&=`, `||=`, `??=`)
//!   + each ternary `?:`
//! ```
//!
//! Nested functions are measured independently: a function's own score never
//! includes the bodies of functions declared inside it. Parameter defaults do
//! count toward the enclosing function, matching ESLint.

use crate::lexer::{lex, Kind, Token};

const ANON: &str = "<anonymous>";

/// Keywords that can never be a method name.
const CONTROL: &[&str] = &[
    "if",
    "while",
    "for",
    "switch",
    "catch",
    "do",
    "with",
    "return",
    "function",
    "else",
    "try",
    "finally",
    "case",
    "default",
    "new",
    "typeof",
    "delete",
    "void",
    "instanceof",
    "in",
    "of",
    "await",
    "yield",
    "throw",
    "import",
    "export",
    "class",
    "extends",
    "var",
    "let",
    "const",
    "enum",
    "interface",
    "type",
    "declare",
    "namespace",
    "module",
];

/// Words that may precede a function/method header without being its name.
const MODIFIERS: &[&str] = &[
    "async",
    "get",
    "set",
    "static",
    "public",
    "private",
    "protected",
    "readonly",
    "override",
    "abstract",
    "declare",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnKind {
    Function,
    Arrow,
    Method,
    Constructor,
    Getter,
    Setter,
}

impl FnKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FnKind::Function => "function",
            FnKind::Arrow => "arrow",
            FnKind::Method => "method",
            FnKind::Constructor => "constructor",
            FnKind::Getter => "getter",
            FnKind::Setter => "setter",
        }
    }
}

/// One decision point that contributed to a score.
#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub line: u32,
    pub branch: &'static str,
}

#[derive(Debug, Clone)]
pub struct Func {
    pub name: String,
    pub kind: FnKind,
    pub line: u32,
    pub complexity: u32,
    /// Nesting level: 0 for a top-level function, 1 for one declared inside it.
    pub depth: u32,
    pub points: Vec<Point>,
    header: usize,
    #[allow(dead_code)]
    body_start: usize,
    body_end: usize,
    children: Vec<usize>,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Count `&&`, `||`, `??` and their assignment forms. Default true.
    pub logical: bool,
    /// Count `?.` as a branch. Default false (ESLint does not count it).
    pub optional_chain: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            logical: true,
            optional_chain: false,
        }
    }
}

/// Analyze one source file. `jsx` should be false for `.ts`/`.mts`/`.cts`.
pub fn analyze(src: &str, jsx: bool, opts: Options) -> Vec<Func> {
    Analyzer::new(lex(src, jsx), opts).run()
}

struct Analyzer<'a> {
    t: Vec<Token<'a>>,
    mask: Vec<bool>,
    pairs: Vec<Option<usize>>,
    /// Per token, the nearest opener before it that no closer between them
    /// closes, counting `(`, `[` and `{` alike (see [`Analyzer::enclosing_openers`]).
    enclosing: Vec<Option<usize>>,
    funcs: Vec<Func>,
    opts: Options,
}

fn opener_for(close: &str) -> &'static str {
    match close {
        ")" => "(",
        "]" => "[",
        _ => "{",
    }
}

fn strip_quotes(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'"' | b'\'' | b'`') && b[b.len() - 1] == b[0] {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

impl<'a> Analyzer<'a> {
    fn new(t: Vec<Token<'a>>, opts: Options) -> Self {
        let n = t.len();
        let mut a = Analyzer {
            t,
            mask: vec![false; n],
            pairs: vec![None; n],
            enclosing: Vec::new(),
            funcs: Vec::new(),
            opts,
        };
        a.match_brackets();
        a.enclosing = a.enclosing_openers();
        a
    }

    // ---------------------------------------------------------------- helpers

    fn match_brackets(&mut self) {
        let mut stack: Vec<usize> = Vec::new();
        for i in 0..self.t.len() {
            let tk = self.t[i];
            if tk.kind != Kind::Punct {
                continue;
            }
            match tk.text {
                "(" | "[" | "{" => stack.push(i),
                ")" | "]" | "}" => {
                    while let Some(j) = stack.pop() {
                        if self.t[j].text == opener_for(tk.text) {
                            self.pairs[j] = Some(i);
                            self.pairs[i] = Some(j);
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// For each token, the opener a backward scan from it stops at: the
    /// nearest `(`, `[` or `{` before it with as many closers as openers
    /// between, of any bracket type. One forward pass with a stack does it: an
    /// opener is pushed, any closer pops one (a closer with none open is
    /// ignored), and each token's answer is the top of the stack. This is what
    /// [`Analyzer::name_before`] needs, without a scan per callback.
    fn enclosing_openers(&self) -> Vec<Option<usize>> {
        let mut stack: Vec<usize> = Vec::new();
        let mut out = Vec::with_capacity(self.t.len());
        for (i, tk) in self.t.iter().enumerate() {
            out.push(stack.last().copied());
            if tk.kind != Kind::Punct {
                continue;
            }
            match tk.text {
                "(" | "[" | "{" => stack.push(i),
                ")" | "]" | "}" => {
                    // A closer with nothing open is ignored: the backward scan
                    // never stops at an opener before it either.
                    stack.pop();
                }
                _ => {} // not a bracket
            }
        }
        out
    }

    #[inline]
    fn at(&self, i: isize) -> Option<Token<'a>> {
        if i < 0 || i as usize >= self.t.len() {
            None
        } else {
            Some(self.t[i as usize])
        }
    }

    /// Is token `i` the punctuator `s`?
    fn p(&self, i: isize, s: &str) -> bool {
        matches!(self.at(i), Some(t) if t.kind == Kind::Punct && t.text == s)
    }

    /// Is token `i` the identifier/keyword `s`?
    fn w(&self, i: isize, s: &str) -> bool {
        matches!(self.at(i), Some(t) if t.kind == Kind::Ident && t.text == s)
    }

    fn masked(&self, i: isize) -> bool {
        i >= 0 && (i as usize) < self.mask.len() && self.mask[i as usize]
    }

    fn pair(&self, i: isize) -> Option<isize> {
        if i < 0 || i as usize >= self.pairs.len() {
            None
        } else {
            self.pairs[i as usize].map(|x| x as isize)
        }
    }

    fn mark(&mut self, a: isize, b: isize) {
        let lo = a.max(0) as usize;
        let hi = ((b + 1).max(0) as usize).min(self.mask.len());
        for k in lo..hi {
            self.mask[k] = true;
        }
    }

    fn is_open(&self, i: isize) -> bool {
        matches!(self.at(i), Some(t) if t.kind == Kind::Punct && matches!(t.text, "(" | "[" | "{"))
    }

    fn is_close(&self, i: isize) -> bool {
        matches!(self.at(i), Some(t) if t.kind == Kind::Punct && matches!(t.text, ")" | "]" | "}"))
    }

    /// `t[i]` is `<`; return the index just past the matching `>`.
    fn skip_generics(&self, i: usize) -> Option<usize> {
        if !self.p(i as isize, "<") {
            return None;
        }
        let mut depth: i32 = 0;
        let limit = (i + 400).min(self.t.len());
        let mut j = i;
        while j < limit {
            let tk = self.t[j];
            if tk.kind == Kind::Punct {
                match tk.text {
                    "<" => depth += 1,
                    ">" => depth -= 1,
                    ">>" => depth -= 2,
                    ">>>" => depth -= 3,
                    ";" | "=>" => return None,
                    _ => {}
                }
                if depth <= 0 {
                    return Some(j + 1);
                }
            }
            j += 1;
        }
        None
    }

    /// `t[i]` is a `>` closing a generic list; return the index of its `<`.
    fn back_generics(&self, i: usize) -> Option<usize> {
        let mut depth: i32 = 0;
        let mut j = i as isize;
        let limit = (i as isize) - 400;
        while j > limit && j >= 0 {
            let tk = self.t[j as usize];
            if tk.kind == Kind::Punct {
                match tk.text {
                    ">" => depth += 1,
                    ">>" => depth += 2,
                    "<" => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j as usize);
                        }
                    }
                    ";" | "{" | "}" => return None,
                    _ => {}
                }
            }
            j -= 1;
        }
        None
    }

    /// Ranges of `: Type` annotations at the top level of a parameter list.
    /// A `:` that follows a top-level `=` is a ternary in a default value, not
    /// a type, so it is left alone.
    fn param_type_ranges(&self, open_i: usize, close_i: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut depth: i32 = 0;
        let mut seen_eq = false;
        let mut i = open_i + 1;
        while i < close_i {
            let tk = self.t[i];
            if tk.kind == Kind::Punct {
                match tk.text {
                    "(" | "[" | "{" => depth += 1,
                    ")" | "]" | "}" => depth -= 1,
                    "," if depth == 0 => seen_eq = false,
                    "=" if depth == 0 => seen_eq = true,
                    ":" if depth == 0 && !seen_eq => {
                        let mut j = i;
                        let mut d2: i32 = 0;
                        while j < close_i {
                            let u = self.t[j];
                            if u.kind == Kind::Punct {
                                match u.text {
                                    "(" | "[" | "{" => d2 += 1,
                                    ")" | "]" | "}" => d2 -= 1,
                                    "," | "=" if d2 == 0 => break,
                                    _ => {}
                                }
                            }
                            j += 1;
                        }
                        out.push((i, j - 1));
                        i = j;
                        continue;
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        out
    }

    /// Skip a `: ReturnType` annotation after the `)` at `close_i`.
    /// Returns the index of the first token after it, plus the range to mask.
    fn return_type_end(&self, close_i: usize) -> (usize, Option<(usize, usize)>) {
        let start = close_i + 1;
        if !self.p(start as isize, ":") {
            return (start, None);
        }
        let mut i = start;
        let mut d: i32 = 0;
        while i < self.t.len() {
            let tk = self.t[i];
            if tk.kind == Kind::Punct {
                match tk.text {
                    "{" => {
                        if d == 0 {
                            // An object type literal only if another `{` or `=>`
                            // follows its closing brace; otherwise this is the body.
                            if let Some(end) = self.pair(i as isize) {
                                if self.p(end + 1, "{") || self.p(end + 1, "=>") {
                                    i = end as usize + 1;
                                    continue;
                                }
                            }
                            break;
                        }
                        d += 1;
                    }
                    "(" | "[" => d += 1,
                    ")" | "]" => {
                        if d == 0 {
                            break;
                        }
                        d -= 1;
                    }
                    ";" | "=" | "," if d == 0 => break,
                    _ => {}
                }
            }
            i += 1;
        }
        (i, Some((start, i - 1)))
    }

    /// End of a standalone type expression starting at `i` (for `as` / `satisfies`).
    fn skip_type_expr(&self, mut i: usize) -> usize {
        let mut d: i32 = 0;
        while i < self.t.len() {
            let tk = self.t[i];
            match tk.kind {
                Kind::Punct => match tk.text {
                    "(" | "[" | "{" => d += 1,
                    ")" | "]" | "}" => {
                        if d == 0 {
                            return i;
                        }
                        d -= 1;
                    }
                    ";" | "," | "?" | ":" if d == 0 => return i,
                    _ => {}
                },
                Kind::Ident if d == 0 => {
                    if matches!(
                        tk.text,
                        "return" | "if" | "else" | "const" | "let" | "var" | "function" | "class"
                    ) {
                        return i;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        self.t.len()
    }

    // ----------------------------------------------------------------- naming

    fn name_before(&self, start: usize) -> String {
        let mut i = start as isize - 1;
        while let Some(t) = self.at(i) {
            if t.kind == Kind::Ident && MODIFIERS.contains(&t.text) {
                i -= 1;
            } else {
                break;
            }
        }

        if self.p(i, "=") {
            let mut j = i - 1;
            if self.masked(j) {
                while self.masked(j) {
                    j -= 1;
                }
                if self.p(j, ":") {
                    j -= 1;
                }
            }
            if let Some(t) = self.at(j) {
                if t.kind == Kind::Ident {
                    return t.text.to_string();
                }
            }
            if self.p(j, "]") {
                if let Some(o) = self.pair(j) {
                    if let Some(t) = self.at(o - 1) {
                        if t.kind == Kind::Ident {
                            return t.text.to_string();
                        }
                    }
                }
            }
            if self.p(j, ")") {
                return ANON.to_string();
            }
        }

        if self.p(i, ":") {
            if let Some(t) = self.at(i - 1) {
                if t.kind == Kind::Ident || t.kind == Kind::Str {
                    return strip_quotes(t.text).to_string();
                }
            }
        }

        // A callback passed to a call: name it after the callee.
        if self.p(i, "(") || self.p(i, ",") {
            let mut o = i;
            if self.p(i, ",") {
                // The opener of the list this comma separates, or -1.
                o = self.enclosing[i as usize].map_or(-1, |x| x as isize);
            }
            if o > 0 && self.p(o, "(") {
                let c = o - 1;
                if let Some(t) = self.at(c) {
                    if t.kind == Kind::Ident {
                        return format!("{}(\u{2026})", t.text);
                    }
                }
                if self.p(c, ")") {
                    return ANON.to_string();
                }
            }
        }

        if self.w(i, "return") {
            return "<returned>".to_string();
        }
        ANON.to_string()
    }

    fn kind_before(&self, start: usize, base: FnKind) -> FnKind {
        let mut i = start as isize - 1;
        loop {
            match self.at(i) {
                Some(t) if t.kind == Kind::Ident && MODIFIERS.contains(&t.text) => {
                    if t.text == "get" {
                        return FnKind::Getter;
                    }
                    if t.text == "set" {
                        return FnKind::Setter;
                    }
                    i -= 1;
                }
                Some(t) if t.kind == Kind::Punct && t.text == "*" => i -= 1,
                _ => return base,
            }
        }
    }

    // -------------------------------------------------------------- detection

    fn concise_body_end(&self, start: usize) -> usize {
        let mut i = start;
        let mut d: i32 = 0;
        while i < self.t.len() {
            let tk = self.t[i];
            if tk.kind == Kind::Punct {
                match tk.text {
                    "(" | "[" | "{" => d += 1,
                    ")" | "]" | "}" => {
                        if d == 0 {
                            return i - 1;
                        }
                        d -= 1;
                    }
                    "," | ";" | "=" if d == 0 && !tk.in_jsx => return i - 1,
                    _ => {}
                }
            }
            i += 1;
        }
        self.t.len() - 1
    }

    fn add(&mut self, name: String, kind: FnKind, header: usize, bs: usize, be: usize) {
        let line = self.t[header].line;
        self.funcs.push(Func {
            name,
            kind,
            line,
            complexity: 1,
            depth: 0,
            points: Vec::new(),
            header,
            body_start: bs,
            body_end: be,
            children: Vec::new(),
        });
    }

    fn run(mut self) -> Vec<Func> {
        let n = self.t.len();
        let mut i = 0;
        while i < n {
            if self.mask[i] {
                i += 1;
                continue;
            }
            let tk = self.t[i];

            if tk.kind == Kind::Ident {
                match tk.text {
                    "type" | "interface" | "declare" | "namespace" | "enum" | "module" => {
                        if let Some(j) = self.skip_type_decl(i) {
                            self.mark(i as isize, j as isize - 1);
                            i = j;
                            continue;
                        }
                    }
                    "as" | "satisfies" if i > 0 => {
                        let j = self.skip_type_expr(i + 1);
                        self.mark(i as isize, j as isize - 1);
                        i = j;
                        continue;
                    }
                    "function" => {
                        if let Some(j) = self.try_function(i) {
                            i = j;
                            continue;
                        }
                    }
                    "const" | "let" | "var" => {
                        if let Some(next) = self.at(i as isize + 1) {
                            if next.kind == Kind::Ident || matches!(next.text, "{" | "[") {
                                self.mask_variable_types(i);
                            }
                        }
                    }
                    _ => {}
                }
            }

            if tk.kind == Kind::Punct && tk.text == "=>" {
                self.try_arrow(i);
                i += 1;
                continue;
            }

            if (matches!(tk.kind, Kind::Ident | Kind::Str)
                || (tk.kind == Kind::Punct && tk.text == "["))
                && self.try_method(i)
            {
                i += 1;
                continue;
            }
            i += 1;
        }

        self.assign_depths();
        let skips = self.do_while_skips();
        for fi in 0..self.funcs.len() {
            let (cc, pts) = self.count(fi, &skips);
            self.funcs[fi].complexity = cc;
            self.funcs[fi].points = pts;
        }
        self.funcs
    }

    /// Skip over a TS type-only declaration; returns the index just past it.
    fn skip_type_decl(&self, i: usize) -> Option<usize> {
        let word = self.t[i].text;
        if matches!(word, "interface" | "namespace" | "enum" | "module") {
            let mut j = i + 1;
            while j < self.t.len() && !self.p(j as isize, "{") {
                if self.p(j as isize, ";") {
                    return Some(j + 1);
                }
                j += 1;
            }
            return self.pair(j as isize).map(|e| e as usize + 1);
        }
        if word == "declare" {
            return if self.w(i as isize + 1, "module") {
                None
            } else {
                Some(i + 1)
            };
        }
        // `type X = ...`
        if !matches!(self.at(i as isize + 1), Some(t) if t.kind == Kind::Ident) {
            return None;
        }
        let mut j = i + 2;
        if self.p(j as isize, "<") {
            j = self.skip_generics(j)?;
        }
        if !self.p(j as isize, "=") {
            return None;
        }
        let mut d: i32 = 0;
        while j < self.t.len() {
            let u = self.t[j];
            if u.kind == Kind::Punct {
                match u.text {
                    "(" | "[" | "{" => d += 1,
                    ")" | "]" | "}" => d -= 1,
                    ";" if d == 0 => return Some(j + 1),
                    _ => {}
                }
            }
            if d == 0 {
                if let Some(next) = self.at(j as isize + 1) {
                    if next.line > u.line
                        && next.kind == Kind::Ident
                        && matches!(
                            next.text,
                            "const"
                                | "let"
                                | "var"
                                | "function"
                                | "class"
                                | "export"
                                | "import"
                                | "type"
                                | "interface"
                                | "enum"
                                | "declare"
                                | "async"
                        )
                    {
                        return Some(j + 1);
                    }
                }
            }
            j += 1;
        }
        Some(self.t.len())
    }

    fn mask_variable_types(&mut self, i: usize) {
        let n = self.t.len();
        let mut j = i + 1;
        let mut depth: i32 = 0;
        while j < n {
            let tk = self.t[j];
            if tk.kind == Kind::Punct {
                match tk.text {
                    "(" | "[" | "{" => depth += 1,
                    ")" | "]" | "}" => {
                        depth -= 1;
                        if depth < 0 {
                            return;
                        }
                    }
                    ";" if depth == 0 => return,
                    "=" if depth == 0 => return,
                    ":" if depth == 0 => {
                        let mut k = j + 1;
                        let mut d2: i32 = 0;
                        while k < n {
                            let u = self.t[k];
                            if u.kind == Kind::Punct {
                                match u.text {
                                    "(" | "[" | "{" => d2 += 1,
                                    ")" | "]" | "}" => d2 -= 1,
                                    "," | "=" | ";" if d2 == 0 => break,
                                    _ => {}
                                }
                            }
                            k += 1;
                        }
                        self.mark(j as isize, k as isize - 1);
                        j = k - 1;
                    }
                    _ => {}
                }
            }
            j += 1;
        }
    }

    fn try_function(&mut self, i: usize) -> Option<usize> {
        let mut j = i + 1;
        if self.p(j as isize, "*") {
            j += 1;
        }
        let mut name: Option<String> = None;
        if let Some(t) = self.at(j as isize) {
            if t.kind == Kind::Ident && !CONTROL.contains(&t.text) {
                name = Some(t.text.to_string());
                j += 1;
            }
        }
        if self.p(j as isize, "<") {
            let k = self.skip_generics(j)?;
            self.mark(j as isize, k as isize - 1);
            j = k;
        }
        if !self.p(j as isize, "(") {
            return None;
        }
        let close = self.pair(j as isize)? as usize;
        for (a, b) in self.param_type_ranges(j, close) {
            self.mark(a as isize, b as isize);
        }
        let (after, rt) = self.return_type_end(close);
        if let Some((a, b)) = rt {
            self.mark(a as isize, b as isize);
        }
        if !self.p(after as isize, "{") {
            return None; // overload signature or ambient declaration
        }
        let end = self.pair(after as isize)? as usize;
        let name = name.unwrap_or_else(|| self.name_before(i));
        self.add(name, FnKind::Function, i, after, end);
        Some(i + 1)
    }

    fn try_arrow(&mut self, i: usize) {
        let ii = i as isize;
        let mut j = ii - 1;
        if j < 0 {
            return;
        }
        let mut ret_span: Option<(usize, usize)> = None;
        let open_i: Option<usize>;

        if self.p(j, ")") {
            open_i = self.pair(j).map(|x| x as usize);
        } else if matches!(self.at(j), Some(t) if t.kind == Kind::Ident && !CONTROL.contains(&t.text))
        {
            // Either a single unparenthesised parameter, or a return type
            // annotation such as `(x): Foo => ...`.
            let mut k = j;
            while k > 0 {
                let ok = match self.at(k) {
                    Some(t) => match t.kind {
                        Kind::Ident | Kind::Num | Kind::Str => true,
                        Kind::Punct => {
                            matches!(t.text, "." | "[" | "]" | "|" | "&" | "<" | ">" | "?")
                        }
                        _ => false,
                    },
                    None => false,
                };
                if ok {
                    k -= 1;
                } else {
                    break;
                }
            }
            if self.p(k, ":") && self.p(k - 1, ")") {
                ret_span = Some((k as usize, j as usize));
                open_i = self.pair(k - 1).map(|x| x as usize);
                j = k - 1;
            } else {
                open_i = None;
            }
        } else {
            return;
        }

        let mut start = match open_i {
            Some(o) => o,
            None => {
                if !matches!(self.at(j), Some(t) if t.kind == Kind::Ident) {
                    return;
                }
                j as usize
            }
        };

        if let Some(o) = open_i {
            let close = j as usize;
            for (a, b) in self.param_type_ranges(o, close) {
                self.mark(a as isize, b as isize);
            }
            if let Some((a, b)) = ret_span {
                self.mark(a as isize, b as isize);
            }
        }

        // Optional generic parameter list and `async` prefix.
        if start > 0 && self.p(start as isize - 1, ">") {
            if let Some(g) = self.back_generics(start - 1) {
                self.mark(g as isize, start as isize - 1);
                start = g;
            }
        }
        if start > 0 && self.w(start as isize - 1, "async") {
            start -= 1;
        }

        let (bs, be);
        if self.p(ii + 1, "{") {
            match self.pair(ii + 1) {
                Some(e) => {
                    bs = i + 1;
                    be = e as usize;
                }
                None => return,
            }
        } else {
            bs = i + 1;
            if bs >= self.t.len() {
                return;
            }
            be = self.concise_body_end(bs);
            if be < bs {
                return;
            }
            if self.p(be as isize + 1, "=") {
                // `x: (a: T) => R = ...` is a type annotation, not a function.
                self.mark(start as isize, be as isize);
                return;
            }
        }
        let name = self.name_before(start);
        self.add(name, FnKind::Arrow, start, bs, be);
    }

    fn try_method(&mut self, i: usize) -> bool {
        let tk = self.t[i];
        let ii = i as isize;

        if tk.kind == Kind::Ident && CONTROL.contains(&tk.text) {
            return false;
        }
        if self.p(ii - 1, ".") || self.p(ii - 1, "?.") {
            return false;
        }
        // `function *name(` is handled by try_function, not here.
        let mut k = ii - 1;
        while self.p(k, "*")
            || matches!(self.at(k), Some(t) if t.kind == Kind::Ident && MODIFIERS.contains(&t.text))
        {
            k -= 1;
        }
        if self.w(k, "function") {
            return false;
        }
        let prev_ok = i == 0
            || match self.at(ii - 1) {
                Some(t) => match t.kind {
                    Kind::Punct => matches!(t.text, "{" | "}" | ";" | "," | "*" | ")" | "]"),
                    Kind::Ident => MODIFIERS.contains(&t.text),
                    _ => false,
                },
                None => false,
            };
        if !prev_ok {
            return false;
        }

        let mut j = i + 1;
        let mut generics: Option<(usize, usize)> = None;
        if tk.kind == Kind::Punct && tk.text == "[" {
            match self.pair(ii) {
                Some(e) => j = e as usize + 1,
                None => return false,
            }
        }
        if self.p(j as isize, "?") {
            j += 1;
        }
        if self.p(j as isize, "<") {
            match self.skip_generics(j) {
                Some(k2) => {
                    generics = Some((j, k2 - 1));
                    j = k2;
                }
                None => return false,
            }
        }
        if !self.p(j as isize, "(") {
            return false;
        }
        let close = match self.pair(j as isize) {
            Some(c) => c as usize,
            None => return false,
        };
        // Compute marks but only commit them if this really is a method: a call
        // like `f(a ? b : c)` must not have its `: c` masked.
        let ptypes = self.param_type_ranges(j, close);
        let (after, rt) = self.return_type_end(close);
        if !self.p(after as isize, "{") {
            return false;
        }
        let end = match self.pair(after as isize) {
            Some(e) => e as usize,
            None => return false,
        };

        if let Some((a, b)) = generics {
            self.mark(a as isize, b as isize);
        }
        for (a, b) in ptypes {
            self.mark(a as isize, b as isize);
        }
        if let Some((a, b)) = rt {
            self.mark(a as isize, b as isize);
        }

        let name = match tk.kind {
            Kind::Ident | Kind::Str => strip_quotes(tk.text).to_string(),
            _ => {
                // `["name"]() {}` -- use the literal when it is a single token.
                if self.pair(ii) == Some(ii + 2)
                    && matches!(self.at(ii + 1), Some(t) if matches!(t.kind, Kind::Ident | Kind::Str))
                {
                    strip_quotes(self.t[i + 1].text).to_string()
                } else {
                    "[computed]".to_string()
                }
            }
        };
        let base = if name == "constructor" {
            FnKind::Constructor
        } else {
            FnKind::Method
        };
        let kind = self.kind_before(i, base);
        self.add(name, kind, i, after, end);
        true
    }

    // ------------------------------------------------------ nesting + scoring

    fn assign_depths(&mut self) {
        self.funcs
            .sort_by(|a, b| a.header.cmp(&b.header).then(b.body_end.cmp(&a.body_end)));
        let mut stack: Vec<usize> = Vec::new();
        for idx in 0..self.funcs.len() {
            let h = self.funcs[idx].header;
            while let Some(&top) = stack.last() {
                let t = &self.funcs[top];
                if t.header <= h && h <= t.body_end {
                    break;
                }
                stack.pop();
            }
            self.funcs[idx].depth = stack.len() as u32;
            if let Some(&top) = stack.last() {
                self.funcs[top].children.push(idx);
            }
            stack.push(idx);
        }
    }

    /// Indices of `while` tokens that belong to a `do ... while`, so the loop is
    /// not counted twice.
    fn do_while_skips(&self) -> Vec<bool> {
        let mut skips = vec![false; self.t.len()];
        for i in 0..self.t.len() {
            if !(self.t[i].kind == Kind::Ident && self.t[i].text == "do") {
                continue;
            }
            if self.p(i as isize + 1, "{") {
                if let Some(e) = self.pair(i as isize + 1) {
                    if self.w(e + 1, "while") {
                        skips[(e + 1) as usize] = true;
                    }
                }
            } else {
                let mut j = i + 1;
                let mut d: i32 = 0;
                while j < self.t.len() {
                    if self.is_open(j as isize) {
                        d += 1;
                    } else if self.is_close(j as isize) {
                        d -= 1;
                    } else if d == 0 && self.w(j as isize, "while") {
                        skips[j] = true;
                        break;
                    }
                    j += 1;
                }
            }
        }
        skips
    }

    fn branch_label(&self, i: usize, skips: &[bool]) -> Option<&'static str> {
        let tk = self.t[i];
        match tk.kind {
            Kind::Ident => match tk.text {
                "if" => Some("if"),
                "for" => Some("for"),
                "case" => Some("case"),
                "catch" => Some("catch"),
                "do" => Some("do"),
                "while" => {
                    if skips[i] {
                        None
                    } else {
                        Some("while")
                    }
                }
                _ => None,
            },
            Kind::Punct => {
                if self.opts.logical {
                    match tk.text {
                        "&&" => return Some("&&"),
                        "||" => return Some("||"),
                        "??" => return Some("??"),
                        "&&=" => return Some("&&="),
                        "||=" => return Some("||="),
                        "??=" => return Some("??="),
                        _ => {}
                    }
                }
                if tk.text == "?" {
                    // `x?: T`, `x?`, `[a?]`, etc. are not ternaries. `a?.b` and
                    // `a ?? b` are separate tokens.
                    let next_is_marker = match self.at(i as isize + 1) {
                        Some(t) => {
                            t.kind == Kind::Punct
                                && matches!(t.text, ":" | "," | ")" | "]" | "}" | ">")
                        }
                        None => true,
                    };
                    if !next_is_marker {
                        return Some("?:");
                    }
                }
                if self.opts.optional_chain && tk.text == "?." {
                    return Some("?.");
                }
                None
            }
            _ => None,
        }
    }

    fn count(&self, fi: usize, skips: &[bool]) -> (u32, Vec<Point>) {
        let f = &self.funcs[fi];
        let mut holes: Vec<(usize, usize)> = f
            .children
            .iter()
            .map(|&c| (self.funcs[c].header, self.funcs[c].body_end))
            .collect();
        holes.sort_unstable();

        let (from, to) = (f.header, f.body_end);
        let mut cc: u32 = 1;
        let mut points = Vec::new();
        let mut h = 0usize;
        let mut i = from;
        while i <= to && i < self.t.len() {
            while h < holes.len() && holes[h].1 < i {
                h += 1;
            }
            if h < holes.len() && holes[h].0 <= i && i <= holes[h].1 {
                i = holes[h].1 + 1;
                continue;
            }
            if self.mask[i] {
                i += 1;
                continue;
            }
            if let Some(label) = self.branch_label(i, skips) {
                cc += 1;
                points.push(Point {
                    line: self.t[i].line,
                    branch: label,
                });
            }
            i += 1;
        }
        (cc, points)
    }
}

// ---------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(src: &str) -> Vec<(String, u32)> {
        analyze(src, true, Options::default())
            .into_iter()
            .map(|f| (f.name, f.complexity))
            .collect()
    }

    fn cc_ts(src: &str) -> Vec<(String, u32)> {
        analyze(src, false, Options::default())
            .into_iter()
            .map(|f| (f.name, f.complexity))
            .collect()
    }

    /// Examples taken from the ESLint `complexity` rule documentation.
    #[test]
    fn ternary_variants() {
        assert_eq!(cc("function f(a, b, c) { return a ? (b) : c; }")[0].1, 2);
        assert_eq!(cc("function f(a, b, c) { return a ? [b] : c; }")[0].1, 2);
        assert_eq!(cc("function f(a, b, c) { return a ? {b} : c; }")[0].1, 2);
        assert_eq!(cc("function f(x) { return x ? x : (x ? 1 : 2); }")[0].1, 3);
    }

    #[test]
    fn typescript_variable_types_are_masked() {
        assert_eq!(
            cc_ts("function ts() { const x: number extends string ? true : false = false; }")[0].1,
            1
        );
        assert_eq!(
            cc_ts("function ts() { let x: T, y: U extends V ? 1 : 2; }")[0].1,
            1
        );
        assert_eq!(
            cc_ts("function ts() { var x: { a: number } = { a: 1 }; }")[0].1,
            1
        );
    }

    #[test]
    fn matches_eslint_reference() {
        assert_eq!(
            cc("function a(x) { if (true) { return x; } else if (false) { return 1; } else { return 4; } }"),
            vec![("a".to_string(), 3)]
        );
        assert_eq!(
            cc("function b() { foo ||= 1; bar &&= 2; }"),
            vec![("b".to_string(), 3)]
        );
        assert_eq!(
            cc("function c(x) { switch (x) { case 1: return 1; case 2: return 2; default: return 3; } }"),
            vec![("c".to_string(), 3)]
        );
        assert_eq!(cc("function d() { return 1; }"), vec![("d".to_string(), 1)]);
        assert_eq!(
            cc("function e(a, b) { return a && b ? 1 : 2; }"),
            vec![("e".to_string(), 3)]
        );
    }

    #[test]
    fn loops_and_handlers() {
        assert_eq!(
            cc("function f(x){ for (const i of x) { while (i) {} } }")[0].1,
            3
        );
        assert_eq!(cc("function f(x){ do { x--; } while (x); }")[0].1, 2);
        assert_eq!(cc("function f(){ try { go(); } catch (e) {} }")[0].1, 2);
        assert_eq!(
            cc("function f(){ try { go(); } catch { return 0; } }")[0].1,
            2
        );
    }

    #[test]
    fn optional_chaining_is_not_a_branch_by_default() {
        assert_eq!(cc("function f(x){ return x?.y?.z; }")[0].1, 1);
        let with = analyze(
            "function f(x){ return x?.y?.z; }",
            true,
            Options {
                logical: true,
                optional_chain: true,
            },
        );
        assert_eq!(with[0].complexity, 3);
    }

    #[test]
    fn nested_functions_are_scored_independently() {
        let out = cc("function outer(a) {\n\
               if (a) {}\n\
               function inner(b) { if (b) {} return b && 1; }\n\
               const arrow = (c) => { if (c) {} return c || 2; };\n\
               return inner(arrow(a));\n\
             }");
        assert!(out.contains(&("outer".to_string(), 2)));
        assert!(out.contains(&("inner".to_string(), 3)));
        assert!(out.contains(&("arrow".to_string(), 3)));
    }

    #[test]
    fn parameter_defaults_count() {
        let out = cc("function defaults(cb = (x) => x && 1, n = 3 || 4) { return cb(n); }");
        assert!(out.contains(&("defaults".to_string(), 2)));
        assert!(out.contains(&("cb".to_string(), 2)));
    }

    #[test]
    fn strings_comments_regex_and_templates_are_not_scanned() {
        assert_eq!(
            cc(r#"function f(){ const s = 'a // b && c'; return s; }"#)[0].1,
            1
        );
        assert_eq!(cc("function f(){ /* while (x) || y */ return 1; }")[0].1, 1);
        assert_eq!(
            cc(r"function f(x){ const re = /a\/b[/]c && d/g; return re.test(x) || x; }")[0].1,
            2
        );
        assert_eq!(cc("function f(a,b){ return a / b / 2; }")[0].1, 1);
        assert_eq!(
            cc("function f(a,b,c,d){ return `x ${a && b ? 1 : 2} ${`in ${c || d}`}`; }")[0].1,
            4
        );
    }

    #[test]
    fn classes_and_object_literals() {
        let out = cc("class Foo {\n\
               constructor(a) { if (a) this.a = a; }\n\
               get value() { return this.x ?? 0; }\n\
               set value(v) { this.x = v; }\n\
               async load(u) { const r = await fetch(u); if (!r.ok) throw new Error(); }\n\
               *gen() { for (const x of []) yield x; }\n\
               #hidden(a) { return a || 1; }\n\
             }");
        assert!(out.contains(&("constructor".to_string(), 2)));
        assert!(out.contains(&("load".to_string(), 2)));
        assert!(out.contains(&("gen".to_string(), 2)));
        assert!(out.contains(&("#hidden".to_string(), 2)));
        assert_eq!(out.iter().filter(|(n, _)| n == "value").count(), 2);

        let obj = cc(
            "const o = { a() { return 1 || 2; }, c: function () { if (1) {} }, d: (x) => x && 1 };",
        );
        assert!(obj.contains(&("a".to_string(), 2)));
        assert!(obj.contains(&("c".to_string(), 2)));
        assert!(obj.contains(&("d".to_string(), 2)));
    }

    #[test]
    fn generator_declaration_is_not_also_a_method() {
        let out = cc("async function* agen(xs) { for await (const x of xs) { if (x) yield x; } }");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], ("agen".to_string(), 3));
    }

    #[test]
    fn typescript_type_positions_are_ignored() {
        let out = cc_ts(
            "type Handler = (e: Event) => void;\n\
             type Cond<T> = T extends string ? A : B;\n\
             interface Foo { onClick(e: Event): void; bar: (x: number) => void; }\n\
             function typed(cb: (x: number) => boolean): string { return cb(1) ? \"y\" : \"n\"; }\n\
             declare function amb(x: number): void;\n\
             function over(x: string): void;\n\
             function over(x: any): void { if (x) {} }\n\
             const f2: (a: number) => number = (a) => (a > 0 ? a : -a);\n\
             function objret(x: number): { a: number } { if (x) {} return { a: 1 }; }\n",
        );
        assert!(out.contains(&("typed".to_string(), 2)));
        assert!(out.contains(&("over".to_string(), 2)));
        assert!(out.contains(&("f2".to_string(), 2)));
        assert!(out.contains(&("objret".to_string(), 2)));
        // exactly one entry per real function -- no phantom type arrows
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn ternary_in_a_default_value_is_not_a_type_annotation() {
        let out = cc_ts("function tern(a = b ? 1 : c && d, e = 2) { return a || e; }");
        assert_eq!(out[0].1, 4);
    }

    #[test]
    fn jsx_text_and_attributes() {
        let src = "export default function App({ items }: Props) {\n\
                     const visible = items.filter((it) => it.name && !it.hidden);\n\
                     return (\n\
                       <div onClick={() => setQ(\"\")}>\n\
                         {visible.length > 0 ? <List items={visible} /> : <Empty />}\n\
                         {q && <p>hi {q} &amp; more</p>}\n\
                         <span>a &lt; b && c || d</span>\n\
                         <>{items.map((i) => <Row on={i.ok && i.live} />)}</>\n\
                       </div>\n\
                     );\n\
                   }\n\
                   function cmp(a, b) { return a < b && b > a; }";
        let out = cc(src);
        // `&&` and `||` inside JSX *text* must not count
        assert!(out.contains(&("App".to_string(), 3)));
        assert!(out.contains(&("cmp".to_string(), 2)));
        assert!(out.contains(&("filter(\u{2026})".to_string(), 2)));
        assert!(out.contains(&("map(\u{2026})".to_string(), 2)));
    }

    #[test]
    fn comparison_is_not_mistaken_for_jsx() {
        assert_eq!(cc("function f(a, b) { return a < b && b > a; }")[0].1, 2);
        assert_eq!(
            cc("function f(a, b) { return f(a < b) || f(b > a); }")[0].1,
            2
        );
    }

    #[test]
    fn no_functions_means_no_output() {
        assert!(cc("const x = 1 && 2;").is_empty());
        assert!(cc("").is_empty());
    }

    #[test]
    fn malformed_input_does_not_panic() {
        for src in [
            "function f( {",
            "class {",
            "`unterminated ${",
            "/*",
            "'",
            "=> => =>",
            "function f() { if (a) { return; }",
            "<div>",
            "${",
            "?.",
            "function",
        ] {
            let _ = analyze(src, true, Options::default());
        }
    }

    /// Callbacks after a comma, in calls, arrays, objects and nested calls,
    /// with stray and mismatched brackets, named as the original back-scan
    /// named them.
    #[test]
    fn callbacks_after_commas_keep_their_names() {
        let src = "f(a, () => 0, x => 1);\n\
                   [g(1, () => 2), () => 3];\n\
                   h(i(j, () => 4), () => 5, [6, () => 7]);\n\
                   ({ k: 1, m: () => 8 }, () => 9);\n\
                   n(o)(p, () => 10);\n\
                   q(r], () => 11);\n\
                   s(t)), () => 12);\n\
                   u((v, () => 13);\n\
                   ), () => 14;\n";
        let names: Vec<String> = analyze(src, true, Options::default())
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(
            names,
            [
                "f(\u{2026})",
                "f(\u{2026})",
                "g(\u{2026})",
                ANON,
                "i(\u{2026})",
                "h(\u{2026})",
                ANON,
                "m",
                ANON,
                ANON,
                ANON,
                ANON,
                ANON,
                ANON
            ]
        );
    }

    /// Entries in [`a_long_array_of_callbacks_is_analyzed_in_linear_time`].
    const LONG_LIST_ENTRIES: usize = 100_000;

    #[test]
    fn a_long_array_of_callbacks_is_analyzed_in_linear_time() {
        let src = format!("[{}]", "()=>0,".repeat(LONG_LIST_ENTRIES));
        // A debug build is about ten times slower than release.
        let budget = std::time::Duration::from_secs(if cfg!(debug_assertions) { 20 } else { 2 });
        let started = std::time::Instant::now();
        let funcs = analyze(&src, true, Options::default());
        let took = started.elapsed();
        assert_eq!(funcs.len(), LONG_LIST_ENTRIES);
        assert!(
            took < budget,
            "{LONG_LIST_ENTRIES} callbacks took {took:?}, over {budget:?}"
        );
    }

    #[test]
    fn unicode_identifiers_and_strings() {
        let out = cc("function unicodeÖ(café) { return café && 1; }");
        assert_eq!(out[0].1, 2);
        let _ = cc(r#"function f(){ const s = "日本語 \é"; return s; }"#);
    }
}
