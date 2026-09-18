//! Sample Rust fixture for code-extraction tests (see SPEC.md §7 M2).

/// Adds two numbers together.
fn add(a: i32, b: i32) -> i32 {
    a + b
}

/// Represents a point in 2D space.
struct Point {
    x: i32,
    y: i32,
}

impl Point {
    fn origin() -> Self {
        Point { x: 0, y: 0 }
    }
}
