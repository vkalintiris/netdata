//! The numbers of the weights endpoints (`src/web/api/queries/weights.c`): how much each metric of the hosts in
//! scope stands out in a highlighted window. For now the Kolmogorov-Smirnov distribution alone, which the `ks2`
//! method ends in.

pub mod ks;
#[cfg(test)]
mod vector;
