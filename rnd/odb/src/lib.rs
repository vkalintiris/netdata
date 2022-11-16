// use std::collections::HashMap;
use hashbrown::HashMap;

type OID = u32;

#[derive(Debug)]
pub struct ODB {
    counter: OID,
    objects: HashMap<OID, String>,
}

impl ODB {
    pub fn new() -> Self {
        Self {
            counter: 0u32,
            objects: HashMap::with_capacity(10000),
        }
    }

    pub fn add(&mut self, sid: &str) -> OID {
        self.counter += 1;
        self.objects.insert(self.counter, String::from(sid));
        self.counter
    }

    pub fn get(&self, oid: OID) -> Option<&String> {
        self.objects.get(&oid)
    }

    pub fn remove(&mut self, oid: OID) -> String {
        self.objects.remove(&oid).expect("requested oid not in odb")
    }

    pub fn count(&self) -> usize {
        self.objects.len()
    }

    pub fn strlen(&self) -> usize {
        let mut n = 0;
        for v in self.objects.values() {
            n += v.len()
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use more_asserts as ma;

    #[test]
    fn test_odb() {
        let mut odb = ODB::new();

        let oid1 = odb.add("a");
        let oid2 = odb.add("b");
        ma::assert_gt!(oid2, oid1);

        let s2 = odb.remove(oid2);
        assert_eq!(s2, "b");

        let oid3 = odb.add("c");
        ma::assert_gt!(oid3, oid2);

        let v = odb.get(oid3);
        assert_eq!(v.is_some(), true);
        let s = v.unwrap();
        assert_eq!(*s, "c");
    }
}
