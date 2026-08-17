use rand::{Rng, RngExt, distr::Distribution, rng};

const UNAMBIGUOUS_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

struct Unambiguous;

impl Distribution<char> for Unambiguous {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> char {
        UNAMBIGUOUS_ALPHABET[rng.random_range(0..UNAMBIGUOUS_ALPHABET.len())] as char
    }
}

pub fn unambiguous_string(len: usize) -> String {
    Unambiguous.sample_iter(&mut rng()).take(len).collect()
}
