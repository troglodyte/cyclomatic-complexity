// Hand-checked scores:
//   plain    1
//   branchy  5  = 1 + if + && + for + if
//   add_one  1, plus its closure as a separate row named `map`, scoring 1
pub fn plain() -> u8 {
    7
}

pub fn branchy(a: bool, b: bool, v: &[u8]) -> u8 {
    if a && b {
        return 1;
    }
    for x in v {
        if *x > 3 {
            return *x;
        }
    }
    0
}

pub fn add_one(v: &[u8]) -> Vec<u8> {
    v.iter().map(|x| x + 1).collect()
}
