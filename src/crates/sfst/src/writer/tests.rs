use super::*;

#[test]
fn error_on_no_primary() {
    let writer = Writer::new();
    let mut buf = Vec::new();
    assert!(matches!(writer.write_to(&mut buf), Err(Error::NoPrimary)));
}

#[test]
fn error_on_no_stream_batches() {
    let mut writer = Writer::new();
    writer.set_primary(vec![1, 2, 3]);
    writer.set_timestamps(vec![4, 5, 6]);
    let mut buf = Vec::new();
    assert!(matches!(
        writer.write_to(&mut buf),
        Err(Error::InvalidStreamBatchCount(0))
    ));
}
