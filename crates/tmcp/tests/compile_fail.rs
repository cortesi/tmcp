//! Compile-fail tests for the tmcp procedural macros.

#[cfg(test)]
mod tests {
    #[test]
    fn compile_fail() {
        let t = trybuild::TestCases::new();
        t.compile_fail("tests/compile_fail/*.rs");
    }
}
