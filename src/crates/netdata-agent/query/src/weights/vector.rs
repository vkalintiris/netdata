//! Test support for the vectors C made (`tests/vectors/`, by `tests/oracle/gen-ks-vectors.sh`).
//!
//! glibc picks its `exp`, `log`, `log1p` and `pow` by the CPU (FMA with AVX2, or not), and the variants' last bits
//! differ, for C and for Rust alike. A vector's header names the glibc and the CPU class it was made on. On such a
//! machine an answer must be C's bit for bit; elsewhere it must be within rounding of C's, and the test says that
//! it compared no bits. A passing test's output is not shown, so where the vectors are made a mismatch must not
//! pass quietly (a glibc upgrade would do it): with `NETDATA_TEST_C_VECTOR_BITS=1` in the environment, which the
//! effort's gates set, a machine of another class fails the test.

use std::fmt::Arguments;
use std::process::Command;

/// A vector C made.
pub(super) struct Vector {
    /// The lines after the four of the header.
    pub(super) lines: Vec<String>,
    /// Whether this machine is of the class the header names.
    exact: bool,
    /// The header's class and this machine's, as `glibc 2.41, cpu fma+avx2`.
    made_on: String,
    here: String,
}

/// `glibc 2.41` on a glibc system: what the generator prints of `gnu_get_libc_version()`.
fn glibc() -> String {
    let version = Command::new("getconf").arg("GNU_LIBC_VERSION").output().ok();
    let version = version.filter(|out| out.status.success());
    version.map_or_else(|| "no glibc".to_owned(), |out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Whether glibc picks its FMA variants here.
#[cfg(target_arch = "x86_64")]
fn fma_variants() -> bool {
    std::arch::is_x86_feature_detected!("fma") && std::arch::is_x86_feature_detected!("avx2")
}

#[cfg(not(target_arch = "x86_64"))]
fn fma_variants() -> bool {
    false
}

impl Vector {
    /// `tests/vectors/<name>`.
    pub(super) fn read(name: &str) -> Self {
        let path = format!("{}/tests/vectors/{name}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let mut lines = text.lines().map(str::to_owned);
        // the second and the third line: `# glibc 2.41`, `# cpu fma+avx2`
        let header: Vec<String> = lines.by_ref().take(4).collect();
        let made_on = format!("{}, {}", &header[1][2..], &header[2][2..]);
        let here = format!("{}, cpu {}", glibc(), if fma_variants() { "fma+avx2" } else { "other" });
        let exact = made_on == here;
        let required = std::env::var_os("NETDATA_TEST_C_VECTOR_BITS").is_some_and(|value| value == "1");
        assert!(
            exact || !required,
            "{name}: made on `{made_on}`, this machine is `{here}`: regenerate it with tests/oracle/gen-ks-vectors.sh"
        );
        Vector { lines: lines.collect(), exact, made_on, here }
    }

    /// `answer` against C's: the same bits on the vector's class of machine, within rounding elsewhere. Where C
    /// answered NaN, any NaN.
    pub(super) fn assert_as_c(&self, answer: f64, expected: f64, what: Arguments<'_>) {
        if expected.is_nan() {
            assert!(answer.is_nan(), "{what} = {answer:e}, C's NaN");
        } else if self.exact {
            assert_eq!(answer.to_bits(), expected.to_bits(), "{what} = {answer:e}, C's {expected:e}");
        } else {
            // relative, so the far tail's small answers are judged too; the floor is for the subnormal ones
            let near = (answer - expected).abs() <= 1e-300 + 1e-9 * expected.abs();
            assert!(near, "{what} = {answer:e}, C's {expected:e}");
        }
    }

    /// Says on stderr how many answers were compared, and whether bit for bit.
    pub(super) fn report(&self, name: &str, compared: usize) {
        if self.exact {
            eprintln!("{name}: {compared} answers are C's bit for bit ({})", self.here);
        } else {
            eprintln!(
                "{name}: NO BITS COMPARED: the vector was made on `{}`, this machine is `{}`. {compared} answers are \
                 within rounding of C's; regenerate the vector here with tests/oracle/gen-ks-vectors.sh to compare \
                 bits",
                self.made_on, self.here
            );
        }
    }
}

/// A double from its bits in hex, as the vectors write them.
pub(super) fn double(hex: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(hex, 16).unwrap())
}
