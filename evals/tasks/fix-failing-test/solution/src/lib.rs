/// The total of a bill split into `parts` equal shares, rounded up to whole
/// cents.
pub fn share(total_cents: u64, parts: u64) -> u64 {
    total_cents.div_ceil(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_round_up() {
        assert_eq!(share(1000, 3), 334);
        assert_eq!(share(900, 3), 300);
    }
}
