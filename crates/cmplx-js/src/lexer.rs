//! A small, dependency-free tokenizer for JavaScript, TypeScript and JSX.
//!
//! It is deliberately *not* a parser. It produces a flat token stream that is
//! accurate about the things that trip up naive scanners — comments, strings,
//! template literals with nested substitutions, regex-vs-division, and JSX
//! text — because those are exactly the places where a complexity counter
//! would otherwise miscount.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ident,
    Num,
    Str,
    Template,
    Regex,
    Punct,
    JsxText,
}

#[derive(Debug, Clone, Copy)]
pub struct Token<'a> {
    pub kind: Kind,
    pub text: &'a str,
    /// 1-based line on which the token starts.
    pub line: u32,
    /// True for tokens lexed inside a JSX tag or JSX children region (i.e. not
    /// inside a `{ ... }` expression container).
    pub in_jsx: bool,
}

/// Multi-byte punctuators, longest first so greedy matching is correct.
const PUNCTS: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", "&&=", "||=", "??=", ">>>", "=>", "==", "!=",
    "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
    "**", "<<", ">>",
];

/// Keywords after which a `/` starts a regex literal and a `<` may start JSX.
const VALUE_KEYWORDS: &[&str] = &[
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
    "extends",
    "default",
];

#[inline]
fn is_id_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c == b'#' || c >= 0x80
}

#[inline]
fn is_id_part(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80
}

/// Length in bytes of a non-ASCII whitespace character at `i`, if any.
fn unicode_space_len(b: &[u8], i: usize) -> Option<usize> {
    match b[i] {
        // U+00A0 NO-BREAK SPACE
        0xC2 if b.get(i + 1) == Some(&0xA0) => Some(2),
        // U+FEFF BOM / U+2028 / U+2029 / U+2000..U+200A
        0xEF if b.get(i + 1) == Some(&0xBB) && b.get(i + 2) == Some(&0xBF) => Some(3),
        0xE2 if b.get(i + 1) == Some(&0x80) => match b.get(i + 2).copied() {
            Some(0x80..=0x8A) | Some(0xA8) | Some(0xA9) | Some(0xAF) => Some(3),
            _ => None,
        },
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Normal,
    /// A `${ ... }` or JSX `{ ... }` expression container.
    Expr,
    /// The string parts of a template literal.
    Template,
    /// Inside `< ... >`.
    JsxTag,
    /// Between `>` and the matching `</`.
    JsxChildren,
}

struct Frame {
    kind: FrameKind,
    /// Brace nesting inside an `Expr` frame.
    depth: u32,
    /// Whether the `}` that closes this frame should be emitted as a token.
    emit_close: bool,
    /// This tag is a closing tag (`</name>` or `</>`).
    closing: bool,
    /// The tag name has been seen, so a following `/` means self-closing.
    started: bool,
}

impl Frame {
    fn new(kind: FrameKind) -> Self {
        Frame {
            kind,
            depth: 0,
            emit_close: false,
            closing: false,
            started: false,
        }
    }
    fn expr(emit_close: bool) -> Self {
        Frame {
            kind: FrameKind::Expr,
            depth: 0,
            emit_close,
            closing: false,
            started: false,
        }
    }
}

pub struct Lexer<'a> {
    src: &'a str,
    b: &'a [u8],
    i: usize,
    line: u32,
    jsx: bool,
    frames: Vec<Frame>,
    out: Vec<Token<'a>>,
}

