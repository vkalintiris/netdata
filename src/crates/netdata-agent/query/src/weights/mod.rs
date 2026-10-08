//! The numbers of the weights endpoints (`src/web/api/queries/weights.c`): how much each metric of the hosts in
//! scope stands out in a highlighted window. For now the statistics of the `ks2` method: the two-sample
//! Kolmogorov-Smirnov test and the distribution it ends in.

pub mod ks;
pub mod ks2;
#[cfg(test)]
mod vector;
