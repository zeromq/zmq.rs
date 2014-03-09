pub struct Context {
    starting: bool,
    terminating: bool,
}

impl Context {
    pub fn new() -> Context {
        Context {
            starting: true,
            terminating: false,
        }
    }
}

#[cfg(test)]
mod test {
    use ctx::Context;

    #[test]
    fn test_new() {
        let ctx = Context::new();
        assert_eq!(ctx.starting, true);
    }
}

