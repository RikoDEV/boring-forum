//! Solved answers.

crate::sql::set_col!(
/// Sets or clears the topic's answer.
    set_solved, "items", "solved_post_number", i64, Option<i32>);
