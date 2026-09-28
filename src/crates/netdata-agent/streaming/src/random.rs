//! `os_random32()` and `os_random()` (`src/libnetdata/os/random.c`): uniform draws from a generator the system
//! seeds. C draws from OpenSSL's `RAND_bytes()`; which generator is not observable.

/// `os_random32()`.
pub fn os_random32() -> u32 {
    rand::random()
}

/// `os_random(max)`: uniform in `[0, max)`, 0 when `max` is 0 or 1.
pub fn os_random(max: u64) -> u64 {
    if max <= 1 { 0 } else { rand::random_range(0..max) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_stay_below_the_bound() {
        assert_eq!((os_random(0), os_random(1)), (0, 0));
        assert!((0..1000).all(|_| os_random(3) < 3));
    }
}
