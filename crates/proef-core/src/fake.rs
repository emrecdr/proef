//! Deterministic synthetic data for `${fake:*}` (ADR-0005).
//!
//! Determinism contract: values derive from the injected run id (FNV-1a →
//! `SplitMix64`) plus the generator name and its occurrence index within one
//! resolution — the same run id reproduces the same values (pin a run id to
//! reproduce a flaky run), distinct run ids produce independent data. The
//! module is dependency-free so the sequence is stable across crate versions.
//!
//! Unknown generator names are rejected statically at pack load (the validator
//! consults [`is_known_generator`]) — a typo never survives to run time.

/// Steele/Vigna `SplitMix64` — small, fast, deterministic.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[usize::try_from(self.below(items.len() as u64)).unwrap_or(0)]
    }
}

/// 64-bit FNV-1a, turning strings into seeds.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

const FIRST_NAMES: &[&str] = &[
    "Alice", "Bob", "Carol", "David", "Emma", "Finn", "Sofie", "Lars", "Noor", "Sem",
];
const LAST_NAMES: &[&str] = &[
    "Jansen", "Vries", "Dekker", "Visser", "Smit", "Meijer", "Smith", "Brown", "Jones", "Mulder",
];
const CITIES: &[&str] = &[
    "Amsterdam",
    "Rotterdam",
    "Utrecht",
    "Den Haag",
    "Eindhoven",
    "Groningen",
];
const STREETS: &[&str] = &[
    "Kerkstraat",
    "Schoolstraat",
    "Dorpsstraat",
    "Molenweg",
    "Hoofdstraat",
    "Stationsweg",
];
const WORDS: &[&str] = &[
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
];

/// Every generator name `${fake:*}` understands (fixed vocabulary;
/// kept in sync with [`generate`] by a test).
pub const GENERATORS: &[&str] = &[
    "firstName",
    "lastName",
    "name",
    "fullName",
    "email",
    "username",
    "phoneNL",
    "postCode",
    "city",
    "street",
    "int",
    "number",
    "digits4",
    "digits8",
    "bool",
    "word",
    "uuid",
];

/// Whether `name` is a known generator (static pack validation).
pub fn is_known_generator(name: &str) -> bool {
    GENERATORS.contains(&name)
}

/// One deterministic value for `generator`, derived from `(run_id, occurrence)`.
/// `None` for unknown generators (callers reject those statically).
pub fn generate(run_id: &str, occurrence: usize, generator: &str) -> Option<String> {
    let seed = fnv1a(run_id.as_bytes())
        ^ fnv1a(generator.as_bytes()).rotate_left(17)
        ^ (occurrence as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut rng = SplitMix64::new(seed);
    let value = match generator {
        "firstName" => rng.pick(FIRST_NAMES).to_owned(),
        "lastName" => rng.pick(LAST_NAMES).to_owned(),
        "name" | "fullName" => format!("{} {}", rng.pick(FIRST_NAMES), rng.pick(LAST_NAMES)),
        "email" => format!(
            "{}.{}@example.com",
            rng.pick(FIRST_NAMES).to_lowercase(),
            rng.pick(LAST_NAMES).to_lowercase()
        ),
        "username" => format!(
            "{}{}",
            rng.pick(FIRST_NAMES).to_lowercase(),
            rng.below(1000)
        ),
        "phoneNL" => format!("06{:08}", rng.below(100_000_000)),
        "postCode" => format!(
            "{:04} {}{}",
            1000 + rng.below(9000),
            upper_letter(&mut rng),
            upper_letter(&mut rng)
        ),
        "city" => rng.pick(CITIES).to_owned(),
        "street" => format!("{} {}", rng.pick(STREETS), 1 + rng.below(200)),
        "int" | "number" => rng.below(10_000).to_string(),
        "digits4" => format!("{:04}", rng.below(10_000)),
        "digits8" => format!("{:08}", rng.below(100_000_000)),
        "bool" => (rng.next_u64() & 1 == 1).to_string(),
        "word" => rng.pick(WORDS).to_owned(),
        "uuid" => {
            let (a, b) = (rng.next_u64(), rng.next_u64());
            format!(
                "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
                a >> 32,
                (a >> 16) & 0xFFFF,
                a & 0xFFF,
                0x8000 | ((b >> 48) & 0x3FFF),
                b & 0xFFFF_FFFF_FFFF
            )
        }
        _ => return None,
    };
    Some(value)
}

fn upper_letter(rng: &mut SplitMix64) -> char {
    char::from(b'A' + u8::try_from(rng.below(26)).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_generator_generates() {
        for generator in GENERATORS {
            assert!(
                generate("run-1", 0, generator).is_some(),
                "generator `{generator}` listed but not implemented"
            );
        }
        assert!(generate("run-1", 0, "nope").is_none());
    }

    #[test]
    fn deterministic_per_run_id_and_occurrence() {
        let a = generate("run-1", 0, "lastName");
        let b = generate("run-1", 0, "lastName");
        assert_eq!(a, b, "same run id + occurrence → same value");
        let c = generate("run-2", 0, "lastName");
        let d = generate("run-1", 1, "lastName");
        // Distinct streams (probabilistically distinct for these seeds).
        assert!(a != c || a != d, "distinct run/occurrence should vary");
    }

    #[test]
    fn uuid_shape_is_v4ish() {
        let uuid = generate("run-1", 0, "uuid").unwrap_or_default();
        assert_eq!(uuid.len(), 36, "{uuid}");
        assert_eq!(uuid.as_bytes()[14], b'4', "{uuid}");
    }
}
