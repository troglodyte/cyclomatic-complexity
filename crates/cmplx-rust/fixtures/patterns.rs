pub enum Token {
    Word(String),
    Num(i64),
    Open,
    Close,
    End,
}

// 1 base
// + arm Word (1) + guard (1) + the `&&` inside the guard (1)
// + arm Num (1)
// + arm Open|Close (1 arm + 1 alternative)
// + arm End (1)
// + catch-all (0)
// = 8
pub fn describe(t: Token, strict: bool) -> &'static str {
    match t {
        Token::Word(w) if strict && !w.is_empty() => "word",
        Token::Num(_) => "num",
        Token::Open | Token::Close => "bracket",
        Token::End => "end",
        _ => "other",
    }
}

// 1 + let-else + if let + alternative = 4
pub fn unwrap_pair(a: Option<Token>, b: Option<u8>) -> u8 {
    let Some(_x) = b else {
        return 0;
    };
    if let Some(Token::Open | Token::Close) = a {
        return 1;
    }
    2
}

// 1 base; `?` is free by default.
pub fn parse(s: &str) -> Result<i64, std::num::ParseIntError> {
    let n: i64 = s.trim().parse()?;
    let m: i64 = s.trim().parse()?;
    Ok(n + m)
}
