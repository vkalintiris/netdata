use super::*;

#[test]
fn numeric_replies_in_the_error_range_are_detected() {
    let reply =
        ":irc.example.net 001 nd :Welcome\r\n:irc.example.net 366 nd #ops :End of NAMES\r\n";
    assert_eq!(error_code(reply), None);

    let reply = ":irc.example.net 433 * nd :Nickname is already in use\r\n";
    assert_eq!(error_code(reply), Some(433));

    let reply = ":irc.example.net 001 nd :Welcome\r\n:irc.example.net 473 nd #ops :Cannot join\r\n";
    assert_eq!(error_code(reply), Some(473));
}

#[test]
fn non_numeric_and_out_of_range_fields_are_ignored() {
    assert_eq!(error_code("PING :12345\r\n"), None);
    // 200-399 are informational.
    assert_eq!(error_code(":s 372 nd :- motd\r\n"), None);
    assert_eq!(error_code(""), None);
    // A 6xx numeric is not in the error range the script checked.
    assert_eq!(error_code(":s 600 nd :x\r\n"), None);
}