/// Tokenize `src`. Set `jsx` to false for `.ts`/`.mts`/`.cts`, where `<` is
/// always a comparison or type argument and never a JSX element.
pub fn lex(src: &str, jsx: bool) -> Vec<Token<'_>> {
    Lexer::new(src, jsx).run()
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str, jsx: bool) -> Self {
        Lexer {
            src,
            b: src.as_bytes(),
            i: 0,
            line: 1,
            jsx,
            frames: vec![Frame::new(FrameKind::Normal)],
            out: Vec::new(),
        }
    }

    pub fn run(mut self) -> Vec<Token<'a>> {
        while self.i < self.b.len() {
            let before_pos = self.i;
            let before_frames = self.frames.len();
            match self
                .frames
                .last()
                .map(|f| f.kind)
                .unwrap_or(FrameKind::Normal)
            {
                FrameKind::Template => self.lex_template(),
                FrameKind::JsxTag => self.lex_jsx_tag(),
                FrameKind::JsxChildren => self.lex_jsx_children(),
                _ => self.lex_normal(),
            }
            // Defensive: never stall, whatever the input looks like.
            if self.i == before_pos && self.frames.len() == before_frames {
                self.i += 1;
                self.align();
            }
        }
        self.out
    }

    // ---------------------------------------------------------------- helpers

    fn push(&mut self, kind: Kind, text: &'a str, line: Option<u32>) {
        let in_jsx = matches!(
            self.frames.last().map(|f| f.kind),
            Some(FrameKind::JsxTag) | Some(FrameKind::JsxChildren)
        );
        self.out.push(Token {
            kind,
            text,
            line: line.unwrap_or(self.line),
            in_jsx,
        });
    }

    /// Advance past UTF-8 continuation bytes so slices stay on char boundaries.
    fn align(&mut self) {
        while self.i < self.b.len() && (self.b[self.i] & 0xC0) == 0x80 {
            self.i += 1;
        }
    }

    fn count_lines(&mut self, from: usize, to: usize) {
        self.line += self.b[from..to].iter().filter(|&&c| c == b'\n').count() as u32;
    }

    // ------------------------------------------------------------ normal mode

    fn lex_normal(&mut self) {
        let c = self.b[self.i];
        if self.skip_whitespace(c) || self.skip_comment(c) || self.lex_token(c) {
            return;
        }
        if self.close_expr_brace(c) {
            return;
        }
        self.lex_punct();
    }

    /// Whitespace and line terminators produce no token; newlines bump the line.
    fn skip_whitespace(&mut self, c: u8) -> bool {
        if c == b' ' || c == b'\t' || c == b'\r' || c == 0x0b || c == 0x0c {
            self.i += 1;
            return true;
        }
        if c == b'\n' {
            self.line += 1;
            self.i += 1;
            return true;
        }
        if c >= 0x80 {
            if let Some(w) = unicode_space_len(self.b, self.i) {
                self.i += w;
                return true;
            }
        }
        false
    }

    fn skip_comment(&mut self, c: u8) -> bool {
        if c != b'/' || self.i + 1 >= self.b.len() {
            return false;
        }
        match self.b[self.i + 1] {
            b'/' => self.skip_line_comment(),
            b'*' => self.skip_block_comment(),
            _ => return false,
        }
        true
    }

    fn skip_line_comment(&mut self) {
        let b = self.b;
        let mut j = self.i;
        while j < b.len() && b[j] != b'\n' {
            j += 1;
        }
        self.i = j;
    }

    /// An unterminated block comment runs to the end of the input.
    fn skip_block_comment(&mut self) {
        let b = self.b;
        let n = b.len();
        let mut j = self.i + 2;
        while j + 1 < n && !(b[j] == b'*' && b[j + 1] == b'/') {
            j += 1;
        }
        let end = if j + 1 < n { j + 2 } else { n };
        self.count_lines(self.i, end);
        self.i = end;
    }

    /// Regexes, JSX openers, quoted literals, numbers and identifiers. The arms
    /// are disjoint on `c`, so their order does not matter; comments are tried
    /// before this, which is what keeps `//` and `/*` out of the regex arm.
    fn lex_token(&mut self, c: u8) -> bool {
        match c {
            b'/' => self.regex_allowed() && self.lex_regex(),
            b'<' => self.jsx && self.jsx_allowed() && self.try_jsx_open(),
            b'`' | b'\'' | b'"' => {
                self.lex_quoted(c);
                true
            }
            _ => self.lex_word(c),
        }
    }

    /// A template opens a frame and is lexed span by span; a string is one token.
    fn lex_quoted(&mut self, quote: u8) {
        if quote == b'`' {
            self.i += 1;
            self.frames.push(Frame::new(FrameKind::Template));
        } else {
            self.lex_string(quote);
        }
    }

    fn lex_word(&mut self, c: u8) -> bool {
        if self.starts_number(c) {
            self.lex_number();
        } else if is_id_start(c) {
            self.lex_ident();
        } else {
            return false;
        }
        true
    }

    fn starts_number(&self, c: u8) -> bool {
        c.is_ascii_digit()
            || (c == b'.' && self.i + 1 < self.b.len() && self.b[self.i + 1].is_ascii_digit())
    }

    fn lex_ident(&mut self) {
        let n = self.b.len();
        let st = self.i;
        self.i += 1;
        while self.i < n && is_id_part(self.b[self.i]) {
            self.i += 1;
        }
        self.push(Kind::Ident, &self.src[st..self.i], None);
    }

    /// Brace bookkeeping for `${ }` and JSX `{ }` containers. Returns true when
    /// `c` closed the container, which consumes it; otherwise the brace (if any)
    /// is counted and still lexed as punctuation.
    fn close_expr_brace(&mut self, c: u8) -> bool {
        let Some(frame) = self.frames.last_mut().filter(|f| f.kind == FrameKind::Expr) else {
            return false;
        };
        match c {
            b'}' if frame.depth == 0 => {
                let emit = frame.emit_close;
                self.frames.pop();
                self.i += 1;
                if emit {
                    self.push(Kind::Punct, "}", None);
                }
                true
            }
            b'}' => {
                frame.depth -= 1;
                false
            }
            b'{' => {
                frame.depth += 1;
                false
            }
            _ => false,
        }
    }

    /// Longest-match punctuation from `PUNCTS`, else a single byte.
    fn lex_punct(&mut self) {
        let src = self.src;
        for &p in PUNCTS {
            if src[self.i..].starts_with(p) {
                self.i += p.len();
                self.push(Kind::Punct, p, None);
                return;
            }
        }
        let st = self.i;
        self.i += 1;
        self.push(Kind::Punct, &src[st..self.i], None);
    }

    fn lex_string(&mut self, quote: u8) {
        let src = self.src;
        let n = self.b.len();
        let st = self.i;
        let ln = self.line;
        self.i += 1;
        while self.i < n {
            let c = self.b[self.i];
            if c == b'\\' {
                self.i = (self.i + 2).min(n);
                self.align();
                continue;
            }
            if c == quote {
                self.i += 1;
                break;
            }
            if c == b'\n' {
                self.line += 1;
            }
            self.i += 1;
        }
        self.push(Kind::Str, &src[st..self.i], Some(ln));
    }

    fn lex_number(&mut self) {
        let src = self.src;
        let b = self.b;
        let n = b.len();
        let st = self.i;
        let hex = st + 1 < n && b[st] == b'0' && (b[st + 1] | 0x20) == b'x';
        while self.i < n {
            let c = b[self.i];
            if c.is_ascii_alphanumeric()
                || c == b'.'
                || c == b'_'
                || ((c == b'+' || c == b'-')
                    && !hex
                    && self.i > st
                    && (b[self.i - 1] | 0x20) == b'e')
            {
                self.i += 1;
            } else {
                break;
            }
        }
        self.push(Kind::Num, &src[st..self.i], None);
    }

    fn regex_allowed(&self) -> bool {
        match self.out.last() {
            None => true,
            Some(p) => match p.kind {
                Kind::Punct => !matches!(p.text, ")" | "]" | "}" | "++" | "--"),
                Kind::Ident => VALUE_KEYWORDS.contains(&p.text),
                _ => false,
            },
        }
    }

    /// Try to consume a regex literal. Returns false (consuming nothing) if the
    /// `/` turns out not to start one.
    fn lex_regex(&mut self) -> bool {
        let src = self.src;
        let b = self.b;
        let n = b.len();
        let st = self.i;
        let mut i = self.i + 1;
        let mut in_class = false;
        while i < n {
            let c = b[i];
            if c == b'\\' {
                i += 2;
                while i < n && (b[i] & 0xC0) == 0x80 {
                    i += 1;
                }
                continue;
            }
            if c == b'\n' {
                return false; // unterminated: not a regex
            }
            if c == b'[' {
                in_class = true;
            } else if c == b']' {
                in_class = false;
            } else if c == b'/' && !in_class {
                i += 1;
                while i < n && is_id_part(b[i]) {
                    i += 1;
                }
                self.push(Kind::Regex, &src[st..i], None);
                self.i = i;
                return true;
            }
            i += 1;
        }
        false
    }

    // ---------------------------------------------------------- template mode

    fn lex_template(&mut self) {
        let src = self.src;
        let n = self.b.len();
        let st = self.i;
        let ln = self.line;
        while self.i < n {
            let c = self.b[self.i];
            if c == b'\\' {
                self.i = (self.i + 2).min(n);
                self.align();
                continue;
            }
            if c == b'`' {
                self.push(Kind::Template, &src[st..self.i], Some(ln));
                self.i += 1;
                self.frames.pop();
                return;
            }
            if c == b'$' && self.i + 1 < n && self.b[self.i + 1] == b'{' {
                self.push(Kind::Template, &src[st..self.i], Some(ln));
                self.i += 2;
                self.frames.push(Frame::expr(false));
                return;
            }
            if c == b'\n' {
                self.line += 1;
            }
            self.i += 1;
        }
        self.push(Kind::Template, &src[st..self.i], Some(ln));
        self.frames.pop();
    }

    // --------------------------------------------------------------- JSX mode

    fn jsx_allowed(&self) -> bool {
        match self.out.last() {
            None => true,
            Some(p) => match p.kind {
                Kind::Punct => !matches!(p.text, ")" | "]" | "}" | "++" | "--"),
                Kind::Ident => VALUE_KEYWORDS.contains(&p.text),
                _ => false,
            },
        }
    }

    /// Confirm by lookahead that a `<` really opens a JSX element. Without this
    /// check, `f(a < b)` would be mistaken for JSX.
    fn try_jsx_open(&mut self) -> bool {
        let b = self.b;
        let n = b.len();
        let mut j = self.i + 1;
        while j < n && (b[j] == b' ' || b[j] == b'\t' || b[j] == b'\r' || b[j] == b'\n') {
            j += 1;
        }
        let ok = if j < n && b[j] == b'>' {
            true // fragment `<>`
        } else if j < n && is_id_start(b[j]) {
            while j < n && (is_id_part(b[j]) || b[j] == b'.' || b[j] == b'-' || b[j] == b':') {
                j += 1;
            }
            let mut k = j;
            while k < n && (b[k] == b' ' || b[k] == b'\t' || b[k] == b'\r' || b[k] == b'\n') {
                k += 1;
            }
            k < n && (b[k] == b'>' || b[k] == b'/' || b[k] == b'{' || is_id_start(b[k]))
        } else {
            false
        };
        if !ok {
            return false;
        }
        self.i += 1;
        self.push(Kind::Punct, "<", None);
        self.frames.push(Frame::new(FrameKind::JsxTag));
        true
    }

    fn lex_jsx_tag(&mut self) {
        let src = self.src;
        let b = self.b;
        let n = b.len();
        let c = b[self.i];

        if c == b' ' || c == b'\t' || c == b'\r' {
            self.i += 1;
            return;
        }
        if c == b'\n' {
            self.line += 1;
            self.i += 1;
            return;
        }
        if c == b'/' {
            let started = self.frames.last().map(|f| f.started).unwrap_or(false);
            if !started {
                // `</name>` or `</>`
                self.frames.last_mut().unwrap().closing = true;
                self.i += 1;
                return;
            }
            if self.i + 1 < n && b[self.i + 1] == b'>' {
                self.push(Kind::Punct, "/>", None);
                self.i += 2;
                self.frames.pop();
                return;
            }
            self.i += 1;
            return;
        }
        if c == b'>' {
            self.i += 1;
            self.push(Kind::Punct, ">", None);
            let closing = self.frames.last().map(|f| f.closing).unwrap_or(false);
            self.frames.pop();
            if closing {
                if self.frames.last().map(|f| f.kind) == Some(FrameKind::JsxChildren) {
                    self.frames.pop();
                }
            } else {
                self.frames.push(Frame::new(FrameKind::JsxChildren));
            }
            return;
        }
        if c == b'{' {
            self.i += 1;
            self.push(Kind::Punct, "{", None);
            self.frames.push(Frame::expr(true));
            return;
        }
        if c == b'\'' || c == b'"' {
            self.lex_string(c);
            return;
        }
        if c == b'=' {
            self.i += 1;
            self.push(Kind::Punct, "=", None);
            return;
        }
        if is_id_start(c) {
            let st = self.i;
            while self.i < n
                && (is_id_part(b[self.i])
                    || b[self.i] == b'.'
                    || b[self.i] == b'-'
                    || b[self.i] == b':')
            {
                self.i += 1;
            }
            self.push(Kind::Ident, &src[st..self.i], None);
            self.frames.last_mut().unwrap().started = true;
            return;
        }
        let st = self.i;
        self.i += 1;
        self.align();
        self.push(Kind::Punct, &src[st..self.i], None);
    }

    fn lex_jsx_children(&mut self) {
        let src = self.src;
        let b = self.b;
        let n = b.len();
        let st = self.i;
        let ln = self.line;
        while self.i < n && b[self.i] != b'<' && b[self.i] != b'{' {
            if b[self.i] == b'\n' {
                self.line += 1;
            }
            self.i += 1;
        }
        if self.i > st {
            self.push(Kind::JsxText, &src[st..self.i], Some(ln));
            return;
        }
        if b[self.i] == b'{' {
            self.i += 1;
            self.push(Kind::Punct, "{", None);
            self.frames.push(Frame::expr(true));
            return;
        }
        self.i += 1;
        self.push(Kind::Punct, "<", None);
        self.frames.push(Frame::new(FrameKind::JsxTag));
    }
}
