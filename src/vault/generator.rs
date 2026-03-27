use rand::Rng;

const LOWERCASE: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
const UPPERCASE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &[u8] = b"0123456789";
const SYMBOLS: &[u8] = b"!@#$%^&*-_+=";

/// Generate a random password of the given length.
/// Guarantees at least one character from each character class.
pub fn generate_password(length: usize) -> String {
    let length = length.max(4);
    let mut rng = rand::thread_rng();

    let all_chars: Vec<u8> = [LOWERCASE, UPPERCASE, DIGITS, SYMBOLS].concat();

    let mut password: Vec<u8> = vec![
        LOWERCASE[rng.gen_range(0..LOWERCASE.len())],
        UPPERCASE[rng.gen_range(0..UPPERCASE.len())],
        DIGITS[rng.gen_range(0..DIGITS.len())],
        SYMBOLS[rng.gen_range(0..SYMBOLS.len())],
    ];

    for _ in 4..length {
        password.push(all_chars[rng.gen_range(0..all_chars.len())]);
    }

    for i in (1..password.len()).rev() {
        let j = rng.gen_range(0..=i);
        password.swap(i, j);
    }

    String::from_utf8(password).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_password_length() {
        let pw = generate_password(20);
        assert_eq!(pw.len(), 20);
    }

    #[test]
    fn test_generate_password_has_all_classes() {
        for _ in 0..10 {
            let pw = generate_password(20);
            assert!(pw.chars().any(|c| c.is_ascii_lowercase()), "missing lowercase");
            assert!(pw.chars().any(|c| c.is_ascii_uppercase()), "missing uppercase");
            assert!(pw.chars().any(|c| c.is_ascii_digit()), "missing digit");
            assert!(pw.chars().any(|c| "!@#$%^&*-_+=".contains(c)), "missing symbol");
        }
    }

    #[test]
    fn test_generate_password_minimum_length() {
        let pw = generate_password(2);
        assert_eq!(pw.len(), 4);
    }
}
