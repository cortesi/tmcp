//! #[group] on an unsupported item is a compile error.
use tmcp::group;

#[group]
trait NotAGroup {}

fn main() {}
