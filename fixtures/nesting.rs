// Hand-checked expected scores are asserted in tests/fixtures.rs.
pub struct Engine {
    limit: u8,
}

impl Engine {
    // 1 + if + && = 3
    pub fn allows(&self, n: u8, open: bool) -> bool {
        if n < self.limit && open {
            return true;
        }
        false
    }

    // 1 + for = 2. The closure is a separate row.
    pub fn tally(&self, xs: &[u8]) -> usize {
        let mut n = 0;
        for _x in xs {
            n += 1;
        }
        xs.iter().filter(|x| **x > self.limit || **x == 0).count() + n
    }
}

pub trait Policy {
    fn name(&self) -> &str;
    // 1 + if = 2
    fn check(&self, ok: bool) -> bool {
        if ok {
            true
        } else {
            false
        }
    }
}

// 1 + if = 2, and the nested fn is its own row at 1 + if = 2.
pub fn outer(a: bool) -> u8 {
    fn inner(b: bool) -> u8 {
        if b {
            1
        } else {
            0
        }
    }
    if a {
        inner(a)
    } else {
        0
    }
}
