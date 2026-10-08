//! The numbers of the weights endpoints (`src/web/api/queries/weights.c`): how much each metric of the hosts in
//! scope stands out in a highlighted window. For now the statistics of the `ks2` method (the two-sample
//! Kolmogorov-Smirnov test and the distribution it ends in), the four methods on one metric and their results.

pub mod ks;
pub mod ks2;
pub mod methods;
pub mod results;

/// `WEIGHTS_METHOD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Ks2,
    Volume,
    AnomalyRate,
    Value,
}

/// `weights_methods[]`.
const METHOD_NAMES: [(&str, Method); 4] = [
    ("ks2", Method::Ks2),
    ("volume", Method::Volume),
    ("anomaly-rate", Method::AnomalyRate),
    ("value", Method::Value),
];

impl Method {
    /// `weights_string_to_method()`: a name that is none of the four is `ks2`.
    pub fn parse(name: &[u8]) -> Self {
        METHOD_NAMES.iter().find(|(known, _)| known.as_bytes() == name).map_or(Method::Ks2, |(_, method)| *method)
    }

    /// `weights_method_to_string()`.
    pub fn name(self) -> &'static str {
        METHOD_NAMES.iter().find(|(_, method)| *method == self).map_or("ks2", |(name, _)| *name)
    }
}
#[cfg(test)]
mod vector;

#[cfg(test)]
mod tests {
    use super::Method;

    /// C's table of the methods' names: the name is exact, and anything else is `ks2`.
    #[test]
    fn a_method_is_known_by_c_s_name() {
        let named = [("ks2", Method::Ks2), ("volume", Method::Volume), ("anomaly-rate", Method::AnomalyRate)];
        for (name, method) in named.into_iter().chain([("value", Method::Value)]) {
            assert_eq!((Method::parse(name.as_bytes()), method.name()), (method, name));
        }
        for name in ["", "KS2", "anomaly_rate", "value ", "nope"] {
            assert_eq!(Method::parse(name.as_bytes()), Method::Ks2, "{name:?}");
        }
    }
}
