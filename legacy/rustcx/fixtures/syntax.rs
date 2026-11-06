//! Syntax that trips token-level scanners: raw strings, nested block comments,
//! lifetimes against char literals, generics against shift operators.

/* a block comment /* nested inside another */ still a comment */

pub struct Holder<'a> {
    pub text: &'a str,
}

impl<'a> Holder<'a> {
    // 1 + if = 2. The raw string contains `if` and `&&` as text, not code.
    pub fn shout(&self) -> String {
        let banner = r#"if a && b { "quoted" }"#;
        if self.text.is_empty() {
            return banner.to_string();
        }
        self.text.to_uppercase()
    }
}

// 1 + if + && = 3. `'x'` is a char literal, `<u8>` is a generic, `>>` is a
// shift — none of which is a decision point.
pub fn ambiguous(n: u32, c: char) -> u32 {
    let v = Vec::<Vec<u8>>::new();
    if c == 'x' && v.is_empty() {
        return n >> 2;
    }
    n
}

// A raw identifier is still a name. `Ident::to_string` keeps the `r#`, so this
// function is reported as `r#match`.
pub fn r#match(a: bool) -> bool {
    a
}

macro_rules! ignored {
    ($x:expr) => {
        if $x {
            1
        } else {
            0
        }
    };
}

// 1 base. The branch inside the macro invocation is invisible to syn — a
// documented limitation, asserted here so it is a deliberate behavior.
pub fn uses_macro(a: bool) -> u8 {
    ignored!(a)
}
