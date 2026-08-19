use super::*;

#[test]
fn nan_and_empty_values_become_null() {
    assert_eq!(number_or_float("nan"), serde_json::Value::Null);
    assert_eq!(number_or_float("NaN"), serde_json::Value::Null);
    assert_eq!(number_or_float(""), serde_json::Value::Null);
    // Anything that is not a number at all stays a string rather than vanishing.
    assert_eq!(number_or_float("n/a"), json!("n/a"));
}

#[test]
fn numbers_keep_the_shape_they_arrived_in() {
    // An integer must not turn into a float on the way through: the shell wrote the
    // argument verbatim, and receivers compare against `80`, not `80.0`.
    assert_eq!(number_or_float("80").to_string(), "80");
    assert_eq!(number_or_float("0").to_string(), "0");
    assert_eq!(number_or_float("91.5").to_string(), "91.5");
    assert_eq!(number_or_float("-3").to_string(), "-3");
}

#[test]
fn integer_like_strings_pass_through_number_or_string() {
    assert_eq!(number_or_string("22").to_string(), "22");
    assert_eq!(number_or_string("").to_string(), "\"\"");
    assert_eq!(number_or_string("abc").to_string(), "\"abc\"");
}
