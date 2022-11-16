#![allow(unused_imports, dead_code)]

#[derive(Debug)]
struct Host {
    hostname: String,
}

enum Event {
    NewHostCreated(Host),
}

pub fn add(left: usize, right: usize) -> usize {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}
